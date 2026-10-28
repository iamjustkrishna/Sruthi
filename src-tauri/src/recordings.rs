//! Keeps every dictation: its audio as a WAV file, plus what the model
//! heard and what was pasted, so Recent dictations can replay them.
//! Everything stays in the app's folder on this PC.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::audio::TARGET_RATE;

/// History entries kept in the index (the WAV files are what take space).
const MAX_ENTRIES: usize = 1000;

#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    /// When it was recorded, in milliseconds since 1970.
    pub time_ms: u64,
    pub seconds: f32,
    /// What the speech model heard, before formatting.
    pub heard: String,
    /// What was pasted (empty when nothing was heard).
    pub text: String,
    /// Process name of the app the text went into (empty for older entries).
    #[serde(default)]
    pub app: String,
}

pub fn folder() -> PathBuf {
    crate::settings::app_root().join("recordings")
}

fn index_path() -> PathBuf {
    folder().join("history.json")
}

fn wav_path(id: &str) -> Result<PathBuf> {
    // Ids are generated here; reject anything that could escape the folder.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        bail!("Invalid recording id");
    }
    Ok(folder().join(format!("{id}.wav")))
}

/// All entries, newest first.
pub fn load() -> Vec<Entry> {
    fs::read_to_string(index_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_index(entries: &[Entry]) -> Result<()> {
    fs::create_dir_all(folder())?;
    fs::write(index_path(), serde_json::to_string_pretty(entries)?)?;
    Ok(())
}

/// Saves 16 kHz mono `samples` as a WAV and records the entry.
pub fn add(samples: &[f32], heard: &str, text: &str, app: &str) -> Result<Entry> {
    let time_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let entry = Entry {
        id: format!("dictation-{time_ms}"),
        time_ms,
        seconds: samples.len() as f32 / TARGET_RATE as f32,
        heard: heard.to_string(),
        text: text.to_string(),
        app: app.to_string(),
    };
    fs::create_dir_all(folder())?;
    fs::write(wav_path(&entry.id)?, encode_wav(samples))?;

    let mut entries = load();
    entries.insert(0, entry.clone());
    entries.truncate(MAX_ENTRIES);
    save_index(&entries)?;
    Ok(entry)
}

pub fn read_wav(id: &str) -> Result<Vec<u8>> {
    Ok(fs::read(wav_path(id)?)?)
}

/// Deletes one recording and its history entry.
pub fn remove(id: &str) -> Result<()> {
    let path = wav_path(id)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    let mut entries = load();
    entries.retain(|e| e.id != id);
    save_index(&entries)
}

/// 16-bit PCM WAV, mono, 16 kHz.
pub(crate) fn encode_wav(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&TARGET_RATE.to_le_bytes());
    out.extend_from_slice(&(TARGET_RATE * 2).to_le_bytes()); // bytes per second
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_and_length() {
        let wav = encode_wav(&[0.0, 0.5, -0.5, 1.0]);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(wav.len(), 44 + 8);
        assert_eq!(i16::from_le_bytes([wav[50], wav[51]]), i16::MAX);
    }

    #[test]
    fn rejects_ids_that_leave_the_folder() {
        assert!(wav_path("../settings").is_err());
        assert!(wav_path("a/b").is_err());
        assert!(wav_path("").is_err());
        assert!(wav_path("dictation-1792000000000").is_ok());
    }
}
