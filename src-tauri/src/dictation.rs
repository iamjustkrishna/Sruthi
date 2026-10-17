//! One dictation: record while the key is held, then transcribe the whole
//! recording in a single pass and format it.

use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use transcribe_rs::{SpeechModel, TranscribeOptions};

use crate::audio::{Recording, TARGET_RATE};
use crate::context::AppKind;
use crate::enhance;
use crate::format::Formatter;

pub type Engine = Arc<Mutex<Option<Box<dyn SpeechModel + Send>>>>;

/// Dictations shorter than this (seconds) are treated as accidental taps.
const MIN_DICTATION_SECS: f32 = 0.3;

pub struct Session {
    recording: Recording,
    engine: Engine,
    formatter: Arc<Formatter>,
    smart: bool,
    /// The user's dictionary at the time the key was pressed.
    words: Vec<String>,
    /// The app that had focus when the key was pressed.
    app: AppKind,
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
        Self { recording, engine, formatter, smart, words, app }
    }

    /// Stops recording, transcribes and formats. Recordings shorter than
    /// an accidental tap come back with no audio.
    pub fn finish(self) -> Result<Outcome> {
        self.recording.stop()?;
        let device = self.recording.device().to_string();
        let audio = self.recording.slice_16k(0, self.recording.len());
        if (audio.len() as f32) / (TARGET_RATE as f32) < MIN_DICTATION_SECS {
            return Ok(Outcome { device, ..Outcome::default() });
        }
        // Digital silence: the microphone itself delivers nothing.
        if crate::audio::rms(&audio) < crate::audio::DEAD_MIC_RMS {
            return Ok(Outcome { device, dead_mic: true, ..Outcome::default() });
        }
        // Boost quiet speech; skip the model entirely when nothing was said.
        let Some(prepared) = enhance::prepare(&audio) else {
            log::info!("No speech in recording");
            return Ok(Outcome { audio, device, ..Outcome::default() });
        };

        let heard = {
            let mut engine = self.engine.lock().unwrap();
            let model = engine.as_mut().ok_or_else(|| anyhow!("Speech model isn't loaded"))?;
            model
                .transcribe(&prepared, &TranscribeOptions::default())
                .map_err(|e| anyhow!("Transcription failed: {e}"))?
                .text
                .trim()
                .to_string()
        };
        let text = if heard.is_empty() { String::new() } else { self.formatter.format(&heard, self.smart, &self.words, self.app) };
        // Local debug log only (stdout in dev builds); helps tune the rules.
        log::info!("Heard:  {heard:?}");
        log::info!("Pasted: {text:?}");
        Ok(Outcome { audio, heard, text, device, dead_mic: false })
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
