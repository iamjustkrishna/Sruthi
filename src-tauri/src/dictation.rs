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

pub struct Session {
    recording: Arc<Recording>,
    engine: Engine,
    formatter: Arc<Formatter>,
    smart: bool,
    /// The user's dictionary at the time the key was pressed.
    words: Vec<String>,
    /// The app that had focus when the key was pressed.
    app: AppKind,
    /// Transcribes pieces while the key is held.
    early: Early,
}

impl Session {
    pub fn start(
        recording: Recording,
        engine: Engine,
        formatter: Arc<Formatter>,
        smart: bool,
        words: Vec<String>,
        app: AppKind,
    ) -> Self {
        let recording = Arc::new(recording);
        let early = Early::start(recording.clone(), engine.clone());
        Self {
            recording,
            engine,
            formatter,
            smart,
            words,
            app,
            early,
        }
    }

    /// The app the text is going to.
    pub fn app(&self) -> AppKind {
        self.app
    }

    /// Stops recording, transcribes and formats. Recordings shorter than
    /// an accidental tap come back with no audio.
    pub fn finish(self) -> Result<Outcome> {
        crate::timing::measure("stop", || self.recording.stop())?;
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
        // stopped early, or the model was slow).
        let mut texts = early.texts;
        let mut rest = self.recording.slice_16k(early.done, self.recording.len());
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
                self.formatter
                    .format(&heard, self.smart, &self.words, self.app)
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

/// The background thread that transcribes finished pieces while the key
/// is held.
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
}

impl Early {
    fn start(recording: Arc<Recording>, engine: Engine) -> Self {
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = thread::spawn(move || {
            let mut progress = Progress::default();
            while matches!(stopped.recv_timeout(POLL), Err(RecvTimeoutError::Timeout)) {
                let rest = recording.slice_16k(progress.done, recording.len());
                let Some(cut) = chunks::next_cut(&rest, false) else {
                    continue;
                };
                let started = Instant::now();
                match transcribe_piece(&engine, &rest[..cut]) {
                    Ok(text) => {
                        progress.busy += started.elapsed();
                        progress.done += recording.device_samples(cut);
                        progress.texts.push(text);
                        progress.pieces += 1;
                    }
                    Err(e) => {
                        // The rest is transcribed after release instead.
                        log::warn!("Couldn't transcribe while recording: {e:#}");
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
    /// (SRUTAM_TEST_MODEL=hybrid picks the engine; parakeet-v3 by default.)
    #[test]
    #[ignore]
    fn pieces_match_whole() {
        let _ = env_logger::builder()
            .is_test(true)
            .filter_level(log::LevelFilter::Warn)
            .try_init();
        let id = std::env::var("SRUTAM_TEST_MODEL").unwrap_or_else(|_| "parakeet-v3".into());
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
