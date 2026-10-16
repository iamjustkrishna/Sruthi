//! One dictation: record while the key is held, then transcribe the whole
//! recording in a single pass and format it.

use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use transcribe_rs::{SpeechModel, TranscribeOptions};

use crate::audio::{Recording, TARGET_RATE};
use crate::format::Formatter;

pub type Engine = Arc<Mutex<Option<Box<dyn SpeechModel + Send>>>>;

/// Dictations shorter than this (seconds) are treated as accidental taps.
const MIN_DICTATION_SECS: f32 = 0.3;

pub struct Session {
    recording: Recording,
    engine: Engine,
    formatter: Arc<Formatter>,
    smart: bool,
}

impl Session {
    pub fn start(recording: Recording, engine: Engine, formatter: Arc<Formatter>, smart: bool) -> Self {
        Self { recording, engine, formatter, smart }
    }

    /// Stops recording and returns the final, formatted text.
    pub fn finish(self) -> Result<String> {
        self.recording.stop()?;
        let audio = self.recording.slice_16k(0, self.recording.len());
        if (audio.len() as f32) / (TARGET_RATE as f32) < MIN_DICTATION_SECS {
            return Ok(String::new());
        }

        let raw = {
            let mut engine = self.engine.lock().unwrap();
            let model = engine.as_mut().ok_or_else(|| anyhow!("Speech model isn't loaded"))?;
            model
                .transcribe(&audio, &TranscribeOptions::default())
                .map_err(|e| anyhow!("Transcription failed: {e}"))?
                .text
                .trim()
                .to_string()
        };
        if raw.is_empty() {
            return Ok(raw);
        }
        let formatted = self.formatter.format(&raw, self.smart);
        // Local debug log only (stdout in dev builds); helps tune the rules.
        log::info!("Heard:  {raw:?}");
        log::info!("Pasted: {formatted:?}");
        Ok(formatted)
    }
}
