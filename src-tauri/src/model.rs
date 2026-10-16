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

pub const DEFAULT_MODEL: &str = "parakeet-v3";

/// A read that returns nothing for this long counts as a stalled connection.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ATTEMPTS: u32 = 6;

#[derive(Clone, Copy)]
enum Engine {
    Parakeet,
    MoonshineBase,
    MoonshineStreaming,
}

enum Source {
    /// A .tar.gz holding the model directory.
    Archive(&'static str),
    /// Individual files: (url, file name inside the model directory).
    Files(&'static [(&'static str, &'static str)]),
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

fn models_root() -> PathBuf {
    crate::settings::app_root().join("models")
}

impl ModelInfo {
    pub fn dir(&self) -> PathBuf {
        models_root().join(self.dir_name)
    }

    pub fn is_installed(&self) -> bool {
        has_required_files(self.engine, &self.dir())
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
        };
        Ok(model)
    }

    pub fn remove(&self) -> Result<()> {
        let dir = self.dir();
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }

    /// Downloads and installs the model. `progress(downloaded, total)` is
    /// called as bytes arrive; `total` is 0 when the size isn't known yet.
    pub fn download(&self, mut progress: impl FnMut(u64, u64)) -> Result<()> {
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
fn fetch_resumable(
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

    /// Runs a real model on a WAV file. Needs the model installed, so it's
    /// opt-in: `SRUTAM_TEST_WAV=path cargo test --release -- --ignored`.
    #[test]
    #[ignore]
    fn transcribes_wav_with_installed_model() {
        let wav = std::env::var("SRUTAM_TEST_WAV").expect("set SRUTAM_TEST_WAV");
        let id = std::env::var("SRUTAM_TEST_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
        let info = find(&id).expect("model in catalog");
        assert!(info.is_installed(), "{id} isn't installed");

        let samples = transcribe_rs::audio::read_wav_samples(Path::new(&wav)).unwrap();
        let mut model = info.load().unwrap();
        let started = std::time::Instant::now();
        let text = model.transcribe(&samples, &transcribe_rs::TranscribeOptions::default()).unwrap().text;
        println!("[{id}] {:.1}s audio in {} ms: {text}", samples.len() as f32 / 16000.0, started.elapsed().as_millis());
        assert!(text.to_lowercase().contains("test"));
    }
}
