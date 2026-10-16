//! Live dictation. While the key is held, the audio is transcribed every
//! few hundred milliseconds so the pill can show the words as you speak.
//! Each time you pause, the phrase so far is locked in and sent to the
//! formatter in the background, so by the time you release the key only
//! the last phrase is left to transcribe and format.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{anyhow, Result};
use transcribe_rs::{SpeechModel, TranscribeOptions};

use crate::audio::{Recording, TARGET_RATE};
use crate::format::{self, rules, Formatter};

pub type Engine = Arc<Mutex<Option<Box<dyn SpeechModel + Send>>>>;
pub type OnUpdate = Arc<dyn Fn(String) + Send + Sync>;

/// How often the live preview refreshes.
const TICK: Duration = Duration::from_millis(350);
/// Quiet this long (seconds) ends a phrase…
const PAUSE_SECS: f32 = 0.45;
/// …but only once the phrase is at least this long (seconds).
const MIN_PHRASE_SECS: f32 = 2.0;
/// Phrases are cut here even without a pause, so no single pass gets slow.
const MAX_PHRASE_SECS: f32 = 20.0;
/// Below this loudness (RMS) the mic counts as quiet.
const QUIET_RMS: f32 = 0.012;
/// Don't bother previewing less audio than this (seconds).
const MIN_PREVIEW_SECS: f32 = 0.5;
/// Whole dictations shorter than this are treated as accidental taps.
const MIN_DICTATION_SECS: f32 = 0.3;

struct Phrase {
    raw: String,
    /// Filled in by the background formatter.
    formatted: Option<String>,
}

#[derive(Default)]
struct Progress {
    phrases: Vec<Phrase>,
    /// Rules-formatted preview of the phrase still being spoken.
    partial: String,
    /// Where the current phrase starts, in device-rate samples.
    phrase_start: usize,
}

impl Progress {
    /// What the pill shows: finished phrases (formatted when ready) plus
    /// the phrase still being spoken.
    fn live_text(&self) -> String {
        let mut parts: Vec<String> = self
            .phrases
            .iter()
            .map(|p| p.formatted.clone().unwrap_or_else(|| rules::apply(&p.raw)))
            .collect();
        if !self.partial.is_empty() {
            parts.push(self.partial.clone());
        }
        format::join_phrases(&parts)
    }
}

pub struct Session {
    recording: Arc<Recording>,
    engine: Engine,
    formatter: Arc<Formatter>,
    smart: bool,
    progress: Arc<Mutex<Progress>>,
    stop: Arc<AtomicBool>,
    preview: Option<JoinHandle<()>>,
    format_tx: Option<mpsc::Sender<(usize, String)>>,
    format_thread: Option<JoinHandle<()>>,
}

impl Session {
    pub fn start(recording: Recording, engine: Engine, formatter: Arc<Formatter>, smart: bool, on_update: OnUpdate) -> Self {
        let recording = Arc::new(recording);
        let progress = Arc::new(Mutex::new(Progress::default()));
        let stop = Arc::new(AtomicBool::new(false));

        // Background formatter: cleans up each finished phrase in order.
        let (format_tx, format_rx) = mpsc::channel::<(usize, String)>();
        let format_thread = {
            let (formatter, progress, on_update) = (formatter.clone(), progress.clone(), on_update.clone());
            thread::spawn(move || {
                for (index, raw) in format_rx {
                    let formatted = formatter.format(&raw, smart);
                    let text = {
                        let mut p = progress.lock().unwrap();
                        p.phrases[index].formatted = Some(formatted);
                        p.live_text()
                    };
                    on_update(text);
                }
            })
        };

        let preview = {
            let (recording, engine, progress, stop) = (recording.clone(), engine.clone(), progress.clone(), stop.clone());
            let format_tx = format_tx.clone();
            thread::spawn(move || preview_loop(&recording, &engine, &progress, &stop, &format_tx, &on_update))
        };

        Self {
            recording,
            engine,
            formatter,
            smart,
            progress,
            stop,
            preview: Some(preview),
            format_tx: Some(format_tx),
            format_thread: Some(format_thread),
        }
    }

    /// Stops recording and returns the final, formatted text.
    pub fn finish(mut self) -> Result<String> {
        self.stop.store(true, Ordering::SeqCst);
        self.recording.stop()?;
        if let Some(preview) = self.preview.take() {
            let _ = preview.join();
        }

        let rate = self.recording.rate() as f32;
        let total = self.recording.len();
        let phrase_start = self.progress.lock().unwrap().phrase_start;

        // Only the last phrase is left to transcribe and format.
        let tail = if (total as f32) / rate < MIN_DICTATION_SECS {
            String::new()
        } else {
            let audio = self.recording.slice_16k(phrase_start, total);
            if audio.len() as f32 / TARGET_RATE as f32 >= MIN_DICTATION_SECS {
                let raw = transcribe(&self.engine, &audio)?;
                if raw.is_empty() { raw } else { self.formatter.format(&raw, self.smart) }
            } else {
                String::new()
            }
        };

        // Let the background formatter finish the earlier phrases.
        drop(self.format_tx.take());
        if let Some(thread) = self.format_thread.take() {
            let _ = thread.join();
        }

        let progress = self.progress.lock().unwrap();
        let mut parts: Vec<String> = progress
            .phrases
            .iter()
            .map(|p| p.formatted.clone().unwrap_or_else(|| rules::apply(&p.raw)))
            .collect();
        parts.push(tail);
        Ok(format::join_phrases(&parts))
    }
}

fn preview_loop(
    recording: &Recording,
    engine: &Engine,
    progress: &Mutex<Progress>,
    stop: &AtomicBool,
    format_tx: &mpsc::Sender<(usize, String)>,
    on_update: &OnUpdate,
) {
    let rate = recording.rate() as f32;
    while !stop.load(Ordering::SeqCst) {
        thread::sleep(TICK);
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let len = recording.len();
        let start = progress.lock().unwrap().phrase_start;
        let secs = len.saturating_sub(start) as f32 / rate;
        if secs < MIN_PREVIEW_SECS {
            continue;
        }

        let quiet = recording.recent_rms((PAUSE_SECS * rate) as usize) < QUIET_RMS;
        let end_phrase = (quiet && secs >= MIN_PHRASE_SECS) || secs >= MAX_PHRASE_SECS;
        let text = match transcribe(engine, &recording.slice_16k(start, len)) {
            Ok(text) => text,
            Err(e) => {
                log::warn!("Live transcription failed: {e}");
                continue;
            }
        };

        let live = {
            let mut p = progress.lock().unwrap();
            if end_phrase {
                if !text.is_empty() {
                    let index = p.phrases.len();
                    p.phrases.push(Phrase { raw: text.clone(), formatted: None });
                    let _ = format_tx.send((index, text));
                }
                p.phrase_start = len;
                p.partial.clear();
            } else if !stop.load(Ordering::SeqCst) {
                p.partial = rules::apply(&text);
            }
            p.live_text()
        };
        on_update(live);
    }
}

fn transcribe(engine: &Engine, audio: &[f32]) -> Result<String> {
    let mut engine = engine.lock().unwrap();
    let model = engine.as_mut().ok_or_else(|| anyhow!("Speech model isn't loaded"))?;
    Ok(model
        .transcribe(audio, &TranscribeOptions::default())
        .map_err(|e| anyhow!("Transcription failed: {e}"))?
        .text
        .trim()
        .to_string())
}
