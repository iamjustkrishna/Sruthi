//! One dictation: record while the key is held, then transcribe and format.
//!
//! Long dictations are transcribed in pieces split at pauses (`chunks.rs`).
//! While the key is held, a background thread transcribes each piece as
//! soon as it's complete, so releasing the key only waits for the last
//! piece. Short dictations are one piece, transcribed after release.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use transcribe_rs::{SpeechModel, TranscribeOptions};

use crate::audio::{Recording, TARGET_RATE};
use crate::chunks;
use crate::context::AppKind;
use crate::enhance;
use crate::format::Formatter;

pub type Engine = Arc<Mutex<Option<Box<dyn SpeechModel + Send>>>>;

/// Dictations shorter than this (seconds) are treated as accidental taps.
const MIN_DICTATION_SECS: f32 = 0.3;
/// How often the background thread looks for a finished piece.
const POLL: Duration = Duration::from_millis(250);
/// New audio (seconds) that makes another preview worth a request.
const PREVIEW_MIN_SECS: f32 = 0.6;
/// A release this soon after a preview reuses it instead of transcribing again.
const PREVIEW_REUSE_SECS: f32 = 0.3;
/// Previews longer than this only cover the newest stretch (not reusable).
const PREVIEW_WINDOW_SECS: f32 = 6.0;

pub struct Session {
    recording: Arc<Recording>,
    engine: Engine,
    formatter: Arc<Formatter>,
    smart: bool,
    /// The user's dictionary at the time the key was pressed.
    words: Vec<String>,
    /// The app that had focus when the key was pressed, filled in off the
    /// press path (UI Automation can take a while).
    app: Arc<Mutex<AppKind>>,
    /// Transcribes pieces while the key is held.
    early: Early,
    /// Press → recording-live, the "start" stage of the timings log.
    mic_ready: Duration,
    /// Whether the press found the mic live or opened it cold.
    start_mode: StartMode,
    /// What the press did with its time, for the timings log.
    press_note: String,
}

/// A preview of the piece still being spoken.
#[derive(Clone)]
struct Preview {
    /// Recording samples (device rate) covered at the end.
    end: usize,
    text: String,
    /// True when it covered the whole piece, not just the newest window.
    full: bool,
}

/// True when the growing piece has enough new audio for another preview.
pub fn preview_due(done: usize, len: usize, preview_end: usize, rate: u32) -> bool {
    let from = done.max(preview_end);
    (len.saturating_sub(from) as f32) / rate as f32 >= PREVIEW_MIN_SECS
}

/// True when a release can take the preview as the tail instead of paying
/// for the same audio twice.
fn preview_reusable(len: usize, preview: &Preview, rate: u32) -> bool {
    preview.full
        && !preview.text.is_empty()
        && (len.saturating_sub(preview.end) as f32) / rate as f32 <= PREVIEW_REUSE_SECS
}

/// How the press turned into live recording, for the timings log.
pub enum StartMode {
    /// The always-ready mic took the press; nothing waited.
    Instant,
    /// Recording opened cold at the press, for this reason.
    Cold(String),
}

impl StartMode {
    /// The short label the timings log carries: `instant` or `cold:<why>`.
    pub fn label(&self) -> String {
        match self {
            StartMode::Instant => "instant".into(),
            StartMode::Cold(reason) => format!("cold:{reason}"),
        }
    }
}

impl Session {
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        recording: Recording,
        engine: Engine,
        formatter: Arc<Formatter>,
        smart: bool,
        words: Vec<String>,
        app: Arc<Mutex<AppKind>>,
        press: Instant,
        start_mode: StartMode,
        press_note: String,
        on_partial: Arc<dyn Fn(String) + Send + Sync>,
    ) -> Self {
        let recording = Arc::new(recording);
        let early = Early::start(recording.clone(), engine.clone(), on_partial);
        Self {
            recording,
            engine,
            formatter,
            smart,
            words,
            app,
            mic_ready: press.elapsed(),
            start_mode,
            press_note,
            early,
        }
    }

    /// Press → recording-live: what the user waits before any speech can
    /// be captured.
    pub fn start_delay(&self) -> Duration {
        self.mic_ready
    }

    /// How the press got live, for the timings log.
    pub fn start_detail(&self) -> String {
        match &self.press_note.is_empty() {
            true => self.start_mode.label(),
            false => format!("{} {}", self.start_mode.label(), self.press_note),
        }
    }

    /// The app the text is going to, as known once the page lookup lands.
    pub fn app(&self) -> AppKind {
        *self.app.lock().unwrap()
    }

    /// Stops recording, transcribes and formats. Recordings shorter than
    /// an accidental tap come back with no audio.
    pub fn finish(self) -> Result<Outcome> {
        crate::timing::measure("stop", || self.recording.stop())?;
        // Read before any field moves out of `self`.
        let app = self.app();
        // Pieces done while the key was held (waiting for one in progress).
        let early = self.early.finish();
        let device = self.recording.device().to_string();
        let audio = crate::timing::measure("resample", || {
            self.recording.slice_16k(0, self.recording.len())
        });
        if (audio.len() as f32) / (TARGET_RATE as f32) < MIN_DICTATION_SECS {
            return Ok(Outcome {
                device,
                ..Outcome::default()
            });
        }
        // Digital silence: the microphone itself delivers nothing.
        if crate::audio::rms(&audio) < crate::audio::DEAD_MIC_RMS {
            return Ok(Outcome {
                device,
                dead_mic: true,
                ..Outcome::default()
            });
        }

        // The rest, in pieces too if it's long (the background thread
        // stopped early, or the model was slow). A preview fresh enough to
        // be the tail is taken as-is: no point paying for it twice.
        let mut texts = early.texts;
        let mut from = early.done;
        if let Some(preview) = &early.preview {
            if preview_reusable(self.recording.len(), preview, TARGET_RATE) {
                texts.push(preview.text.clone());
                from = preview.end;
                log::debug!("Reused the last preview for the tail");
            }
        }
        let mut rest = self.recording.slice_16k(from, self.recording.len());
        while let Some(cut) = chunks::next_cut(&rest, true) {
            texts.push(transcribe_piece(&self.engine, &rest[..cut])?);
            rest.drain(..cut);
        }
        texts.push(transcribe_piece(&self.engine, &rest)?);
        if texts.len() > 1 {
            log::info!(
                "Transcribed in {} pieces, {} while the key was held",
                texts.len(),
                early.pieces
            );
        }

        let heard = chunks::join(&texts);
        let text = if heard.is_empty() {
            String::new()
        } else {
            crate::timing::measure("format", || {
                self.formatter.format(&heard, self.smart, &self.words, app)
            })
        };
        // Local debug log only (stdout in dev builds); helps tune the rules.
        log::info!("Heard:  {heard:?}");
        log::info!("Pasted: {text:?}");
        Ok(Outcome {
            audio,
            heard,
            text,
            device,
            dead_mic: false,
        })
    }
}

/// Transcribes a whole recording the way a dictation's tail is: in pieces
/// split at pauses, joined back into one text.
pub(crate) fn transcribe_audio(engine: &Engine, audio: &[f32]) -> Result<String> {
    let mut texts = Vec::new();
    let mut rest = audio.to_vec();
    while let Some(cut) = chunks::next_cut(&rest, true) {
        texts.push(transcribe_piece(engine, &rest[..cut])?);
        rest.drain(..cut);
    }
    texts.push(transcribe_piece(engine, &rest)?);
    Ok(chunks::join(&texts))
}

/// Boosts quiet speech and transcribes one piece; silence comes back empty
/// without running the model.
fn transcribe_piece(engine: &Engine, audio: &[f32]) -> Result<String> {
    let Some(prepared) = crate::timing::measure("prepare", || enhance::prepare(audio)) else {
        log::info!(
            "No speech in {:.1}s of audio",
            audio.len() as f32 / TARGET_RATE as f32
        );
        return Ok(String::new());
    };
    let stages_before = crate::timing::summary();
    let started = Instant::now();
    let heard = {
        let mut engine = engine.lock().unwrap();
        let model = engine
            .as_mut()
            .ok_or_else(|| anyhow!("Speech model isn't loaded"))?;
        model
            .transcribe(&prepared, &TranscribeOptions::default())
            .map_err(|e| anyhow!("Transcription failed: {e}"))?
            .text
            .trim()
            .to_string()
    };
    // Hybrid reports Parakeet and Whisper separately; other models as one step.
    if crate::timing::summary() == stages_before {
        crate::timing::add("speech", started.elapsed());
    }
    Ok(heard)
}

/// The background thread that transcribes finished pieces — and previews
/// the piece still being spoken — while the key is held.
struct Early {
    /// Dropped when the key is released.
    stop: mpsc::Sender<()>,
    thread: JoinHandle<Progress>,
}

/// How far the background thread got.
#[derive(Default)]
struct Progress {
    /// Where the audio not yet transcribed starts (device-rate samples).
    done: usize,
    texts: Vec<String>,
    /// Pieces transcribed (including silent ones, which have no text).
    pieces: usize,
    /// Time spent transcribing them.
    busy: Duration,
    /// The newest preview, if the release may reuse it.
    preview: Option<Preview>,
}

/// Reports the text so far: finished pieces plus, while recording, the
/// piece still being spoken.
fn announce(on_partial: &Arc<dyn Fn(String) + Send + Sync>, texts: &[String], tail: Option<&str>) {
    let mut all = texts.to_vec();
    if let Some(tail) = tail {
        all.push(tail.to_string());
    }
    let text = chunks::join(&all);
    if !text.is_empty() {
        on_partial(text);
    }
}

impl Early {
    fn start(
        recording: Arc<Recording>,
        engine: Engine,
        on_partial: Arc<dyn Fn(String) + Send + Sync>,
    ) -> Self {
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = thread::spawn(move || {
            let mut progress = Progress::default();
            while matches!(stopped.recv_timeout(POLL), Err(RecvTimeoutError::Timeout)) {
                let len = recording.len();
                let rest = recording.slice_16k(progress.done, len);
                // Finished pieces first: they are what the final text uses.
                if let Some(cut) = chunks::next_cut(&rest, false) {
                    let started = Instant::now();
                    match transcribe_piece(&engine, &rest[..cut]) {
                        Ok(text) => {
                            progress.busy += started.elapsed();
                            progress.done += recording.device_samples(cut);
                            progress.texts.push(text);
                            progress.pieces += 1;
                            progress.preview = None;
                            announce(&on_partial, &progress.texts, None);
                        }
                        Err(e) => {
                            // The rest is transcribed after release instead.
                            log::warn!("Couldn't transcribe while recording: {e:#}");
                            break;
                        }
                    }
                    continue;
                }
                // Nothing finished: preview the piece still being spoken.
                let preview_end = progress.preview.as_ref().map_or(progress.done, |p| p.end);
                if !preview_due(progress.done, len, preview_end, TARGET_RATE) {
                    continue;
                }
                let full = rest.len() as f32 / TARGET_RATE as f32 <= PREVIEW_WINDOW_SECS;
                let skip = if full {
                    0
                } else {
                    rest.len() - (PREVIEW_WINDOW_SECS * TARGET_RATE as f32) as usize
                };
                let started = Instant::now();
                match transcribe_piece(&engine, &rest[skip..]) {
                    Ok(text) => {
                        crate::timing::add("live", started.elapsed());
                        let preview = Preview {
                            end: len,
                            text,
                            full,
                        };
                        announce(&on_partial, &progress.texts, Some(&preview.text));
                        progress.preview = Some(preview);
                    }
                    Err(e) => {
                        log::warn!("Couldn't preview while recording: {e:#}");
                        break;
                    }
                }
            }
            progress
        });
        Self { stop, thread }
    }

    /// Stops looking for pieces, waits for one in progress, and adds the
    /// time spent to this thread's timings.
    fn finish(self) -> Progress {
        drop(self.stop);
        let waited = Instant::now();
        let progress = self.thread.join().unwrap_or_else(|_| {
            log::warn!("Background transcription panicked");
            Progress::default()
        });
        // "early" ran while the key was held; "early-wait" is the part of
        // a piece still running at release (or the background thread
        // noticing the release).
        if progress.pieces > 0 {
            crate::timing::add("early", progress.busy);
        }
        crate::timing::add("early-wait", waited.elapsed());
        progress
    }
}

/// Result of one dictation.
#[derive(Default)]
pub struct Outcome {
    /// The original 16 kHz recording (not boosted), empty for accidental taps.
    pub audio: Vec<f32>,
    /// What the speech model heard.
    pub heard: String,
    /// The formatted text to paste (empty when nothing was heard).
    pub text: String,
    /// The microphone it was recorded from.
    pub device: String,
    /// The microphone delivered no sound at all.
    pub dead_mic: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_needs_new_audio() {
        let rate = TARGET_RATE;
        let quarter = rate as usize / 4;
        let full = (rate as f32 * 0.6) as usize;
        // Nothing new yet: no request.
        assert!(!preview_due(0, quarter, 0, rate));
        // Enough new audio: preview.
        assert!(preview_due(0, full, 0, rate));
        // A preview covers up to its end; only newer audio asks again.
        let end = rate as usize * 2;
        assert!(!preview_due(0, end + quarter, end, rate));
        assert!(preview_due(0, end + full, end, rate));
        // A finished piece moves the floor past any old preview.
        assert!(!preview_due(end, end + quarter, 0, rate));
    }

    #[test]
    fn release_reuses_only_fresh_full_previews() {
        let rate = TARGET_RATE;
        let preview = |end, text: &str, full| Preview {
            end,
            text: text.into(),
            full,
        };
        let end = rate as usize * 5;
        // Fresh, whole, with words: reuse.
        assert!(preview_reusable(end, &preview(end, "hello", true), rate));
        assert!(preview_reusable(
            end + rate as usize / 4,
            &preview(end, "hello", true),
            rate
        ));
        // Too old, windowed, or silent: transcribe the tail again.
        assert!(!preview_reusable(
            end + rate as usize,
            &preview(end, "hello", true),
            rate
        ));
        assert!(!preview_reusable(end, &preview(end, "hello", false), rate));
        assert!(!preview_reusable(end, &preview(end, "", true), rate));
    }

    /// Word-level differences between two transcripts: (edits, words).
    fn word_diff(a: &str, b: &str) -> (usize, usize, Vec<String>) {
        let words = |s: &str| -> Vec<String> {
            s.split_whitespace()
                .map(|w| {
                    w.chars()
                        .filter(|c| c.is_alphanumeric())
                        .collect::<String>()
                        .to_lowercase()
                })
                .filter(|w| !w.is_empty())
                .collect()
        };
        let (a, b) = (words(a), words(b));
        let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
        for i in 0..=a.len() {
            d[i][0] = i;
        }
        for j in 0..=b.len() {
            d[0][j] = j;
        }
        for i in 1..=a.len() {
            for j in 1..=b.len() {
                let same = if a[i - 1] == b[j - 1] { 0 } else { 1 };
                d[i][j] = (d[i - 1][j - 1] + same)
                    .min(d[i - 1][j] + 1)
                    .min(d[i][j - 1] + 1);
            }
        }
        let (mut i, mut j, mut changes) = (a.len(), b.len(), Vec::new());
        while i > 0 || j > 0 {
            if i > 0 && j > 0 && d[i][j] == d[i - 1][j - 1] + usize::from(a[i - 1] != b[j - 1]) {
                if a[i - 1] != b[j - 1] {
                    changes.push(format!("{} -> {}", a[i - 1], b[j - 1]));
                }
                i -= 1;
                j -= 1;
            } else if i > 0 && d[i][j] == d[i - 1][j] + 1 {
                changes.push(format!("-{}", a[i - 1]));
                i -= 1;
            } else {
                changes.push(format!("+{}", b[j - 1]));
                j -= 1;
            }
        }
        changes.reverse();
        (d[a.len()][b.len()], a.len(), changes)
    }

    /// Transcribes the saved recordings longer than one piece twice: whole
    /// (as before) and in pieces the way a dictation does it, with the
    /// key-held pieces on a simulated clock. Prints both, the words that
    /// differ, and how long the text took after release.
    /// `cargo test --lib pieces_match_whole -- --ignored --nocapture`
    /// (SRUTHI_TEST_MODEL=hybrid picks the engine; parakeet-v3 by default.)
    #[test]
    #[ignore]
    fn pieces_match_whole() {
        let _ = env_logger::builder()
            .is_test(true)
            .filter_level(log::LevelFilter::Warn)
            .try_init();
        let id = std::env::var("SRUTHI_TEST_MODEL").unwrap_or_else(|_| "parakeet-v3".into());
        let words = crate::settings::load().dictionary;
        let mut model = crate::model::find(&id).unwrap().load().unwrap();
        model.set_hotwords(&words);
        model.set_latin_only(true);
        let engine: Engine = Arc::new(Mutex::new(Some(model)));

        let mut files: Vec<_> = std::fs::read_dir(crate::recordings::folder())
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "wav"))
            .collect();
        files.sort();
        let rate = TARGET_RATE as usize;
        let (mut edits, mut total_words) = (0, 0);
        for file in files {
            let audio = transcribe_rs::audio::read_wav_samples(&file).unwrap();
            let secs = audio.len() as f32 / rate as f32;
            if secs < chunks::MIN_PIECE_SECS {
                continue;
            }
            let started = Instant::now();
            let whole = transcribe_piece(&engine, &audio).unwrap();
            let whole_ms = started.elapsed().as_millis();

            // Key held: the background thread polls every 250 ms while free.
            let (mut done, mut busy_until, mut texts, mut cuts) =
                (0usize, 0f32, Vec::new(), Vec::new());
            let step = (POLL.as_secs_f32() * rate as f32) as usize;
            let mut now = step;
            while now <= audio.len() {
                let t = now as f32 / rate as f32;
                if t >= busy_until {
                    if let Some(cut) = chunks::next_cut(&audio[done..now], false) {
                        let started = Instant::now();
                        texts.push(transcribe_piece(&engine, &audio[done..done + cut]).unwrap());
                        busy_until = t + started.elapsed().as_secs_f32();
                        done += cut;
                        cuts.push(format!("{:.1}", done as f32 / rate as f32));
                    }
                }
                now += step;
            }
            // Released: wait for a piece in progress, then the rest.
            let wait = (busy_until - secs).max(0.0);
            let started = Instant::now();
            let mut rest = audio[done..].to_vec();
            while let Some(cut) = chunks::next_cut(&rest, true) {
                texts.push(transcribe_piece(&engine, &rest[..cut]).unwrap());
                rest.drain(..cut);
                done += cut;
                cuts.push(format!("{:.1}*", done as f32 / rate as f32));
            }
            texts.push(transcribe_piece(&engine, &rest).unwrap());
            let after_ms = (wait * 1000.0) as u128 + started.elapsed().as_millis();
            let pieces = chunks::join(&texts);

            let (e, n, changes) = word_diff(&whole, &pieces);
            edits += e;
            total_words += n;
            println!(
                "\n{} ({secs:.1}s) cuts at {:?}\n  whole  {whole_ms:>5} ms  {whole}\n  pieces {after_ms:>5} ms after release (waited {:.0} ms)  {pieces}\n  {e} of {n} words differ {changes:?}",
                file.file_name().unwrap().to_string_lossy(),
                cuts,
                wait * 1000.0,
            );
        }
        println!("\nTotal: {edits} of {total_words} words differ");
    }
}
