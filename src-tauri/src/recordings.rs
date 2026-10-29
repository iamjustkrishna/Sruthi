//! Keeps every dictation: its audio as a WAV file, plus what the model
//! heard and what was pasted, so Recent dictations can replay them.
//! Everything stays in the app's folder on this PC.
//!
//! The words and the audio age differently. The words (a few hundred bytes
//! each) are kept for the newest `MAX_ENTRIES` dictations; the audio (about
//! 0.7 MB each) can be deleted, zipped or moved, by hand or by the keep-audio
//! setting, and the words stay.

use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::audio::TARGET_RATE;

/// Dictations whose words are kept in the index.
const MAX_ENTRIES: usize = 5000;
/// Newest dictations whose audio is kept when no shorter limit is set.
const MAX_AUDIO: usize = 1000;
const DAY_MS: u64 = 86_400_000;

fn yes() -> bool {
    true
}

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
    /// What you actually said, as corrected by you; the benchmark's answer
    /// key.
    #[serde(default)]
    pub reference: Option<String>,
    /// False once the audio was deleted; the words stay.
    #[serde(default = "yes")]
    pub audio: bool,
    /// Where the audio is when it is not in the recordings folder: a `.zip`
    /// (archived) or a folder (moved).
    #[serde(default)]
    pub stored: Option<String>,
}

#[cfg(test)]
thread_local! {
    /// Tests point the store at a throwaway folder instead of the real one.
    static TEST_FOLDER: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

pub fn folder() -> PathBuf {
    #[cfg(test)]
    if let Some(dir) = TEST_FOLDER.with(|f| f.borrow().clone()) {
        return dir;
    }
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

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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
    // Written beside the index and renamed over it, so a crash mid-write can't
    // leave a half-written history.
    let tmp = folder().join("history.json.part");
    fs::write(&tmp, serde_json::to_string(entries)?)?;
    fs::rename(&tmp, index_path())?;
    Ok(())
}

/// Saves 16 kHz mono `samples` as a WAV and records the entry.
pub fn add(samples: &[f32], heard: &str, text: &str, app: &str) -> Result<Entry> {
    add_with(samples, heard, text, app, crate::settings::load().keep_audio_days)
}

fn add_with(
    samples: &[f32],
    heard: &str,
    text: &str,
    app: &str,
    keep_days: Option<u32>,
) -> Result<Entry> {
    let time_ms = now_ms();
    // "Don't keep audio" (0 days) records the words only.
    let keep_audio = keep_days != Some(0);
    let entry = Entry {
        id: format!("dictation-{time_ms}"),
        time_ms,
        seconds: samples.len() as f32 / TARGET_RATE as f32,
        heard: heard.to_string(),
        text: text.to_string(),
        app: app.to_string(),
        reference: None,
        audio: keep_audio,
        stored: None,
    };
    fs::create_dir_all(folder())?;
    if keep_audio {
        fs::write(wav_path(&entry.id)?, encode_wav(samples))?;
    }

    let mut entries = load();
    entries.insert(0, entry.clone());
    enforce(&mut entries, keep_days);
    save_index(&entries)?;
    Ok(entry)
}

/// Applies the limits to `entries` (newest first), deleting the audio files
/// that fall outside them: words past `MAX_ENTRIES` go entirely; audio past
/// the newest `MAX_AUDIO`, or older than `keep_days`, goes but the words stay.
/// Corrected dictations keep their audio (the benchmark needs it) and so does
/// anything archived or moved. Returns how many audio files were removed.
fn enforce(entries: &mut Vec<Entry>, keep_days: Option<u32>) -> usize {
    let mut removed = 0;
    if entries.len() > MAX_ENTRIES {
        for gone in entries.drain(MAX_ENTRIES..) {
            if gone.stored.is_none() && delete_wav(&gone.id) {
                removed += 1;
            }
        }
    }
    let cutoff = match keep_days {
        Some(days) if days > 0 => Some(now_ms().saturating_sub(days as u64 * DAY_MS)),
        _ => None,
    };
    let mut with_audio = 0;
    for e in entries.iter_mut() {
        if !e.audio || e.stored.is_some() {
            continue;
        }
        with_audio += 1;
        let too_many = with_audio > MAX_AUDIO;
        let too_old = cutoff.is_some_and(|c| e.time_ms < c);
        let keep_days_zero = keep_days == Some(0);
        if (too_many || too_old || keep_days_zero) && e.reference.is_none() {
            if delete_wav(&e.id) {
                removed += 1;
            }
            e.audio = false;
            with_audio -= 1;
        }
    }
    removed
}

/// Removes one local WAV file; true when there was one to remove.
fn delete_wav(id: &str) -> bool {
    wav_path(id)
        .ok()
        .is_some_and(|p| fs::remove_file(p).is_ok())
}

/// Applies the keep-audio setting to what is already stored (at startup and
/// when the setting changes). Returns how many audio files were removed.
pub fn apply_retention(keep_days: Option<u32>) -> Result<usize> {
    let mut entries = load();
    let removed = enforce(&mut entries, keep_days);
    save_index(&entries)?;
    Ok(removed)
}

/// Sets (or clears) the corrected text of one dictation.
pub fn set_reference(id: &str, reference: Option<String>) -> Result<()> {
    let mut entries = load();
    let Some(entry) = entries.iter_mut().find(|e| e.id == id) else {
        bail!("That dictation is gone");
    };
    entry.reference = reference;
    save_index(&entries)
}

/// The 16 kHz samples of one recording.
pub fn read_samples(id: &str) -> Result<Vec<f32>> {
    let path = audio_file(id)?;
    transcribe_rs::audio::read_wav_samples(&path).map_err(|e| anyhow::anyhow!("{e}"))
}

pub fn read_wav(id: &str) -> Result<Vec<u8>> {
    Ok(fs::read(audio_file(id)?)?)
}

/// Where a recording's WAV can be read from right now, or why it can't be.
fn audio_file(id: &str) -> Result<PathBuf> {
    let local = wav_path(id)?;
    if local.exists() {
        return Ok(local);
    }
    let entry = load().into_iter().find(|e| e.id == id);
    match entry {
        Some(e) if !e.audio => bail!("The audio for this dictation was deleted"),
        Some(e) => match e.stored {
            Some(place) if place.to_lowercase().ends_with(".zip") => {
                bail!("This audio is in a zip file. Restore it in Settings to play it")
            }
            Some(place) => {
                let moved = Path::new(&place).join(format!("{id}.wav"));
                if moved.exists() {
                    Ok(moved)
                } else {
                    bail!("This audio was moved to {place}, which is not available right now")
                }
            }
            None => bail!("The audio file is missing"),
        },
        None => bail!("That dictation is gone"),
    }
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

// ---------------------------------------------------------------------------
// Reaching every day

/// One day of history: how many dictations and words.
#[derive(Serialize, Debug, PartialEq)]
pub struct DaySummary {
    /// Local calendar day, "2026-10-22".
    pub date: String,
    pub count: u32,
    pub words: u32,
}

/// Every day that has dictations, newest first.
pub fn days() -> Vec<DaySummary> {
    days_of(&load())
}

fn days_of(entries: &[Entry]) -> Vec<DaySummary> {
    let mut out: Vec<DaySummary> = Vec::new();
    for e in entries {
        let date = crate::stats::local_day_key(e.time_ms);
        let words = e.text.split_whitespace().count() as u32;
        match out.last_mut() {
            Some(day) if day.date == date => {
                day.count += 1;
                day.words += words;
            }
            _ => out.push(DaySummary {
                date,
                count: 1,
                words,
            }),
        }
    }
    out
}

/// A request for some of the history.
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct PageQuery {
    /// Only dictations older than this (milliseconds), for "show more".
    pub before_ms: Option<u64>,
    /// Only dictations newer than this, for "Last 7 days".
    pub since_ms: Option<u64>,
    /// Only this local day, "2026-10-22".
    pub day: Option<String>,
    /// Only dictations whose words, raw words or app contain this.
    pub query: Option<String>,
    pub limit: usize,
}

#[derive(Serialize)]
pub struct Page {
    pub entries: Vec<Entry>,
    /// More matches exist beyond this page.
    pub more: bool,
    /// All matches, across every page.
    pub matches: usize,
    /// How many different days those matches fall on.
    pub days_matched: usize,
    /// How many dictations were searched.
    pub searched: usize,
}

pub fn page(q: &PageQuery) -> Page {
    page_of(load(), q)
}

fn page_of(entries: Vec<Entry>, q: &PageQuery) -> Page {
    let needle = q
        .query
        .as_deref()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());
    let searched = entries.len();
    let limit = if q.limit == 0 { 50 } else { q.limit.min(500) };
    let mut matching: Vec<Entry> = entries
        .into_iter()
        .filter(|e| q.since_ms.is_none_or(|s| e.time_ms >= s))
        .filter(|e| {
            q.day
                .as_deref()
                .is_none_or(|d| crate::stats::local_day_key(e.time_ms) == d)
        })
        .filter(|e| {
            needle.as_deref().is_none_or(|n| {
                e.text.to_lowercase().contains(n)
                    || e.heard.to_lowercase().contains(n)
                    || e.app.to_lowercase().contains(n)
            })
        })
        .collect();
    let matches = matching.len();
    let days_matched = days_of(&matching).len();
    // "Show more" pages from the oldest dictation already on screen.
    if let Some(before) = q.before_ms {
        matching.retain(|e| e.time_ms < before);
    }
    let more = matching.len() > limit;
    matching.truncate(limit);
    Page {
        entries: matching,
        more,
        matches,
        days_matched,
        searched,
    }
}

// ---------------------------------------------------------------------------
// Audio on disk: how much, and moving it out of the way

/// A place audio was moved or zipped to.
#[derive(Serialize)]
pub struct Archive {
    pub path: String,
    /// "zip" or "folder".
    pub kind: &'static str,
    pub count: usize,
    pub bytes: u64,
    /// Still there (a drive may be unplugged).
    pub present: bool,
    pub from_ms: u64,
    pub to_ms: u64,
}

#[derive(Serialize)]
pub struct AudioUsage {
    /// Size of the audio in the recordings folder.
    pub bytes: u64,
    /// Audio files in the recordings folder.
    pub recordings: usize,
    /// Dictations in the history.
    pub entries: usize,
    /// Days they cover.
    pub days: usize,
    pub archives: Vec<Archive>,
}

pub fn audio_usage() -> AudioUsage {
    let entries = load();
    let (mut bytes, mut recordings) = (0u64, 0usize);
    if let Ok(dir) = fs::read_dir(folder()) {
        for f in dir.flatten() {
            if f.path().extension().is_some_and(|e| e == "wav") {
                recordings += 1;
                bytes += f.metadata().map_or(0, |m| m.len());
            }
        }
    }
    let mut archives: Vec<Archive> = Vec::new();
    for e in entries.iter().filter(|e| e.audio) {
        let Some(place) = &e.stored else { continue };
        match archives.iter_mut().find(|a| &a.path == place) {
            Some(a) => {
                a.count += 1;
                a.from_ms = a.from_ms.min(e.time_ms);
                a.to_ms = a.to_ms.max(e.time_ms);
            }
            None => {
                let zip = place.to_lowercase().ends_with(".zip");
                let meta = fs::metadata(place).ok();
                archives.push(Archive {
                    path: place.clone(),
                    kind: if zip { "zip" } else { "folder" },
                    count: 1,
                    bytes: if zip { meta.as_ref().map_or(0, |m| m.len()) } else { 0 },
                    present: meta.is_some(),
                    from_ms: e.time_ms,
                    to_ms: e.time_ms,
                });
            }
        }
    }
    for a in archives.iter_mut().filter(|a| a.kind == "folder") {
        a.bytes = entries
            .iter()
            .filter(|e| e.stored.as_deref() == Some(a.path.as_str()))
            .filter_map(|e| fs::metadata(Path::new(&a.path).join(format!("{}.wav", e.id))).ok())
            .map(|m| m.len())
            .sum();
    }
    archives.sort_by_key(|a| std::cmp::Reverse(a.to_ms));
    AudioUsage {
        bytes,
        recordings,
        entries: entries.len(),
        days: days_of(&entries).len(),
        archives,
    }
}

/// Some recordings and what they take up.
#[derive(Serialize, Debug, PartialEq, Default, Clone, Copy)]
pub struct Candidates {
    pub count: usize,
    pub bytes: u64,
    /// Corrected dictations that were left alone (the benchmark needs them).
    pub kept: usize,
}

/// Local audio older than `days` (0: all of it): the ids and their files.
/// Corrected dictations are never included.
fn candidate_files(entries: &[Entry], days: u32) -> (Vec<(String, PathBuf, u64)>, usize) {
    let cutoff = now_ms().saturating_sub(days as u64 * DAY_MS);
    let mut kept = 0;
    let mut out = Vec::new();
    for e in entries {
        if !e.audio || e.stored.is_some() || (days > 0 && e.time_ms >= cutoff) {
            continue;
        }
        let Ok(path) = wav_path(&e.id) else { continue };
        let Ok(meta) = fs::metadata(&path) else { continue };
        if e.reference.is_some() {
            kept += 1;
            continue;
        }
        out.push((e.id.clone(), path, meta.len()));
    }
    (out, kept)
}

pub fn candidates(older_than_days: u32) -> Candidates {
    let (files, kept) = candidate_files(&load(), older_than_days);
    Candidates {
        count: files.len(),
        bytes: files.iter().map(|f| f.2).sum(),
        kept,
    }
}

/// Deletes the audio older than `days` (0: all of it). The words stay.
pub fn delete_audio(older_than_days: u32) -> Result<Candidates> {
    let mut entries = load();
    let (files, kept) = candidate_files(&entries, older_than_days);
    let mut done = Candidates {
        kept,
        ..Candidates::default()
    };
    for (id, path, bytes) in &files {
        if fs::remove_file(path).is_ok() {
            done.count += 1;
            done.bytes += bytes;
            if let Some(e) = entries.iter_mut().find(|e| &e.id == id) {
                e.audio = false;
            }
        }
    }
    save_index(&entries)?;
    Ok(done)
}

fn local_name_day(ms: u64) -> String {
    crate::stats::local_day_key(ms)
}

/// What a zip or a move did.
#[derive(Serialize, Debug)]
pub struct Archived {
    pub count: usize,
    /// Space the audio took before.
    pub bytes_before: u64,
    /// Where it is now (the zip file, or the folder).
    pub path: String,
    /// The zip's size (equal to `bytes_before` for a move).
    pub bytes_after: u64,
}

/// Packs the audio older than `days` into one zip in `dest_dir`, checks the
/// zip by reading every file back, and only then removes the originals.
pub fn zip_audio(older_than_days: u32, dest_dir: &Path) -> Result<Archived> {
    let mut entries = load();
    let (files, _) = candidate_files(&entries, older_than_days);
    if files.is_empty() {
        bail!("There is no audio that old to zip");
    }
    fs::create_dir_all(dest_dir).context("Couldn't create the folder for the zip")?;
    let times: Vec<u64> = entries
        .iter()
        .filter(|e| files.iter().any(|f| f.0 == e.id))
        .map(|e| e.time_ms)
        .collect();
    let (first, last) = (
        times.iter().copied().min().unwrap_or(0),
        times.iter().copied().max().unwrap_or(0),
    );
    let base = format!(
        "sruthi-audio-{}-to-{}",
        local_name_day(first),
        local_name_day(last)
    );
    let mut zip_path = dest_dir.join(format!("{base}.zip"));
    let mut n = 2;
    while zip_path.exists() {
        zip_path = dest_dir.join(format!("{base}-{n}.zip"));
        n += 1;
    }
    let part = zip_path.with_extension("zip.part");
    let written = (|| -> Result<()> {
        let mut zip = zip::ZipWriter::new(BufWriter::new(File::create(&part)?));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (id, path, _) in &files {
            zip.start_file(format!("{id}.wav"), opts)?;
            io::copy(&mut BufReader::new(File::open(path)?), &mut zip)?;
        }
        zip.finish()?.flush()?;
        Ok(())
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&part);
        return Err(e.context("Couldn't write the zip"));
    }
    fs::rename(&part, &zip_path)?;
    // Verify before anything is deleted: every file must read back whole.
    verify_zip(&zip_path, &files).inspect_err(|_| {
        let _ = fs::remove_file(&zip_path);
    })?;

    let place = zip_path.display().to_string();
    let mut bytes_before = 0;
    for (id, path, bytes) in &files {
        if fs::remove_file(path).is_ok() {
            bytes_before += bytes;
            if let Some(e) = entries.iter_mut().find(|e| &e.id == id) {
                e.stored = Some(place.clone());
            }
        }
    }
    save_index(&entries)?;
    Ok(Archived {
        count: files.len(),
        bytes_before,
        bytes_after: fs::metadata(&zip_path).map_or(0, |m| m.len()),
        path: place,
    })
}

fn verify_zip(zip_path: &Path, files: &[(String, PathBuf, u64)]) -> Result<()> {
    let mut archive = zip::ZipArchive::new(BufReader::new(File::open(zip_path)?))
        .context("The zip could not be reopened")?;
    if archive.len() != files.len() {
        bail!("The zip is missing recordings");
    }
    for (id, _, bytes) in files {
        let mut f = archive
            .by_name(&format!("{id}.wav"))
            .with_context(|| format!("{id} is not in the zip"))?;
        // Reading to the end checks the stored checksum.
        let read = io::copy(&mut f, &mut io::sink()).context("A recording in the zip is damaged")?;
        if read != *bytes {
            bail!("A recording in the zip has the wrong size");
        }
    }
    Ok(())
}

/// Copies the audio older than `days` to `dest_dir`, checks every copy, and
/// only then removes the originals. Moved audio plays while the folder is there.
pub fn move_audio(older_than_days: u32, dest_dir: &Path) -> Result<Archived> {
    let mut entries = load();
    let (files, _) = candidate_files(&entries, older_than_days);
    if files.is_empty() {
        bail!("There is no audio that old to move");
    }
    fs::create_dir_all(dest_dir).context("Couldn't create that folder")?;
    let mut copied: Vec<PathBuf> = Vec::new();
    let result = (|| -> Result<()> {
        for (id, path, bytes) in &files {
            let target = dest_dir.join(format!("{id}.wav"));
            let part = dest_dir.join(format!("{id}.wav.part"));
            fs::copy(path, &part).with_context(|| format!("Couldn't copy {id}"))?;
            fs::rename(&part, &target)?;
            copied.push(target.clone());
            if fs::metadata(&target)?.len() != *bytes {
                bail!("A copy came out the wrong size");
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        for c in &copied {
            let _ = fs::remove_file(c);
        }
        return Err(e);
    }
    let place = dest_dir.display().to_string();
    let mut bytes_before = 0;
    for (id, path, bytes) in &files {
        if fs::remove_file(path).is_ok() {
            bytes_before += bytes;
            if let Some(e) = entries.iter_mut().find(|e| &e.id == id) {
                e.stored = Some(place.clone());
            }
        }
    }
    save_index(&entries)?;
    Ok(Archived {
        count: files.len(),
        bytes_before,
        bytes_after: bytes_before,
        path: place,
    })
}

/// Brings audio kept at `location` (a zip, or a folder it was moved to) back
/// into the recordings folder. Returns how many recordings were restored.
pub fn restore(location: &str) -> Result<usize> {
    let mut entries = load();
    let ids: Vec<String> = entries
        .iter()
        .filter(|e| e.stored.as_deref() == Some(location))
        .map(|e| e.id.clone())
        .collect();
    if ids.is_empty() {
        bail!("Nothing is kept there any more");
    }
    fs::create_dir_all(folder())?;
    let is_zip = location.to_lowercase().ends_with(".zip");
    let mut restored: Vec<String> = Vec::new();
    for id in &ids {
        let target = wav_path(id)?;
        let part = folder().join(format!("{id}.wav.part"));
        let ok = (|| -> Result<()> {
            if is_zip {
                let mut archive = zip::ZipArchive::new(BufReader::new(
                    File::open(location).context("The zip file is not there")?,
                ))?;
                let mut f = archive.by_name(&format!("{id}.wav"))?;
                io::copy(&mut f, &mut BufWriter::new(File::create(&part)?))?;
            } else {
                fs::copy(Path::new(location).join(format!("{id}.wav")), &part)
                    .context("The folder is not available")?;
            }
            fs::rename(&part, &target)?;
            Ok(())
        })();
        match ok {
            Ok(()) => restored.push(id.clone()),
            Err(e) => {
                let _ = fs::remove_file(&part);
                if restored.is_empty() {
                    return Err(e);
                }
                break;
            }
        }
    }
    for e in entries.iter_mut().filter(|e| restored.contains(&e.id)) {
        e.stored = None;
    }
    save_index(&entries)?;
    // A moved folder gives its files up; a zip is left for the person to delete.
    if !is_zip {
        for id in &restored {
            let _ = fs::remove_file(Path::new(location).join(format!("{id}.wav")));
        }
    }
    Ok(restored.len())
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

    /// Points the store at a fresh temp folder for the current test thread.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sruthi-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        TEST_FOLDER.with(|f| *f.borrow_mut() = Some(dir.join("recordings")));
        dir
    }

    fn entry(id: &str, time_ms: u64, text: &str) -> Entry {
        Entry {
            id: id.into(),
            time_ms,
            seconds: 1.0,
            heard: text.to_lowercase(),
            text: text.into(),
            app: "chrome.exe".into(),
            reference: None,
            audio: true,
            stored: None,
        }
    }

    /// A WAV of `n` samples of a rough tone, so it compresses a little.
    fn write_wav(id: &str, n: usize) {
        let samples: Vec<f32> = (0..n).map(|i| ((i % 50) as f32 / 50.0) - 0.5).collect();
        fs::create_dir_all(folder()).unwrap();
        fs::write(wav_path(id).unwrap(), encode_wav(&samples)).unwrap();
    }

    /// Three dictations: two old (10 and 9 days ago), one from now.
    fn seed() {
        let now = now_ms();
        let entries = vec![
            entry("dictation-3", now, "Fresh words about Kubernetes"),
            entry("dictation-2", now - 9 * DAY_MS, "Nine days old"),
            entry("dictation-1", now - 10 * DAY_MS, "Ten days old, also Kubernetes"),
        ];
        for e in &entries {
            write_wav(&e.id, 4000);
        }
        save_index(&entries).unwrap();
    }

    fn by_id(id: &str) -> Entry {
        load().into_iter().find(|e| e.id == id).unwrap()
    }

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

    #[test]
    fn days_group_by_local_day_with_counts_and_words() {
        let base = 1_781_524_830_000; // mid-June 2026, UTC noon-ish
        let entries = vec![
            entry("a", base + 1_000, "one two three"),
            entry("b", base, "four five"),
            entry("c", base - 3 * DAY_MS, "six"),
        ];
        let days = days_of(&entries);
        assert_eq!(days.len(), 2);
        assert_eq!((days[0].count, days[0].words), (2, 5));
        assert_eq!((days[1].count, days[1].words), (1, 1));
    }

    #[test]
    fn a_page_searches_every_entry_and_pages_from_the_oldest_shown() {
        let entries: Vec<Entry> = (0..10)
            .rev()
            .map(|i| entry(&format!("id-{i}"), 1_000_000 + i * 1_000, &format!("word {i}")))
            .collect();
        let first = page_of(
            entries.clone(),
            &PageQuery {
                limit: 4,
                ..PageQuery::default()
            },
        );
        assert_eq!(first.entries.len(), 4);
        assert!(first.more);
        assert_eq!(first.matches, 10);
        let oldest_shown = first.entries.last().unwrap().time_ms;
        let next = page_of(
            entries.clone(),
            &PageQuery {
                before_ms: Some(oldest_shown),
                limit: 4,
                ..PageQuery::default()
            },
        );
        assert_eq!(next.entries[0].id, "id-5");
        let found = page_of(
            entries,
            &PageQuery {
                query: Some(" WORD 7 ".into()),
                limit: 10,
                ..PageQuery::default()
            },
        );
        assert_eq!(found.entries.len(), 1);
        assert_eq!((found.matches, found.searched), (1, 10));
    }

    #[test]
    fn a_page_can_be_narrowed_to_a_day_or_a_recent_window() {
        let now = now_ms();
        let entries = vec![
            entry("new", now, "today"),
            entry("old", now - 9 * DAY_MS, "last week"),
        ];
        let recent = page_of(
            entries.clone(),
            &PageQuery {
                since_ms: Some(now - 7 * DAY_MS),
                limit: 10,
                ..PageQuery::default()
            },
        );
        assert_eq!(recent.entries.len(), 1);
        let day = crate::stats::local_day_key(now - 9 * DAY_MS);
        let that_day = page_of(
            entries,
            &PageQuery {
                day: Some(day),
                limit: 10,
                ..PageQuery::default()
            },
        );
        assert_eq!(that_day.entries[0].id, "old");
    }

    #[test]
    fn deleting_old_audio_keeps_the_words_and_spares_corrected_dictations() {
        let _dir = scratch("delete");
        seed();
        let mut entries = load();
        entries.iter_mut().find(|e| e.id == "dictation-1").unwrap().reference = Some("fixed".into());
        save_index(&entries).unwrap();

        let plan = candidates(7);
        assert_eq!((plan.count, plan.kept), (1, 1));
        let done = delete_audio(7).unwrap();
        assert_eq!(done.count, 1);
        assert!(!by_id("dictation-2").audio);
        assert!(!wav_path("dictation-2").unwrap().exists());
        // The corrected one and today's keep their audio; every word is still listed.
        assert!(by_id("dictation-1").audio && wav_path("dictation-1").unwrap().exists());
        assert!(by_id("dictation-3").audio);
        assert_eq!(load().len(), 3);
        assert!(read_wav("dictation-2").unwrap_err().to_string().contains("deleted"));
    }

    #[test]
    fn zipping_verifies_then_removes_and_restoring_brings_it_back() {
        let dir = scratch("zip");
        seed();
        let before = fs::read(wav_path("dictation-2").unwrap()).unwrap();
        let out = zip_audio(7, &dir.join("zips")).unwrap();
        assert_eq!(out.count, 2);
        assert!(out.bytes_after < out.bytes_before);
        assert!(Path::new(&out.path).exists());
        assert!(!wav_path("dictation-2").unwrap().exists());
        assert!(wav_path("dictation-3").unwrap().exists(), "today's audio is not zipped");
        assert_eq!(by_id("dictation-2").stored.as_deref(), Some(out.path.as_str()));
        assert!(read_wav("dictation-2").unwrap_err().to_string().contains("zip"));

        let usage = audio_usage();
        assert_eq!(usage.archives.len(), 1);
        assert_eq!((usage.archives[0].kind, usage.archives[0].count), ("zip", 2));

        assert_eq!(restore(&out.path).unwrap(), 2);
        assert_eq!(fs::read(wav_path("dictation-2").unwrap()).unwrap(), before);
        assert!(by_id("dictation-2").stored.is_none());
    }

    #[test]
    fn moving_audio_keeps_it_playable_from_the_new_folder() {
        let dir = scratch("move");
        seed();
        let dest = dir.join("drive");
        let out = move_audio(7, &dest).unwrap();
        assert_eq!(out.count, 2);
        assert!(!wav_path("dictation-1").unwrap().exists());
        assert!(dest.join("dictation-1.wav").exists());
        assert!(read_wav("dictation-1").is_ok(), "plays from where it was moved");

        assert_eq!(restore(&out.path).unwrap(), 2);
        assert!(wav_path("dictation-1").unwrap().exists());
        assert!(!dest.join("dictation-1.wav").exists());
    }

    #[test]
    fn keeping_audio_for_a_while_drops_only_older_audio() {
        let _dir = scratch("retention");
        seed();
        assert_eq!(apply_retention(Some(30)).unwrap(), 0);
        assert_eq!(apply_retention(Some(7)).unwrap(), 2);
        assert!(by_id("dictation-3").audio);
        assert!(!by_id("dictation-1").audio);
        // Words are untouched.
        assert_eq!(load().len(), 3);
    }

    #[test]
    fn no_audio_mode_records_words_only() {
        let _dir = scratch("noaudio");
        let e = add_with(&[0.1; 1600], "hello", "Hello.", "chrome.exe", Some(0)).unwrap();
        assert!(!e.audio);
        assert!(!wav_path(&e.id).unwrap().exists());
        assert_eq!(load().len(), 1);
    }

    #[test]
    fn the_oldest_audio_goes_when_more_than_the_audio_cap_exists() {
        let _dir = scratch("cap");
        let now = now_ms();
        let mut entries: Vec<Entry> = (0..(MAX_AUDIO + 2))
            .map(|i| entry(&format!("d-{i}"), now - i as u64 * 1_000, "w"))
            .collect();
        for e in entries.iter().rev().take(3) {
            write_wav(&e.id, 100);
        }
        let removed = enforce(&mut entries, None);
        assert_eq!(removed, 2, "only files that exist count");
        assert!(!entries[MAX_AUDIO].audio && !entries[MAX_AUDIO + 1].audio);
        assert!(entries[MAX_AUDIO - 1].audio);
    }
}
