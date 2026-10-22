//! Speech model catalog: what can be installed, where it lives, whether it's
//! installed, and a resumable downloader that survives stalled connections.

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use transcribe_rs::onnx::moonshine::{MoonshineModel, MoonshineVariant, StreamingModel};
use transcribe_rs::onnx::parakeet::ParakeetModel;
use transcribe_rs::onnx::Quantization;
use transcribe_rs::SpeechModel;

use crate::whisper::{self, WhisperKind};

pub const DEFAULT_MODEL: &str = "parakeet-v3";

/// A read that returns nothing for this long counts as a stalled connection.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ATTEMPTS: u32 = 6;

#[derive(Clone, Copy)]
enum Engine {
    Parakeet,
    MoonshineBase,
    MoonshineStreaming,
    /// whisper.cpp command-line tool (see whisper.rs).
    Whisper(WhisperKind),
    /// Parakeet V3 first, Whisper turbo when Parakeet clearly missed speech.
    Hybrid,
}

enum Source {
    /// A .tar.gz holding the model directory.
    Archive(&'static str),
    /// Individual files: (url, file name inside the model directory).
    Files(&'static [(&'static str, &'static str)]),
    /// whisper.cpp tools + model, managed by whisper.rs.
    Whisper(WhisperKind),
    /// Made of other entries; downloads whichever parts are missing.
    Parts,
}

pub struct ModelInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub languages: &'static str,
    pub size_mb: u32,
    dir_name: &'static str,
    engine: Engine,
    source: Source,
}

macro_rules! hf_v2 {
    ($file:literal) => {
        concat!("https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx/resolve/main/", $file)
    };
}

pub const CATALOG: &[ModelInfo] = &[
    ModelInfo {
        id: "parakeet-v3",
        name: "Parakeet V3",
        description: "Best all-round accuracy. Recommended.",
        languages: "English + 24 European languages",
        size_mb: 478,
        dir_name: "parakeet-tdt-0.6b-v3-int8",
        engine: Engine::Parakeet,
        source: Source::Archive("https://blob.handy.computer/parakeet-v3-int8.tar.gz"),
    },
    ModelInfo {
        id: "hybrid",
        name: "Hybrid: Parakeet + Whisper",
        description: "Parakeet's speed for normal speech; whispers and missed words get a second pass with Whisper turbo (a few seconds).",
        languages: "English",
        size_mb: 478 + whisper::RUNTIME_MB + whisper::MODEL_MB,
        dir_name: "",
        engine: Engine::Hybrid,
        source: Source::Parts,
    },
    ModelInfo {
        id: "whisper-turbo",
        name: "Whisper Large v3 Turbo",
        description: "Best with whispers, accents and rare words, but slow on this PC's CPU: about 3-5 s per dictation.",
        languages: "English",
        size_mb: whisper::RUNTIME_MB + whisper::MODEL_MB,
        dir_name: "",
        engine: Engine::Whisper(WhisperKind::Turbo),
        source: Source::Whisper(WhisperKind::Turbo),
    },
    ModelInfo {
        id: "whisper-tiny",
        name: "Whisper Tiny",
        description: "The smallest Whisper, and quick on any CPU. Multilingual - an experiment for Hinglish, so expect rough results.",
        languages: "Multilingual (Hindi, English + 97 more)",
        size_mb: whisper::RUNTIME_MB + whisper::TINY_MODEL_MB,
        dir_name: "",
        engine: Engine::Whisper(WhisperKind::Tiny),
        source: Source::Whisper(WhisperKind::Tiny),
    },
    ModelInfo {
        id: "hinglish-apex",
        name: "Whisper Hinglish Apex",
        description: "Fine-tuned for Hindi + English mixed speech; types romanized Hinglish directly (\"Main aaj subah office ja raha hoon\"). Slow like the turbo it's built on, and rough on pure English.",
        languages: "Romanized Hinglish (Hindi + English)",
        size_mb: whisper::RUNTIME_MB + whisper::APEX_MODEL_MB,
        dir_name: "",
        engine: Engine::Whisper(WhisperKind::HinglishApex),
        source: Source::Whisper(WhisperKind::HinglishApex),
    },
    ModelInfo {
        id: "hinglish-swift",
        name: "Whisper Hinglish Swift",
        description: "Small and quick, types romanized Hinglish the same way. Rougher than Apex on noisy audio, and rough on pure English.",
        languages: "Romanized Hinglish (Hindi + English)",
        size_mb: whisper::RUNTIME_MB + whisper::SWIFT_MODEL_MB,
        dir_name: "",
        engine: Engine::Whisper(WhisperKind::HinglishSwift),
        source: Source::Whisper(WhisperKind::HinglishSwift),
    },
    ModelInfo {
        id: "parakeet-v2",
        name: "Parakeet V2",
        description: "English-only, slightly more accurate on English than V3.",
        languages: "English",
        size_mb: 631,
        dir_name: "parakeet-tdt-0.6b-v2-int8",
        engine: Engine::Parakeet,
        source: Source::Files(&[
            (hf_v2!("encoder-model.int8.onnx"), "encoder-model.int8.onnx"),
            (hf_v2!("decoder_joint-model.int8.onnx"), "decoder_joint-model.int8.onnx"),
            (hf_v2!("nemo128.onnx"), "nemo128.onnx"),
            (hf_v2!("vocab.txt"), "vocab.txt"),
        ]),
    },
    ModelInfo {
        id: "moonshine-base",
        name: "Moonshine Base",
        description: "Small and fast. A little less accurate than Parakeet.",
        languages: "English",
        size_mb: 55,
        dir_name: "moonshine-base",
        engine: Engine::MoonshineBase,
        source: Source::Archive("https://blob.handy.computer/moonshine-base.tar.gz"),
    },
    ModelInfo {
        id: "moonshine-tiny",
        name: "Moonshine Tiny",
        description: "Smallest and fastest. Good for older or busy PCs.",
        languages: "English",
        size_mb: 31,
        dir_name: "moonshine-tiny-streaming-en",
        engine: Engine::MoonshineStreaming,
        source: Source::Archive("https://blob.handy.computer/moonshine-tiny-streaming-en.tar.gz"),
    },
];

pub fn find(id: &str) -> Option<&'static ModelInfo> {
    CATALOG.iter().find(|m| m.id == id)
}

fn parakeet_v3() -> &'static ModelInfo {
    find("parakeet-v3").expect("parakeet-v3 in catalog")
}

/// Ids of entries that `id` is built from (deleting those would break it).
pub fn parts_of(id: &str) -> &'static [&'static str] {
    match id {
        "hybrid" => &["parakeet-v3", "whisper-turbo"],
        _ => &[],
    }
}

pub(crate) fn models_root() -> PathBuf {
    crate::settings::app_root().join("models")
}

impl ModelInfo {
    pub fn dir(&self) -> PathBuf {
        models_root().join(self.dir_name)
    }

    pub fn is_installed(&self) -> bool {
        match self.engine {
            Engine::Whisper(kind) => whisper::is_installed(kind),
            Engine::Hybrid => parakeet_v3().is_installed() && whisper::is_installed(WhisperKind::Turbo),
            _ => has_required_files(self.engine, &self.dir()),
        }
    }

    pub fn load(&self) -> Result<Box<dyn SpeechModel + Send>> {
        let dir = self.dir();
        let model: Box<dyn SpeechModel + Send> = match self.engine {
            Engine::Parakeet => Box::new(ParakeetModel::load(&dir, &Quantization::Int8).map_err(|e| anyhow!("{e}"))?),
            Engine::MoonshineBase => Box::new(
                MoonshineModel::load(&dir, MoonshineVariant::Base, &Quantization::default())
                    .map_err(|e| anyhow!("{e}"))?,
            ),
            Engine::MoonshineStreaming => {
                let threads = thread::available_parallelism().map(|n| n.get().min(4)).unwrap_or(2);
                Box::new(StreamingModel::load(&dir, threads, &Quantization::default()).map_err(|e| anyhow!("{e}"))?)
            }
            Engine::Whisper(kind) => whisper::load(kind)?,
            Engine::Hybrid => Box::new(whisper::Hybrid::new(parakeet_v3().load()?, whisper::load(WhisperKind::Turbo)?)),
        };
        Ok(model)
    }

    pub fn remove(&self) -> Result<()> {
        match self.engine {
            Engine::Whisper(kind) => return whisper::remove(kind),
            Engine::Hybrid => bail!("Hybrid uses Parakeet V3 and Whisper turbo; delete those instead"),
            _ => {}
        }
        let dir = self.dir();
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }

    /// Downloads and installs the model. `progress(downloaded, total)` is
    /// called as bytes arrive; `total` is 0 when the size isn't known yet.
    pub fn download(&self, mut progress: impl FnMut(u64, u64)) -> Result<()> {
        self.download_with(&mut progress)
    }

    fn download_with(&self, progress: &mut dyn FnMut(u64, u64)) -> Result<()> {
        fs::create_dir_all(models_root())?;
        let client = reqwest::blocking::Client::builder()
            // Applies to each read, so a silent connection errors out
            // instead of hanging, and the retry loop resumes it.
            .timeout(STALL_TIMEOUT)
            .connect_timeout(Duration::from_secs(20))
            .build()?;

        match &self.source {
            Source::Archive(url) => {
                let part = models_root().join(format!("{}.tar.gz.part", self.dir_name));
                fetch_resumable(&client, url, &part, &mut |done, total| progress(done, total))?;
                self.install_archive(&part)?;
                let _ = fs::remove_file(&part);
            }
            Source::Files(files) => {
                let staging = models_root().join(format!("{}.part", self.dir_name));
                fs::create_dir_all(&staging)?;
                let sizes: Vec<u64> = files.iter().map(|(url, _)| remote_size(&client, url).unwrap_or(0)).collect();
                let total: u64 = sizes.iter().sum();
                let mut finished = 0u64;
                for ((url, name), size) in files.iter().zip(&sizes) {
                    let dest = staging.join(name);
                    fetch_resumable(&client, url, &dest, &mut |done, _| progress(finished + done, total))?;
                    finished += (*size).max(fs::metadata(&dest).map(|m| m.len()).unwrap_or(0));
                }
                if !has_required_files(self.engine, &staging) {
                    bail!("Downloaded files are incomplete");
                }
                let target = self.dir();
                let _ = fs::remove_dir_all(&target);
                fs::rename(&staging, &target)?;
            }
            Source::Whisper(kind) => whisper::download(*kind, progress)?,
            Source::Parts => {
                let parakeet = parakeet_v3();
                let mb = |n: u32| n as u64 * 1_048_576;
                let parakeet_bytes = if parakeet.is_installed() { 0 } else { mb(parakeet.size_mb) };
                let whisper_bytes = if whisper::is_installed(WhisperKind::Turbo) { 0 } else { mb(whisper::RUNTIME_MB + whisper::MODEL_MB) };
                let total = parakeet_bytes + whisper_bytes;
                if parakeet_bytes > 0 {
                    parakeet.download_with(&mut |done, _| progress(done, total))?;
                }
                if whisper_bytes > 0 {
                    whisper::download(WhisperKind::Turbo, &mut |done, _| progress(parakeet_bytes + done, total))?;
                }
            }
        }
        Ok(())
    }

    fn install_archive(&self, archive: &Path) -> Result<()> {
        let staging = models_root().join(format!("{}._extract", self.dir_name));
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir_all(&staging)?;
        let gz = flate2::read::GzDecoder::new(File::open(archive)?);
        tar::Archive::new(gz)
            .unpack(&staging)
            .context("Failed to unpack model archive")?;

        let found = find_model_dir(self.engine, &staging)
            .ok_or_else(|| anyhow!("Archive doesn't contain the expected model files"))?;
        let target = self.dir();
        let _ = fs::remove_dir_all(&target);
        fs::rename(&found, &target)?;
        let _ = fs::remove_dir_all(&staging);

        // Archives packed on a Mac carry "._name" metadata files; drop them.
        for entry in fs::read_dir(&target)?.flatten() {
            if entry.file_name().to_string_lossy().starts_with("._") {
                let _ = fs::remove_file(entry.path());
            }
        }
        Ok(())
    }
}

fn has_required_files(engine: Engine, dir: &Path) -> bool {
    let has = |name: &str| dir.join(name).is_file();
    match engine {
        Engine::Parakeet => {
            has("vocab.txt")
                && has("nemo128.onnx")
                && fs::read_dir(dir)
                    .map(|entries| {
                        entries
                            .flatten()
                            .any(|e| e.file_name().to_string_lossy().starts_with("encoder-model"))
                    })
                    .unwrap_or(false)
        }
        Engine::MoonshineBase => {
            has("encoder_model.onnx") && has("decoder_model_merged.onnx") && has("tokenizer.json")
        }
        Engine::MoonshineStreaming => {
            has("encoder.onnx") && has("decoder.onnx") && has("tokenizer.json")
        }
        Engine::Whisper(_) | Engine::Hybrid => false,
    }
}

fn find_model_dir(engine: Engine, dir: &Path) -> Option<PathBuf> {
    if has_required_files(engine, dir) {
        return Some(dir.to_path_buf());
    }
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_dir())
        .find_map(|e| find_model_dir(engine, &e.path()))
}

fn remote_size(client: &reqwest::blocking::Client, url: &str) -> Result<u64> {
    let resp = client.head(url).send()?.error_for_status()?;
    resp.content_length().ok_or_else(|| anyhow!("No size"))
}

/// Downloads `url` into `dest`, resuming from whatever `dest` already holds.
/// Retries with backoff when the connection drops or stalls.
pub(crate) fn fetch_resumable(
    client: &reqwest::blocking::Client,
    url: &str,
    dest: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<()> {
    let mut last_err = None;
    for attempt in 0..MAX_ATTEMPTS {
        if attempt > 0 {
            log::warn!("Download attempt {attempt} failed, retrying: {:?}", last_err);
            thread::sleep(Duration::from_secs(2u64.pow(attempt.min(4))));
        }
        match fetch_once(client, url, dest, progress) {
            Ok(()) => return Ok(()),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("Download failed")))
}

fn fetch_once(
    client: &reqwest::blocking::Client,
    url: &str,
    dest: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<()> {
    let have = fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    let mut req = client.get(url);
    if have > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let mut resp = req.send()?;

    let status = resp.status();
    if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        // We already have every byte.
        progress(have, have);
        return Ok(());
    }
    let resp_status = resp.error_for_status_ref().map(|r| r.status())?;
    let resuming = resp_status == reqwest::StatusCode::PARTIAL_CONTENT;
    let start = if resuming { have } else { 0 };
    let total = resp.content_length().map(|n| n + start).unwrap_or(0);

    let file = if resuming {
        OpenOptions::new().append(true).open(dest)?
    } else {
        // Server ignored the range: start over.
        File::create(dest)?
    };
    let mut out = BufWriter::new(file);
    let mut buf = vec![0u8; 1 << 20];
    let mut downloaded = start;
    progress(downloaded, total);
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        downloaded += n as u64;
        progress(downloaded, total);
    }
    out.flush()?;
    if total > 0 && downloaded < total {
        bail!("Connection closed early ({downloaded} of {total} bytes)");
    }
    Ok(())
}

/// The model the user picked, falling back to the default.
pub fn selected_model() -> &'static ModelInfo {
    crate::settings::load()
        .model
        .and_then(|id| find(&id))
        .unwrap_or_else(|| find(DEFAULT_MODEL).expect("default model in catalog"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whisper_tiny_is_in_the_catalog() {
        let info = find("whisper-tiny").expect("whisper-tiny in the catalog");
        assert_eq!(info.name, "Whisper Tiny");
        assert!(matches!(info.engine, Engine::Whisper(WhisperKind::Tiny)));
        assert_eq!(info.size_mb, whisper::RUNTIME_MB + whisper::TINY_MODEL_MB);
    }

    #[test]
    fn the_hinglish_models_are_in_the_catalog() {
        let apex = find("hinglish-apex").expect("hinglish-apex in the catalog");
        assert_eq!(apex.name, "Whisper Hinglish Apex");
        assert!(matches!(apex.engine, Engine::Whisper(WhisperKind::HinglishApex)));
        assert_eq!(apex.size_mb, whisper::RUNTIME_MB + whisper::APEX_MODEL_MB);

        let swift = find("hinglish-swift").expect("hinglish-swift in the catalog");
        assert_eq!(swift.name, "Whisper Hinglish Swift");
        assert!(matches!(swift.engine, Engine::Whisper(WhisperKind::HinglishSwift)));
        assert_eq!(swift.size_mb, whisper::RUNTIME_MB + whisper::SWIFT_MODEL_MB);
    }

    /// Runs a real model on a WAV file. Needs the model installed, so it's
    /// opt-in: `SRUTAM_TEST_WAV=path cargo test --release -- --ignored`.
    #[test]
    #[ignore]
    fn transcribes_wav_with_installed_model() {
        let wav = std::env::var("SRUTAM_TEST_WAV").expect("set SRUTAM_TEST_WAV");
        let id = std::env::var("SRUTAM_TEST_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
        let info = find(&id).expect("model in catalog");
        assert!(info.is_installed(), "{id} isn't installed");

        // A directory runs every .wav in it, in name order.
        let path = Path::new(&wav);
        let mut files: Vec<PathBuf> = if path.is_dir() {
            fs::read_dir(path).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "wav")).collect()
        } else {
            vec![path.to_path_buf()]
        };
        files.sort();

        // SRUTAM_TEST_GAIN=0.02 simulates quiet speech: scales the audio and
        // adds hiss, then runs it through the same boost dictation uses.
        let gain: Option<f32> = std::env::var("SRUTAM_TEST_GAIN").ok().and_then(|g| g.parse().ok());

        // SRUTAM_TEST_HOTWORDS="Tauri,Kubernetes" boosts those words while
        // decoding and shows the dictionary correction on top.
        let hotwords: Vec<String> = std::env::var("SRUTAM_TEST_HOTWORDS")
            .map(|w| w.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
            .unwrap_or_default();

        let mut model = info.load().unwrap();
        model.set_hotwords(&hotwords);
        model.set_latin_only(true);
        for file in files {
            let mut samples = transcribe_rs::audio::read_wav_samples(&file).unwrap();
            // SRUTAM_TEST_ENHANCE=1 runs the same speech check and boost as
            // dictation (useful on saved recordings).
            if std::env::var_os("SRUTAM_TEST_ENHANCE").is_some() && gain.is_none() {
                match crate::enhance::prepare(&samples) {
                    Some(boosted) => samples = boosted,
                    None => {
                        println!("[{id}] {}: rejected as no speech", file.file_name().unwrap().to_string_lossy());
                        continue;
                    }
                }
            }
            if let Some(gain) = gain {
                let mut seed = 7u32;
                for s in &mut samples {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                    *s = *s * gain + ((seed >> 16) as f32 / 32_768.0 - 1.0) * 0.0004;
                }
                let quiet = model.transcribe(&samples, &transcribe_rs::TranscribeOptions::default()).unwrap().text;
                println!("  quiet, no boost: {quiet}");
                samples = match crate::enhance::prepare(&samples) {
                    Some(boosted) => boosted,
                    None => {
                        println!("  boost: no speech detected");
                        continue;
                    }
                };
            }
            let started = std::time::Instant::now();
            let text = model.transcribe(&samples, &transcribe_rs::TranscribeOptions::default()).unwrap().text;
            println!("[{id}] {} ({:.1}s audio, {} ms)
  raw:   {text}
  rules: {}", file.file_name().unwrap().to_string_lossy(), samples.len() as f32 / 16000.0, started.elapsed().as_millis(), crate::format::rules::apply(&text));
            if !hotwords.is_empty() {
                println!("  dict:  {}", crate::format::dictionary::apply(&text, &hotwords));
            }
            if text.trim().is_empty() {
                println!("  (model heard nothing)");
            }
        }
    }
}
