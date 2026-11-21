// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Sruthi contributors

//! Speech model catalog: what can be installed, where it lives, whether it's
//! installed, and a resumable downloader that survives stalled connections.
//! Built-ins and user-added links share one runtime registry.

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, OnceLock, RwLock};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use transcribe_rs::SpeechModel;
use transcribe_rs::onnx::Quantization;
use transcribe_rs::onnx::moonshine::{MoonshineModel, MoonshineVariant, StreamingModel};
use transcribe_rs::onnx::parakeet::ParakeetModel;

use crate::whisper::{self, WhisperKind};

pub const DEFAULT_MODEL: &str = "parakeet-v3";

/// A read that returns nothing for this long counts as a stalled connection.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ATTEMPTS: u32 = 6;

/// How much heat a model can put into a laptop during a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Heat {
    Light,
    Moderate,
    Heavy,
}

impl Heat {
    pub fn label(self) -> &'static str {
        match self {
            Heat::Light => "light",
            Heat::Moderate => "moderate",
            Heat::Heavy => "heavy",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "light" => Some(Heat::Light),
            "moderate" => Some(Heat::Moderate),
            "heavy" => Some(Heat::Heavy),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Engine {
    Parakeet,
    MoonshineBase,
    MoonshineStreaming,
    /// whisper.cpp command-line tool (see whisper.rs).
    Whisper(WhisperKind),
    /// Parakeet V3 first, Whisper turbo when Parakeet clearly missed speech.
    Hybrid,
    /// A model added by link; its layout decides which engine actually runs it.
    CustomOnnx,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Source {
    /// A .tar.gz holding the model directory.
    Archive(String),
    /// Individual files: (url, file name inside the model directory).
    Files(Vec<(String, String)>),
    /// whisper.cpp tools + model, managed by whisper.rs.
    Whisper(WhisperKind),
    /// Made of other entries; downloads whichever parts are missing.
    Parts,
    /// One file at a URL, saved under `name`.
    SingleFile(String, String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub languages: String,
    pub size_mb: u32,
    dir_name: String,
    engine: Engine,
    source: Source,
    /// Custom entries carry their own heat class and fallback wait estimate.
    #[serde(default)]
    heat_override: Option<Heat>,
    #[serde(default)]
    fallback_override: Option<f32>,
}

macro_rules! hf_v2 {
    ($file:literal) => {
        concat!(
            "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx/resolve/main/",
            $file
        )
    };
}

/// The models shipped with the app. Custom links are added on top at runtime.
pub static BUILTIN: LazyLock<Vec<ModelInfo>> = LazyLock::new(builtin);

fn builtin() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "parakeet-v3".into(),
            name: "Parakeet V3".into(),
            description: "Best all-round accuracy. Recommended.".into(),
            languages: "English + 24 European languages".into(),
            size_mb: 478,
            dir_name: "parakeet-tdt-0.6b-v3-int8".into(),
            engine: Engine::Parakeet,
            source: Source::Archive("https://blob.handy.computer/parakeet-v3-int8.tar.gz".into()),
            heat_override: None,
            fallback_override: None,
        },
        ModelInfo {
            id: "hybrid".into(),
            name: "Hybrid: Parakeet + Whisper".into(),
            description: "Parakeet's speed for normal speech; whispers and missed words get a second pass with Whisper turbo (a few seconds).".into(),
            languages: "English".into(),
            size_mb: 478 + whisper::RUNTIME_MB + whisper::MODEL_MB,
            dir_name: String::new(),
            engine: Engine::Hybrid,
            source: Source::Parts,
            heat_override: None,
            fallback_override: None,
        },
        ModelInfo {
            id: "whisper-turbo".into(),
            name: "Whisper Large v3 Turbo".into(),
            description: "Best with whispers, accents and rare words, but slow on this PC's CPU: about 3-5 s per dictation.".into(),
            languages: "English".into(),
            size_mb: whisper::RUNTIME_MB + whisper::MODEL_MB,
            dir_name: String::new(),
            engine: Engine::Whisper(WhisperKind::Turbo),
            source: Source::Whisper(WhisperKind::Turbo),
            heat_override: None,
            fallback_override: None,
        },
        ModelInfo {
            id: "whisper-tiny".into(),
            name: "Whisper Tiny".into(),
            description: "The smallest Whisper, and quick on any CPU. Multilingual - an experiment for Hinglish, so expect rough results.".into(),
            languages: "Multilingual (Hindi, English + 97 more)".into(),
            size_mb: whisper::RUNTIME_MB + whisper::TINY_MODEL_MB,
            dir_name: String::new(),
            engine: Engine::Whisper(WhisperKind::Tiny),
            source: Source::Whisper(WhisperKind::Tiny),
            heat_override: None,
            fallback_override: None,
        },
        ModelInfo {
            id: "hinglish-apex".into(),
            name: "Whisper Hinglish Apex".into(),
            description: "Fine-tuned for Hindi + English mixed speech; types romanized Hinglish directly (\"Main aaj subah office ja raha hoon\"). Slow like the turbo it's built on, and rough on pure English.".into(),
            languages: "Romanized Hinglish (Hindi + English)".into(),
            size_mb: whisper::RUNTIME_MB + whisper::APEX_MODEL_MB,
            dir_name: String::new(),
            engine: Engine::Whisper(WhisperKind::HinglishApex),
            source: Source::Whisper(WhisperKind::HinglishApex),
            heat_override: None,
            fallback_override: None,
        },
        ModelInfo {
            id: "hinglish-swift".into(),
            name: "Whisper Hinglish Swift".into(),
            description: "Small and quick, types romanized Hinglish the same way. Rougher than Apex on noisy audio, and rough on pure English.".into(),
            languages: "Romanized Hinglish (Hindi + English)".into(),
            size_mb: whisper::RUNTIME_MB + whisper::SWIFT_MODEL_MB,
            dir_name: String::new(),
            engine: Engine::Whisper(WhisperKind::HinglishSwift),
            source: Source::Whisper(WhisperKind::HinglishSwift),
            heat_override: None,
            fallback_override: None,
        },
        ModelInfo {
            id: "parakeet-v2".into(),
            name: "Parakeet V2".into(),
            description: "English-only, slightly more accurate on English than V3.".into(),
            languages: "English".into(),
            size_mb: 631,
            dir_name: "parakeet-tdt-0.6b-v2-int8".into(),
            engine: Engine::Parakeet,
            source: Source::Files(vec![
                (hf_v2!("encoder-model.int8.onnx").into(), "encoder-model.int8.onnx".into()),
                (hf_v2!("decoder_joint-model.int8.onnx").into(), "decoder_joint-model.int8.onnx".into()),
                (hf_v2!("nemo128.onnx").into(), "nemo128.onnx".into()),
                (hf_v2!("vocab.txt").into(), "vocab.txt".into()),
            ]),
            heat_override: None,
            fallback_override: None,
        },
        ModelInfo {
            id: "moonshine-base".into(),
            name: "Moonshine Base".into(),
            description: "Small and fast. A little less accurate than Parakeet.".into(),
            languages: "English".into(),
            size_mb: 55,
            dir_name: "moonshine-base".into(),
            engine: Engine::MoonshineBase,
            source: Source::Archive("https://blob.handy.computer/moonshine-base.tar.gz".into()),
            heat_override: None,
            fallback_override: None,
        },
        ModelInfo {
            id: "moonshine-tiny".into(),
            name: "Moonshine Tiny".into(),
            description: "Smallest and fastest. Good for older or busy PCs.".into(),
            languages: "English".into(),
            size_mb: 31,
            dir_name: "moonshine-tiny-streaming-en".into(),
            engine: Engine::MoonshineStreaming,
            source: Source::Archive("https://blob.handy.computer/moonshine-tiny-streaming-en.tar.gz".into()),
            heat_override: None,
            fallback_override: None,
        },
    ]
}

/// Built-ins plus the user's saved links, shared app-wide.
pub struct Registry {
    models: Vec<Arc<ModelInfo>>,
}

impl Registry {
    pub fn load() -> Self {
        Self::load_from(&crate::settings::app_root())
    }

    /// Loads built-ins and any valid custom entries under `root`. A corrupt
    /// file drops only the custom entries; built-ins always survive.
    pub fn load_from(root: &Path) -> Self {
        let mut models: Vec<Arc<ModelInfo>> = BUILTIN.iter().cloned().map(Arc::new).collect();
        for value in read_custom_values_from(root) {
            match serde_json::from_value::<ModelInfo>(value) {
                Ok(model) => models.push(Arc::new(model)),
                Err(e) => log::warn!("Skipping invalid custom model entry: {e}"),
            }
        }
        Self { models }
    }

    pub fn all(&self) -> &[Arc<ModelInfo>] {
        &self.models
    }

    pub fn find(&self, id: &str) -> Option<Arc<ModelInfo>> {
        self.models.iter().find(|m| m.id == id).cloned()
    }
}

static REGISTRY: OnceLock<RwLock<Registry>> = OnceLock::new();

fn registry() -> &'static RwLock<Registry> {
    REGISTRY.get_or_init(|| RwLock::new(Registry::load()))
}

pub fn all() -> Vec<Arc<ModelInfo>> {
    registry().read().unwrap().all().to_vec()
}

pub fn find(id: &str) -> Option<Arc<ModelInfo>> {
    registry().read().unwrap().find(id)
}

/// Re-reads `custom_models.json` after an add or remove.
pub fn reload_registry() {
    *registry().write().unwrap() = Registry::load();
}

impl ModelInfo {
    /// A starting point for a user-added model. The downloader fills in the
    /// source and layout before it is saved.
    pub fn custom(id: &str, name: &str, heat: Heat, fallback_ms_per_s: f32) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            languages: String::new(),
            size_mb: 0,
            dir_name: format!("custom-{}", short_hash(id)),
            engine: Engine::CustomOnnx,
            source: Source::SingleFile(String::new(), "model.onnx".into()),
            heat_override: Some(heat),
            fallback_override: Some(fallback_ms_per_s),
        }
    }

    /// Adds the concrete download layout to a custom entry.
    pub fn with_source(mut self, source: Source) -> Self {
        self.source = source;
        self
    }

    /// Sets the engine a custom entry will load with.
    pub fn with_engine(mut self, engine: Engine) -> Self {
        self.engine = engine;
        self
    }

    pub fn with_dir(mut self, dir_name: String) -> Self {
        self.dir_name = dir_name;
        self
    }

    pub fn with_details(mut self, description: String, languages: String, size_mb: u32) -> Self {
        self.description = description;
        self.languages = languages;
        self.size_mb = size_mb;
        self
    }
}

/// Hosts we accept model links from. Nothing is fetched from anywhere else.
const CUSTOM_HOSTS: &[&str] = &[
    "huggingface.co",
    "github.com",
    "objects.githubusercontent.com",
    "blob.handy.computer",
];

/// Validates a pasted model link and removes the fragment (which is never
/// part of a download URL). Query parameters are kept.
pub fn validate_model_url(url: &str) -> Result<String, String> {
    let mut parsed = reqwest::Url::parse(url).map_err(|e| format!("Invalid URL: {e}"))?;
    if parsed.scheme() != "https" {
        return Err("Model links must use HTTPS".into());
    }
    let host = parsed.host_str().unwrap_or("").to_lowercase();
    if !CUSTOM_HOSTS.contains(&host.as_str()) {
        return Err(format!("Model links from {host} aren't allowed"));
    }
    parsed.set_fragment(None);
    Ok(parsed.to_string())
}

fn custom_models_path() -> PathBuf {
    crate::settings::app_root().join("custom_models.json")
}

fn read_custom_values_from(root: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(root.join("custom_models.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<serde_json::Value>>(&text).ok())
        .unwrap_or_default()
}

fn read_custom_values() -> Vec<serde_json::Value> {
    read_custom_values_from(&crate::settings::app_root())
}

/// Keeps unknown entries verbatim so a schema change or a hand-edited file
/// cannot silently erase someone's models on the next save.
fn write_custom_values(entries: &[serde_json::Value]) -> Result<()> {
    let root = crate::settings::app_root();
    fs::create_dir_all(&root)?;
    let tmp = root.join("custom_models.json.part");
    fs::write(&tmp, serde_json::to_string_pretty(entries)?)?;
    fs::rename(&tmp, custom_models_path())?;
    Ok(())
}

/// Works out which engine and download layout a link needs. Only ONNX files,
/// Hugging Face repos and model archives are accepted.
fn model_layout(url: &str) -> Result<(Engine, Source), String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| e.to_string())?;
    let segments: Vec<&str> = parsed
        .path_segments()
        .map(|s| s.filter(|x| !x.is_empty()).collect())
        .unwrap_or_default();
    let last = segments.last().copied().unwrap_or("");
    if last.ends_with(".tar.gz") {
        // An archive of model files is only useful if it holds a layout the
        // offline engine recognises; extraction checks this.
        return Ok((Engine::Parakeet, Source::Archive(url.to_owned())));
    }
    if last.ends_with(".onnx") {
        return Ok((
            Engine::CustomOnnx,
            Source::SingleFile(url.to_owned(), last.to_owned()),
        ));
    }
    if parsed.host_str() == Some("huggingface.co") && segments.len() == 2 {
        let api = format!(
            "https://huggingface.co/api/models/{}/{}/tree/main",
            segments[0], segments[1]
        );
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| e.to_string())?;
        let body = client
            .get(&api)
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.text())
            .map_err(|e| format!("Couldn't read that Hugging Face repo: {e}"))?;
        let entries: Vec<serde_json::Value> = serde_json::from_str(&body)
            .map_err(|e| format!("Couldn't read that Hugging Face repo: {e}"))?;
        let mut files = Vec::new();
        for entry in entries {
            let Some(path) = entry["path"].as_str() else {
                continue;
            };
            let Ok(file_name) = path.rsplit('/').next().ok_or(()) else {
                continue;
            };
            if [".onnx", ".json", ".txt", ".safetensors"]
                .iter()
                .any(|ext| file_name.ends_with(ext))
            {
                files.push((
                    format!(
                        "https://huggingface.co/{}/{}/resolve/main/{}",
                        segments[0], segments[1], path
                    ),
                    file_name.to_owned(),
                ));
                if files.len() >= 20 {
                    break;
                }
            }
        }
        if files.is_empty() {
            return Err("No model files found at that Hugging Face repo".into());
        }
        // Parakeet's ONNX layout is the one layout we can load generically.
        // Use it when the repo has the expected files, so a pasted link to a
        // Parakeet-style repo is a real, runnable model.
        let names: Vec<&str> = files.iter().map(|(_, n)| n.as_str()).collect();
        let parakeet = names.iter().any(|n| n.starts_with("encoder-model"))
            && names.iter().any(|n| n.starts_with("decoder_joint"))
            && names.contains(&"nemo128.onnx")
            && names.contains(&"vocab.txt");
        let engine = if parakeet {
            Engine::Parakeet
        } else {
            Engine::CustomOnnx
        };
        return Ok((engine, Source::Files(files)));
    }
    Err("That link doesn't point to an ONNX file, a model archive, or a Hugging Face repo".into())
}

/// Best-effort size probe. A missing Content-Length simply means 0 MB in the UI.
fn remote_size_mb(url: &str) -> u32 {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .ok()
        .and_then(|c| c.head(url).send().ok())
        .and_then(|r| r.content_length());
    client.map(|n| (n / 1_048_576) as u32).unwrap_or(0)
}

/// Adds a link to the custom registry and reloads it. Downloaded files stay
/// where they are; this only writes `custom_models.json`.
pub fn add_custom(url: &str, name: &str, heat: Heat) -> Result<Arc<ModelInfo>, String> {
    let url = validate_model_url(url)?;
    let id = format!("custom-{}", &short_hash(&url)[..16]);
    let (engine, source) = model_layout(&url)?;
    let entry = ModelInfo::custom(&id, name, heat, 1_000.0)
        .with_engine(engine)
        .with_source(source)
        .with_dir(id.clone())
        .with_details(
            "Model added by link.".into(),
            "Custom".into(),
            remote_size_mb(&url),
        );
    let mut values = read_custom_values();
    values.retain(|value| value["id"].as_str() != Some(id.as_str()));
    values.push(serde_json::to_value(&entry).map_err(|e| e.to_string())?);
    write_custom_values(&values).map_err(|e| e.to_string())?;
    reload_registry();
    find(&id).ok_or_else(|| "Custom model wasn't registered".into())
}

/// Removes a link from the registry metadata only — never the downloaded files.
pub fn remove_custom(id: &str) -> Result<(), String> {
    let mut values = read_custom_values();
    let before = values.len();
    values.retain(|value| value["id"].as_str() != Some(id));
    if values.len() == before {
        return Err("Custom model not found".into());
    }
    write_custom_values(&values).map_err(|e| e.to_string())?;
    reload_registry();
    Ok(())
}

/// Short stable id for a custom model's directory name.
fn short_hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{hash:016x}")
}

impl ModelInfo {
    /// Whether this OS can run the model. The Whisper engine (whisper.cpp
    /// binaries) is Windows-only for now; ONNX models run everywhere.
    pub fn available_here(&self) -> bool {
        !self.uses_whisper() || cfg!(windows)
    }
}

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
        models_root().join(&self.dir_name)
    }

    pub fn is_installed(&self) -> bool {
        match &self.engine {
            Engine::Whisper(kind) => whisper::is_installed(*kind),
            Engine::Hybrid => {
                parakeet_v3().is_installed() && whisper::is_installed(WhisperKind::Turbo)
            }
            _ => has_required_files(&self.engine, &self.dir()),
        }
    }

    /// Everything whisper-backed gains the card engine when it's set up;
    /// used for the device row and the timing log.
    pub fn uses_whisper(&self) -> bool {
        matches!(self.engine, Engine::Whisper(_) | Engine::Hybrid)
    }

    /// Whether the offline engine can actually load this layout. Pasted links
    /// with an unknown ONNX layout stay downloadable but cannot run.
    pub fn runnable(&self) -> bool {
        !matches!(self.engine, Engine::CustomOnnx)
    }

    /// The cost of running this model on a laptop. Apex and Turbo are
    /// large Whisper models; Hybrid can fall back to one on the CPU.
    pub fn heat(&self) -> Heat {
        if let Some(heat) = self.heat_override {
            return heat;
        }
        match &self.engine {
            Engine::Parakeet | Engine::MoonshineBase | Engine::MoonshineStreaming => Heat::Light,
            Engine::Whisper(WhisperKind::Tiny) => Heat::Light,
            Engine::Whisper(WhisperKind::HinglishSwift) => Heat::Moderate,
            Engine::Whisper(WhisperKind::Turbo | WhisperKind::HinglishApex) => Heat::Heavy,
            Engine::Hybrid => Heat::Heavy,
            Engine::CustomOnnx => Heat::Moderate,
        }
    }

    pub fn is_heavy(&self) -> bool {
        self.heat() == Heat::Heavy
    }

    /// Fallback wait estimate in milliseconds per audio second, used until
    /// this PC has measured the model itself.
    pub fn fallback_ms_per_s(&self, on_gpu: bool) -> f32 {
        if let Some(ms) = self.fallback_override {
            return ms;
        }
        match &self.engine {
            Engine::Parakeet => 160.0,
            Engine::MoonshineBase | Engine::MoonshineStreaming => 110.0,
            Engine::Whisper(WhisperKind::Tiny) => 260.0,
            Engine::Whisper(WhisperKind::HinglishSwift) => {
                if on_gpu {
                    300.0
                } else {
                    1_100.0
                }
            }
            Engine::Whisper(WhisperKind::Turbo | WhisperKind::HinglishApex) => {
                if on_gpu {
                    450.0
                } else {
                    2_600.0
                }
            }
            Engine::Hybrid => {
                if on_gpu {
                    500.0
                } else {
                    2_800.0
                }
            }
            Engine::CustomOnnx => 1_000.0,
        }
    }

    pub fn load(&self) -> Result<Box<dyn SpeechModel + Send>> {
        let dir = self.dir();
        let model: Box<dyn SpeechModel + Send> = match &self.engine {
            Engine::Parakeet => Box::new(
                ParakeetModel::load(&dir, &Quantization::Int8).map_err(|e| anyhow!("{e}"))?,
            ),
            Engine::MoonshineBase => Box::new(
                MoonshineModel::load(&dir, MoonshineVariant::Base, &Quantization::default())
                    .map_err(|e| anyhow!("{e}"))?,
            ),
            Engine::MoonshineStreaming => {
                let threads = thread::available_parallelism()
                    .map(|n| n.get().min(4))
                    .unwrap_or(2);
                Box::new(
                    StreamingModel::load(&dir, threads, &Quantization::default())
                        .map_err(|e| anyhow!("{e}"))?,
                )
            }
            Engine::Whisper(kind) => whisper::load(*kind)?,
            Engine::Hybrid => Box::new(whisper::Hybrid::new(
                parakeet_v3().load()?,
                whisper::load(WhisperKind::Turbo)?,
            )),
            Engine::CustomOnnx => {
                bail!("This model's layout isn't supported by the offline engine yet")
            }
        };
        Ok(model)
    }

    pub fn remove(&self) -> Result<()> {
        match &self.engine {
            Engine::Whisper(kind) => return whisper::remove(*kind),
            Engine::Hybrid => {
                bail!("Hybrid uses Parakeet V3 and Whisper turbo; delete those instead")
            }
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
                fetch_resumable(&client, url, &part, &mut |done, total| {
                    progress(done, total)
                })?;
                self.install_archive(&part)?;
                let _ = fs::remove_file(&part);
            }
            Source::Files(files) => {
                let staging = models_root().join(format!("{}.part", self.dir_name));
                fs::create_dir_all(&staging)?;
                let sizes: Vec<u64> = files
                    .iter()
                    .map(|(url, _)| remote_size(&client, url).unwrap_or(0))
                    .collect();
                let total: u64 = sizes.iter().sum();
                let mut finished = 0u64;
                for ((url, name), size) in files.iter().zip(&sizes) {
                    let dest = staging.join(name);
                    fetch_resumable(&client, url, &dest, &mut |done, _| {
                        progress(finished + done, total)
                    })?;
                    finished += (*size).max(fs::metadata(&dest).map(|m| m.len()).unwrap_or(0));
                }
                if !has_required_files(&self.engine, &staging) {
                    bail!("Downloaded files are incomplete");
                }
                let target = self.dir();
                let _ = fs::remove_dir_all(&target);
                fs::rename(&staging, &target)?;
            }
            Source::SingleFile(url, name) => {
                if url.is_empty() {
                    bail!("This model has no download URL");
                }
                let staging = models_root().join(format!("{}.part", self.dir_name));
                fs::create_dir_all(&staging)?;
                let dest = staging.join(name);
                fetch_resumable(&client, url, &dest, progress)?;
                let target = self.dir();
                let _ = fs::remove_dir_all(&target);
                fs::rename(&staging, &target)?;
            }
            Source::Whisper(kind) => whisper::download(*kind, progress)?,
            Source::Parts => {
                let parakeet = parakeet_v3();
                let mb = |n: u32| n as u64 * 1_048_576;
                let parakeet_bytes = if parakeet.is_installed() {
                    0
                } else {
                    mb(parakeet.size_mb)
                };
                let whisper_bytes = if whisper::is_installed(WhisperKind::Turbo) {
                    0
                } else {
                    mb(whisper::RUNTIME_MB + whisper::MODEL_MB)
                };
                let total = parakeet_bytes + whisper_bytes;
                if parakeet_bytes > 0 {
                    parakeet.download_with(&mut |done, _| progress(done, total))?;
                }
                if whisper_bytes > 0 {
                    whisper::download(WhisperKind::Turbo, &mut |done, _| {
                        progress(parakeet_bytes + done, total)
                    })?;
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

        let found = find_model_dir(&self.engine, &staging)
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

fn has_required_files(engine: &Engine, dir: &Path) -> bool {
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
        Engine::CustomOnnx => {
            // A downloaded single-file model is installed once its directory
            // holds at least one .onnx file.
            fs::read_dir(dir)
                .map(|entries| {
                    entries
                        .flatten()
                        .any(|e| e.path().extension().is_some_and(|x| x == "onnx"))
                })
                .unwrap_or(false)
        }
        Engine::Whisper(_) | Engine::Hybrid => false,
    }
}

fn find_model_dir(engine: &Engine, dir: &Path) -> Option<PathBuf> {
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
            log::warn!(
                "Download attempt {attempt} failed, retrying: {:?}",
                last_err
            );
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

fn parakeet_v3() -> Arc<ModelInfo> {
    find(DEFAULT_MODEL).expect("parakeet-v3 in catalog")
}

/// The model the user picked, falling back to the default.
pub fn selected_model() -> Arc<ModelInfo> {
    let id = crate::settings::load()
        .model
        .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
    find(&id).unwrap_or_else(parakeet_v3)
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
    fn uses_whisper_flags_the_right_rows() {
        // Everything whisper-backed gains the card engine when it's set up,
        // so the catalog rows and hybrid need one marker.
        assert!(find("hinglish-apex").unwrap().uses_whisper());
        assert!(find("whisper-turbo").unwrap().uses_whisper());
        assert!(find("hybrid").unwrap().uses_whisper());
        assert!(!find("parakeet-v3").unwrap().uses_whisper());
        assert!(!find("moonshine-base").unwrap().uses_whisper());
    }

    #[test]
    fn the_hinglish_models_are_in_the_catalog() {
        let apex = find("hinglish-apex").expect("hinglish-apex in the catalog");
        assert_eq!(apex.name, "Whisper Hinglish Apex");
        assert!(matches!(
            apex.engine,
            Engine::Whisper(WhisperKind::HinglishApex)
        ));
        assert_eq!(apex.size_mb, whisper::RUNTIME_MB + whisper::APEX_MODEL_MB);

        let swift = find("hinglish-swift").expect("hinglish-swift in the catalog");
        assert_eq!(swift.name, "Whisper Hinglish Swift");
        assert!(matches!(
            swift.engine,
            Engine::Whisper(WhisperKind::HinglishSwift)
        ));
        assert_eq!(swift.size_mb, whisper::RUNTIME_MB + whisper::SWIFT_MODEL_MB);
    }

    fn temp_root(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sruthi-registry-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn builtin_and_custom_entries_share_one_registry() {
        let reg = Registry::load();
        assert!(reg.find("parakeet-v3").is_some());
    }

    #[test]
    fn corrupt_custom_file_is_skipped() {
        let root = temp_root("corrupt");
        fs::write(root.join("custom_models.json"), "{ not json").unwrap();
        let reg = Registry::load_from(&root);
        assert!(reg.find("parakeet-v3").is_some());
        assert_eq!(reg.all().len(), BUILTIN.len());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_non_https_and_unknown_hosts() {
        assert!(validate_model_url("http://huggingface.co/a.onnx").is_err());
        assert!(validate_model_url("file:///C:/model.onnx").is_err());
        assert!(validate_model_url("https://evil.example/model.onnx").is_err());
    }

    #[test]
    fn accepts_known_hosts_and_strips_fragment() {
        let u =
            validate_model_url("https://huggingface.co/org/repo/resolve/main/model.onnx?x=1#frag")
                .unwrap();
        assert_eq!(
            u,
            "https://huggingface.co/org/repo/resolve/main/model.onnx?x=1"
        );
    }

    #[test]
    fn one_bad_custom_entry_does_not_drop_the_others() {
        let root = temp_root("partial");
        let good = ModelInfo::custom("custom-good", "Good", Heat::Heavy, 900.0);
        let text = format!(
            "[{}, {{\"id\":\"broken\",\"engine\":\"not-an-engine\"}}]",
            serde_json::to_string(&good).unwrap()
        );
        fs::write(root.join("custom_models.json"), text).unwrap();
        let reg = Registry::load_from(&root);
        assert!(reg.find("custom-good").is_some());
        // The bad entry is kept on disk so a later save cannot erase it.
        assert_eq!(read_custom_values_from(&root).len(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn custom_heat_and_eta_round_trip_through_json() {
        let entry = ModelInfo::custom("custom-x", "Custom", Heat::Heavy, 900.0);
        let json = serde_json::to_string(&entry).unwrap();
        let back: ModelInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.heat(), Heat::Heavy);
        assert_eq!(back.fallback_ms_per_s(false), 900.0);
    }

    #[test]
    fn custom_heat_and_eta_survive_the_owned_conversion() {
        let entry = ModelInfo::custom("custom-x", "Custom", Heat::Heavy, 900.0);
        assert_eq!(entry.heat(), Heat::Heavy);
        assert_eq!(entry.fallback_ms_per_s(false), 900.0);
    }

    /// Runs a real model on a WAV file. Needs the model installed, so it's
    /// opt-in: `SRUTHI_TEST_WAV=path cargo test --release -- --ignored`.
    #[test]
    #[ignore]
    fn transcribes_wav_with_installed_model() {
        let wav = std::env::var("SRUTHI_TEST_WAV").expect("set SRUTHI_TEST_WAV");
        let id = std::env::var("SRUTHI_TEST_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
        let info = find(&id).expect("model in catalog");
        assert!(info.is_installed(), "{id} isn't installed");

        // A directory runs every .wav in it, in name order.
        let path = Path::new(&wav);
        let mut files: Vec<PathBuf> = if path.is_dir() {
            fs::read_dir(path)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "wav"))
                .collect()
        } else {
            vec![path.to_path_buf()]
        };
        files.sort();

        // SRUTHI_TEST_GAIN=0.02 simulates quiet speech: scales the audio and
        // adds hiss, then runs it through the same boost dictation uses.
        let gain: Option<f32> = std::env::var("SRUTHI_TEST_GAIN")
            .ok()
            .and_then(|g| g.parse().ok());

        // SRUTHI_TEST_HOTWORDS="Tauri,Kubernetes" boosts those words while
        // decoding and shows the dictionary correction on top.
        let hotwords: Vec<String> = std::env::var("SRUTHI_TEST_HOTWORDS")
            .map(|w| {
                w.split(',')
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let mut model = info.load().unwrap();
        model.set_hotwords(&hotwords);
        model.set_latin_only(true);
        for file in files {
            let mut samples = transcribe_rs::audio::read_wav_samples(&file).unwrap();
            // SRUTHI_TEST_ENHANCE=1 runs the same speech check and boost as
            // dictation (useful on saved recordings).
            if std::env::var_os("SRUTHI_TEST_ENHANCE").is_some() && gain.is_none() {
                match crate::enhance::prepare(&samples) {
                    Some(boosted) => samples = boosted,
                    None => {
                        println!(
                            "[{id}] {}: rejected as no speech",
                            file.file_name().unwrap().to_string_lossy()
                        );
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
                let quiet = model
                    .transcribe(&samples, &transcribe_rs::TranscribeOptions::default())
                    .unwrap()
                    .text;
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
            let text = model
                .transcribe(&samples, &transcribe_rs::TranscribeOptions::default())
                .unwrap()
                .text;
            println!(
                "[{id}] {} ({:.1}s audio, {} ms)
  raw:   {text}
  rules: {}",
                file.file_name().unwrap().to_string_lossy(),
                samples.len() as f32 / 16000.0,
                started.elapsed().as_millis(),
                crate::format::rules::apply(&text)
            );
            if !hotwords.is_empty() {
                println!(
                    "  dict:  {}",
                    crate::format::dictionary::apply(&text, &hotwords)
                );
            }
            if text.trim().is_empty() {
                println!("  (model heard nothing)");
            }
        }
    }
}
