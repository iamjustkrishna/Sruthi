//! Whisper models through whisper.cpp: large-v3-turbo (pinned to English,
//! also the Hybrid second opinion for Parakeet), tiny (multilingual, for a
//! first look at Hinglish), and Oriserve's Hindi->Hinglish fine-tunes
//! (Apex, Swift), which type Romanized Hinglish directly.
//!
//! Whisper understands whispers and accents far better than Parakeet, but
//! its encoder is heavy. Two ways to run it:
//! - On the graphics card (a Vulkan build of whisper.cpp in
//!   `runtime-vulkan`): a whisper-server that stays loaded and uses the full
//!   30 s window, the way Whisper was trained.
//! - On the CPU: a full window took ~9-17 s here, so each run starts
//!   whisper-cli with a window sized to the clip (`audio_ctx`), ~3-4 s for a
//!   3 s whisper. A persistent server can't do that: changing the window per
//!   request produced garbage.

use std::fs::{self, File};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use transcribe_rs::{
    ModelCapabilities, SpeechModel, TranscribeError, TranscribeOptions, TranscriptionResult,
};

use crate::audio::TARGET_RATE;

const RUNTIME_URL: &str =
    "https://github.com/ggml-org/whisper.cpp/releases/download/b5454/whisper-blas-bin-x64.zip";
pub const RUNTIME_MB: u32 = 21;
pub const MODEL_MB: u32 = 547;
pub const TINY_MODEL_MB: u32 = 75;
pub const APEX_MODEL_MB: u32 = 834;
pub const SWIFT_MODEL_MB: u32 = 57;

/// Which of the whisper.cpp models a catalog entry loads.
#[derive(Clone, Copy)]
pub enum WhisperKind {
    /// Whisper large-v3-turbo: the "second opinion" model, pinned to English.
    Turbo,
    /// Whisper tiny, multilingual: a first look at Hinglish. It detects the
    /// language itself, per clip.
    Tiny,
    /// Oriserve's fine-tune of large-v3-turbo (Apex) for Hindi-English
    /// code-switching. Types Romanized Hinglish ("Main aaj subah office
    /// ja raha hoon"), not Devanagari.
    HinglishApex,
    /// Oriserve's Whisper-base fine-tune (Swift): the small, quick version
    /// of the same Hinglish idea.
    HinglishSwift,
}

impl WhisperKind {
    fn model_url(self) -> &'static str {
        match self {
            Self::Turbo => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin"
            }
            Self::Tiny => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
            Self::HinglishApex => {
                "https://huggingface.co/Marquestra/Whisper-Hindi2Hinglish-Apex-GGML/resolve/main/ggml-apex-hinglish-q8_0.bin"
            }
            Self::HinglishSwift => {
                "https://huggingface.co/rohitag13/whisper-hindi2hinglish-swift-GGUF/resolve/main/ggml-hinglish-swift-q5_1.bin"
            }
        }
    }

    fn model_file(self) -> &'static str {
        match self {
            Self::Turbo => "ggml-large-v3-turbo-q5_0.bin",
            Self::Tiny => "ggml-tiny.bin",
            Self::HinglishApex => "ggml-apex-hinglish-q8_0.bin",
            Self::HinglishSwift => "ggml-hinglish-swift-q5_1.bin",
        }
    }

    fn model_mb(self) -> u32 {
        match self {
            Self::Turbo => MODEL_MB,
            Self::Tiny => TINY_MODEL_MB,
            Self::HinglishApex => APEX_MODEL_MB,
            Self::HinglishSwift => SWIFT_MODEL_MB,
        }
    }

    /// Turbo reads English. Tiny is the Hinglish test model: no flag, so
    /// whisper.cpp detects en/hi itself per clip, and the output shows
    /// what that gets us. The Oriserve fine-tunes also decode under the
    /// English token - their cards say auto or "hi" puts the output back
    /// in Devanagari.
    fn language_args(self) -> &'static [&'static str] {
        match self {
            Self::Turbo | Self::HinglishApex | Self::HinglishSwift => &["-l", "en"],
            Self::Tiny => &[],
        }
    }

    /// Whether the user's dictionary should go in as the initial prompt.
    /// The Oriserve fine-tunes shouldn't get it: an English word list
    /// drags their spellings back to English ("office mein" became
    /// "officemen" in OpenWispr's testing).
    fn accepts_prompt(self) -> bool {
        match self {
            Self::HinglishApex | Self::HinglishSwift => false,
            Self::Turbo | Self::Tiny => true,
        }
    }

    /// Short label for status and error messages ("Whisper turbo isn't
    /// downloaded").
    pub fn label(self) -> &'static str {
        match self {
            Self::Turbo => "turbo",
            Self::Tiny => "tiny",
            Self::HinglishApex => "hinglish-apex",
            Self::HinglishSwift => "hinglish-swift",
        }
    }
}

/// Whisper's full window, in encoder positions (50 per second, 30 s).
const FULL_AUDIO_CTX: u32 = 1500;
/// Shortest window used: about 7.7 s. Smaller windows hallucinated.
const MIN_AUDIO_CTX: u32 = 384;

fn capabilities(kind: WhisperKind) -> ModelCapabilities {
    match kind {
        WhisperKind::Turbo => ModelCapabilities {
            name: "Whisper large-v3-turbo",
            engine_id: "whisper_cli",
            sample_rate: TARGET_RATE,
            languages: &["en"],
            supports_timestamps: false,
            supports_translation: false,
            supports_streaming: false,
        },
        WhisperKind::Tiny => ModelCapabilities {
            name: "Whisper tiny",
            engine_id: "whisper_cli",
            sample_rate: TARGET_RATE,
            languages: &["en", "hi"],
            supports_timestamps: false,
            supports_translation: false,
            supports_streaming: false,
        },
        WhisperKind::HinglishApex => ModelCapabilities {
            name: "Whisper Hinglish Apex",
            engine_id: "whisper_cli",
            sample_rate: TARGET_RATE,
            languages: &["hi", "en"],
            supports_timestamps: false,
            supports_translation: false,
            supports_streaming: false,
        },
        WhisperKind::HinglishSwift => ModelCapabilities {
            name: "Whisper Hinglish Swift",
            engine_id: "whisper_cli",
            sample_rate: TARGET_RATE,
            languages: &["hi", "en"],
            supports_timestamps: false,
            supports_translation: false,
            supports_streaming: false,
        },
    }
}

fn root() -> PathBuf {
    crate::settings::app_root().join("whisper")
}

fn runtime_dir() -> PathBuf {
    root().join("runtime")
}

fn cli_path() -> PathBuf {
    runtime_dir().join(if cfg!(windows) {
        "whisper-cli.exe"
    } else {
        "whisper-cli"
    })
}

/// whisper.cpp built with Vulkan, for the graphics card. The official
/// releases have no such build for Windows, so it's built locally for now
/// (see docs/GPU.md).
fn gpu_runtime_dir() -> PathBuf {
    root().join("runtime-vulkan")
}

fn gpu_server_path() -> PathBuf {
    gpu_runtime_dir().join(if cfg!(windows) {
        "whisper-server.exe"
    } else {
        "whisper-server"
    })
}

fn model_path(kind: WhisperKind) -> PathBuf {
    root().join("models").join(kind.model_file())
}

/// Is any whisper model file left in the shared models folder? Removing
/// the last one takes the runtime with it.
fn model_left_in(models: &Path) -> bool {
    [
        WhisperKind::Turbo,
        WhisperKind::Tiny,
        WhisperKind::HinglishApex,
        WhisperKind::HinglishSwift,
    ]
    .iter()
    .any(|k| models.join(k.model_file()).is_file())
}

pub fn is_installed(kind: WhisperKind) -> bool {
    cli_path().is_file() && model_path(kind).is_file()
}

/// Downloads whatever is missing: the whisper.cpp tools (shared by both
/// models) and the one model.
pub fn download(kind: WhisperKind, progress: &mut dyn FnMut(u64, u64)) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(20))
        .build()?;
    let runtime_bytes = if cli_path().is_file() {
        0
    } else {
        RUNTIME_MB as u64 * 1_048_576
    };
    let total = runtime_bytes + kind.model_mb() as u64 * 1_048_576;

    if !cli_path().is_file() {
        fs::create_dir_all(root())?;
        let zip_path = root().join("runtime.zip.part");
        crate::model::fetch_resumable(&client, RUNTIME_URL, &zip_path, &mut |done, _| {
            progress(done, total)
        })?;
        install_runtime(&zip_path)?;
        let _ = fs::remove_file(&zip_path);
    }
    if !model_path(kind).is_file() {
        fs::create_dir_all(root().join("models"))?;
        let part = root()
            .join("models")
            .join(format!("{}.part", kind.model_file()));
        crate::model::fetch_resumable(&client, kind.model_url(), &part, &mut |done, _| {
            progress(runtime_bytes + done, total)
        })?;
        fs::rename(&part, model_path(kind))?;
    }
    progress(total, total);
    Ok(())
}

fn install_runtime(zip_path: &Path) -> Result<()> {
    let staging = root().join("runtime._extract");
    let _ = fs::remove_dir_all(&staging);
    zip::ZipArchive::new(File::open(zip_path)?)?
        .extract(&staging)
        .context("Failed to unpack whisper.cpp")?;
    let exe = cli_path().file_name().unwrap().to_owned();
    let dir = find_dir_with(&staging, &exe)
        .ok_or_else(|| anyhow!("whisper-cli not found in download"))?;
    let _ = fs::remove_dir_all(runtime_dir());
    fs::rename(&dir, runtime_dir())?;
    let _ = fs::remove_dir_all(&staging);
    Ok(())
}

fn find_dir_with(dir: &Path, file: &std::ffi::OsStr) -> Option<PathBuf> {
    if dir.join(file).is_file() {
        return Some(dir.to_path_buf());
    }
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_dir())
        .find_map(|e| find_dir_with(&e.path(), file))
}

/// Removes the model's file; the shared runtime goes away too once no
/// model is left that would still run on it.
pub fn remove(kind: WhisperKind) -> Result<()> {
    let models = root().join("models");
    let file = model_path(kind);
    if file.is_file() {
        fs::remove_file(&file)?;
    }
    if !model_left_in(&models) {
        for dir in [runtime_dir(), gpu_runtime_dir()] {
            if dir.exists() {
                fs::remove_dir_all(dir)?;
            }
        }
        if fs::read_dir(&models)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
        {
            let _ = fs::remove_dir(&models);
        }
        if fs::read_dir(root())
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
        {
            let _ = fs::remove_dir(root());
        }
    }
    Ok(())
}

/// Encoder window for a clip: 1.5x its length plus 3 s of slack, at least
/// ~7.7 s, and the full 30 s (0 = "all") for long dictations.
fn audio_ctx(seconds: f32) -> u32 {
    let positions = ((seconds * 1.5 + 3.0) * 50.0).ceil() as u32;
    let rounded = positions.div_ceil(64) * 64;
    if rounded >= FULL_AUDIO_CTX {
        0
    } else {
        rounded.max(MIN_AUDIO_CTX)
    }
}

/// Longest a Whisper run may take: about 1 s to load, ~1.5x the audio to
/// process on this CPU, and slack.
fn time_budget(seconds: f32) -> Duration {
    Duration::from_secs_f32((5.0 + seconds * 1.5).min(45.0))
}

pub struct WhisperCli {
    kind: WhisperKind,
    threads: usize,
    /// The user's dictionary as a hint ("initial prompt"), the way Wispr
    /// biases its models: Whisper then spells those words as given.
    prompt: Option<String>,
}

impl WhisperCli {
    pub fn new(kind: WhisperKind) -> Result<Self> {
        if !is_installed(kind) {
            return Err(anyhow!("Whisper {} isn't downloaded", kind.label()));
        }
        // One thread per physical core: on a 6-core/12-thread CPU, 6
        // threads took 4.2 s where 10 took 8.0 s (SMT threads just contend).
        let threads = std::thread::available_parallelism()
            .map(|n| (n.get() / 2).max(4))
            .unwrap_or(4);
        Ok(Self {
            kind,
            threads,
            prompt: None,
        })
    }

    fn run(&self, samples: &[f32]) -> Result<String> {
        let wav = root().join(format!("dictation-{}.wav", std::process::id()));
        fs::write(&wav, crate::recordings::encode_wav(samples))?;
        let seconds = samples.len() as f32 / TARGET_RATE as f32;
        let ctx = audio_ctx(seconds);

        let mut cmd = Command::new(cli_path());
        cmd.arg("-m")
            .arg(model_path(self.kind))
            .arg("-f")
            .arg(&wav)
            .args(self.kind.language_args())
            .args([
                "-nt",
                "-np",
                "-t",
                &self.threads.to_string(),
                "-ac",
                &ctx.to_string(),
            ])
            // Greedy decoding, no temperature retries: ~3.6 s instead of
            // 4-8 s on short whispers, with the same text. Retries mostly
            // re-ran hallucination loops.
            .args(["-bs", "1", "-bo", "1", "-nf"])
            .current_dir(runtime_dir())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(prompt) = &self.prompt {
            cmd.arg("--prompt").arg(prompt);
        }
        crate::process::hide_window(&mut cmd);
        let mut child = cmd.spawn().context("Couldn't start whisper.cpp")?;
        crate::process::kill_with_app(&child);

        // A run past its budget is stuck repeating itself ("Shrutam.
        // Shrutam. ..." until the window ends): stop it and fall back.
        let deadline = std::time::Instant::now() + time_budget(seconds);
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break Some(status);
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        let _ = fs::remove_file(&wav);
        let status =
            status.ok_or_else(|| anyhow!("Whisper took too long ({seconds:.1}s of audio)"))?;
        if !status.success() {
            return Err(anyhow!("whisper.cpp failed ({status})"));
        }
        let mut stdout = String::new();
        if let Some(mut out) = child.stdout.take() {
            use std::io::Read;
            out.read_to_string(&mut stdout)?;
        }
        let text = stdout;
        let joined = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let text = drop_repeated_sentences(&joined);
        Ok(if echoes_prompt(&text, self.prompt.as_deref()) {
            String::new()
        } else {
            text
        })
    }
}

/// On short or silent clips Whisper can return its prompt instead of what
/// was said. A one-word prompt can't be told apart from saying that word,
/// so only multi-word prompts are checked.
fn echoes_prompt(text: &str, prompt: Option<&str>) -> bool {
    let norm = |s: &str| {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_lowercase()
    };
    prompt.is_some_and(|p| p.contains(',') && !text.is_empty() && norm(text) == norm(p))
}

/// On short clips Whisper often says the same sentence twice ("Don't you
/// know about this? Don't you know about this?"). Back-to-back identical
/// sentences are almost never what someone dictated, so keep one.
fn drop_repeated_sentences(text: &str) -> String {
    let mut sentences: Vec<&str> = Vec::new();
    let mut start = 0;
    for (i, c) in text.char_indices() {
        if matches!(c, '.' | '?' | '!') {
            sentences.push(text[start..=i].trim());
            start = i + 1;
        }
    }
    if start < text.len() {
        sentences.push(text[start..].trim());
    }
    let norm = |s: &str| {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_lowercase()
    };
    let mut out: Vec<&str> = Vec::new();
    for s in sentences.into_iter().filter(|s| !s.is_empty()) {
        if out.last().is_some_and(|last| norm(last) == norm(s)) {
            continue;
        }
        out.push(s);
    }
    out.join(" ")
}

/// Whisper on the graphics card when that works, otherwise on the CPU.
/// Tiny stays on the CPU: it's small enough to be quick there, and the
/// server only loads turbo's model, so the GPU path is turbo-only.
pub fn load(kind: WhisperKind) -> Result<Box<dyn SpeechModel + Send>> {
    if !is_installed(kind) {
        return Err(anyhow!("Whisper {} isn't downloaded", kind.label()));
    }
    if matches!(kind, WhisperKind::Turbo)
        && gpu_server_path().is_file()
        && std::env::var_os("SRUTHI_WHISPER_CPU").is_none()
    {
        match WhisperServer::start() {
            Ok(server) => return Ok(Box::new(server)),
            Err(e) => log::warn!("Whisper on the graphics card didn't start, using the CPU: {e:#}"),
        }
    }
    Ok(Box::new(WhisperCli::new(kind)?))
}

/// whisper-server on the graphics card: the model stays loaded, so a
/// dictation only pays for the work itself.
pub struct WhisperServer {
    child: Child,
    base: String,
    client: reqwest::blocking::Client,
    prompt: Option<String>,
}

impl WhisperServer {
    pub fn start() -> Result<Self> {
        let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
        // The server loads turbo's model, so the GPU path is turbo-only.
        let mut cmd = Command::new(gpu_server_path());
        cmd.arg("-m")
            .arg(model_path(WhisperKind::Turbo))
            .args(["--host", "127.0.0.1", "--port", &port.to_string()])
            // The GPU does the heavy lifting; a few CPU threads are enough.
            .args(["-t", "4", "-l", "en", "-nt", "-bs", "1", "-bo", "1", "-nf"])
            .current_dir(gpu_runtime_dir())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        crate::gpu::use_dedicated(&mut cmd);
        crate::process::hide_window(&mut cmd);
        let child = cmd.spawn().context("Couldn't start whisper-server")?;
        crate::process::kill_with_app(&child);
        let mut server = Self {
            child,
            base: format!("http://127.0.0.1:{port}"),
            client: reqwest::blocking::Client::builder().build()?,
            prompt: None,
        };
        server.wait_ready(Duration::from_secs(60))?;
        // The first run on the GPU sets up its shaders; do that now, not
        // during the first dictation.
        let warm = Instant::now();
        server.request(&vec![0.0; TARGET_RATE as usize], Duration::from_secs(60))?;
        log::info!(
            "Whisper turbo ready on the graphics card (warm-up {} ms)",
            warm.elapsed().as_millis()
        );
        Ok(server)
    }

    fn wait_ready(&mut self, timeout: Duration) -> Result<()> {
        // The server only starts listening once the model is loaded.
        let started = Instant::now();
        while started.elapsed() < timeout {
            if let Some(status) = self.child.try_wait()? {
                return Err(anyhow!("whisper-server exited ({status})"));
            }
            if self
                .client
                .get(&self.base)
                .timeout(Duration::from_secs(2))
                .send()
                .is_ok()
            {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Err(anyhow!("whisper-server didn't start in time"))
    }

    fn request(&self, samples: &[f32], timeout: Duration) -> Result<String> {
        let wav = reqwest::blocking::multipart::Part::bytes(crate::recordings::encode_wav(samples))
            .file_name("dictation.wav")
            .mime_str("audio/wav")?;
        let mut form = reqwest::blocking::multipart::Form::new()
            .part("file", wav)
            .text("response_format", "text")
            .text("temperature", "0")
            .text("temperature_inc", "0")
            .text("no_timestamps", "true");
        if let Some(prompt) = &self.prompt {
            form = form.text("prompt", prompt.clone());
        }
        let resp = self
            .client
            .post(format!("{}/inference", self.base))
            .multipart(form)
            .timeout(timeout)
            .send()?;
        if !resp.status().is_success() {
            return Err(anyhow!("whisper-server answered {}", resp.status()));
        }
        Ok(resp.text()?)
    }

    fn run(&mut self, samples: &[f32]) -> Result<String> {
        let seconds = samples.len() as f32 / TARGET_RATE as f32;
        // A stuck run (a repetition loop) would also hold up the next
        // dictation, so give up on it and restart the server.
        let text = match self.request(samples, gpu_time_budget(seconds)) {
            Ok(text) => text,
            Err(e) => {
                let _ = self.child.kill();
                let prompt = self.prompt.take();
                *self = Self::start()?;
                self.prompt = prompt;
                return Err(e.context(format!(
                    "Whisper on the graphics card failed ({seconds:.1}s of audio)"
                )));
            }
        };
        let joined = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let text = drop_repeated_sentences(&joined);
        Ok(if echoes_prompt(&text, self.prompt.as_deref()) {
            String::new()
        } else {
            text
        })
    }
}

impl Drop for WhisperServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Longest a GPU run may take before it's treated as stuck.
fn gpu_time_budget(seconds: f32) -> Duration {
    Duration::from_secs_f32((4.0 + seconds * 0.5).min(30.0))
}

impl SpeechModel for WhisperServer {
    fn capabilities(&self) -> ModelCapabilities {
        capabilities(WhisperKind::Turbo)
    }

    fn set_hotwords(&mut self, words: &[String]) {
        self.prompt = prompt_for(words);
    }

    fn transcribe_raw(
        &mut self,
        samples: &[f32],
        _options: &TranscribeOptions,
    ) -> Result<TranscriptionResult, TranscribeError> {
        let started = Instant::now();
        let text = self
            .run(samples)
            .map_err(|e| TranscribeError::Inference(format!("{e:#}")))?;
        crate::timing::add("whisper-gpu", started.elapsed());
        log::info!(
            "Whisper turbo (GPU): {:.1}s of audio in {} ms",
            samples.len() as f32 / TARGET_RATE as f32,
            started.elapsed().as_millis()
        );
        Ok(TranscriptionResult {
            text,
            segments: None,
        })
    }
}

/// The user's dictionary as Whisper's prompt, the way Wispr biases its
/// models. Just the words: a label like "Glossary:" got echoed back on
/// short clips.
fn prompt_for(words: &[String]) -> Option<String> {
    let words: Vec<&str> = words
        .iter()
        .map(|w| w.trim())
        .filter(|w| !w.is_empty())
        .collect();
    (!words.is_empty()).then(|| format!("{}.", words.join(", ")))
}

impl SpeechModel for WhisperCli {
    fn capabilities(&self) -> ModelCapabilities {
        capabilities(self.kind)
    }

    fn set_hotwords(&mut self, words: &[String]) {
        // The Oriserve fine-tunes shouldn't see the dictionary (see
        // accepts_prompt).
        self.prompt = if self.kind.accepts_prompt() {
            prompt_for(words)
        } else {
            None
        };
    }

    fn transcribe_raw(
        &mut self,
        samples: &[f32],
        _options: &TranscribeOptions,
    ) -> Result<TranscriptionResult, TranscribeError> {
        let started = std::time::Instant::now();
        let text = self
            .run(samples)
            .map_err(|e| TranscribeError::Inference(e.to_string()))?;
        crate::timing::add("whisper", started.elapsed());
        log::info!(
            "Whisper {}: {:.1}s of audio in {} ms",
            self.kind.label(),
            samples.len() as f32 / TARGET_RATE as f32,
            started.elapsed().as_millis()
        );
        Ok(TranscriptionResult {
            text,
            segments: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Hybrid

const HYBRID_CAPABILITIES: ModelCapabilities = ModelCapabilities {
    name: "Hybrid (Parakeet + Whisper)",
    engine_id: "hybrid",
    sample_rate: TARGET_RATE,
    languages: &["en"],
    supports_timestamps: false,
    supports_translation: false,
    supports_streaming: false,
};

/// Normal speech runs ~2-3 words per second of actual speaking. Far fewer
/// words than that means the fast model missed most of it (typical for
/// whispers: 3 s of whispering came back as "Yeah.").
const MIN_WORDS_PER_SECOND: f32 = 1.0;
/// Below this much speech, there's too little to judge.
const MIN_SPEECH_SECS: f32 = 0.4;

/// Parakeet first (fast); Whisper turbo only when Parakeet clearly missed
/// what was said.
pub struct Hybrid {
    fast: Box<dyn SpeechModel + Send>,
    slow: Box<dyn SpeechModel + Send>,
    /// The user's dictionary, to spot misheard dictionary words.
    words: Vec<String>,
}

impl Hybrid {
    pub fn new(fast: Box<dyn SpeechModel + Send>, slow: Box<dyn SpeechModel + Send>) -> Self {
        Self {
            fast,
            slow,
            words: Vec::new(),
        }
    }
}

impl SpeechModel for Hybrid {
    fn capabilities(&self) -> ModelCapabilities {
        HYBRID_CAPABILITIES
    }

    fn set_hotwords(&mut self, words: &[String]) {
        self.fast.set_hotwords(words);
        self.slow.set_hotwords(words);
        self.words = words.to_vec();
    }

    fn set_latin_only(&mut self, on: bool) {
        self.fast.set_latin_only(on);
    }

    fn transcribe_raw(
        &mut self,
        samples: &[f32],
        options: &TranscribeOptions,
    ) -> Result<TranscriptionResult, TranscribeError> {
        // `transcribe` (not `_raw`) keeps Parakeet's own silence padding.
        let fast = crate::timing::measure("parakeet", || self.fast.transcribe(samples, options))?;
        let speech = speech_seconds(samples);
        // Whisper also gets a say when Parakeet's words sound close to, but
        // not quite like, a dictionary word: with the dictionary as its
        // prompt, Whisper heard "Sruthi" where Parakeet wrote "Shop".
        let missed_words = needs_second_opinion(&fast.text, speech);
        let near_miss =
            !missed_words && crate::format::dictionary::has_near_miss(&fast.text, &self.words);
        if !missed_words && !near_miss {
            return Ok(fast);
        }
        log::info!(
            "Hybrid: Parakeet heard {:?} in {speech:.1}s of speech; asking Whisper",
            fast.text
        );
        let slow = self.slow.transcribe_raw(samples, options)?;
        let use_slow = if missed_words {
            !slow.text.trim().is_empty()
        } else {
            // Asked about a possible name: only worth it if Whisper found one
            // (on "Right." it answered "Aja, Aja." after hearing "Ajay").
            crate::format::dictionary::mentions_entry(&slow.text, &self.words)
        };
        Ok(if use_slow { slow } else { fast })
    }
}

fn needs_second_opinion(text: &str, speech_secs: f32) -> bool {
    if speech_secs < MIN_SPEECH_SECS {
        return false;
    }
    // A number like "987-6543210" is one token but ten spoken digits, so
    // digits count as words too (two digits ~ one word of speaking time).
    let words: f32 = text
        .split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .map(|w| (w.chars().filter(char::is_ascii_digit).count() as f32 / 2.0).max(1.0))
        .sum();
    words == 0.0 || words / speech_secs < MIN_WORDS_PER_SECOND
}

/// Seconds of actual speaking: 20 ms frames louder than a tenth of the
/// loud frames (pauses don't count).
fn speech_seconds(samples: &[f32]) -> f32 {
    let frame = TARGET_RATE as usize / 50;
    let mut levels: Vec<f32> = samples.chunks(frame).map(crate::audio::rms).collect();
    if levels.is_empty() {
        return 0.0;
    }
    let mut sorted = levels.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let loud = sorted[(sorted.len() * 9) / 10];
    levels.retain(|&l| l > loud * 0.1 && l > 1e-4);
    levels.len() as f32 / 50.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiny_leaves_language_to_auto_detection() {
        // Turbo dicts in pinned English; tiny is the Hinglish test model:
        // no flag, so whisper.cpp detects en/hi per clip.
        assert_eq!(WhisperKind::Turbo.language_args(), &["-l", "en"]);
        assert!(WhisperKind::Tiny.language_args().is_empty());
    }

    #[test]
    fn runtime_outlives_models_until_the_last_one_goes() {
        let dir = std::env::temp_dir().join(format!("sruthi-whisper-test-{}", std::process::id()));
        let models = dir.join("models");
        std::fs::create_dir_all(&models).unwrap();
        for kind in [
            WhisperKind::Turbo,
            WhisperKind::Tiny,
            WhisperKind::HinglishApex,
            WhisperKind::HinglishSwift,
        ] {
            std::fs::write(models.join(kind.model_file()), b"x").unwrap();
        }

        // Removing turbo while other models are installed keeps the runtime.
        std::fs::remove_file(models.join(WhisperKind::Turbo.model_file())).unwrap();
        assert!(model_left_in(&models));

        std::fs::remove_file(models.join(WhisperKind::Tiny.model_file())).unwrap();
        std::fs::remove_file(models.join(WhisperKind::HinglishApex.model_file())).unwrap();
        std::fs::remove_file(models.join(WhisperKind::HinglishSwift.model_file())).unwrap();
        assert!(!model_left_in(&models));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hinglish_models_stay_english_pinned_and_unprompted() {
        // Oriserve's cards: decoding under auto or "hi" puts the output
        // back in Devanagari, and an English glossary as the initial
        // prompt drags the spellings back to English ("office mein"
        // became "officemen"). So: pinned "en", no dictionary prompt.
        assert_eq!(WhisperKind::HinglishApex.language_args(), &["-l", "en"]);
        assert_eq!(WhisperKind::HinglishSwift.language_args(), &["-l", "en"]);
        assert!(!WhisperKind::HinglishApex.accepts_prompt());
        assert!(!WhisperKind::HinglishSwift.accepts_prompt());
        assert!(WhisperKind::Turbo.accepts_prompt());
        assert!(WhisperKind::Tiny.accepts_prompt());
    }

    #[test]
    fn labels_read_like_the_offers() {
        assert_eq!(WhisperKind::Turbo.label(), "turbo");
        assert_eq!(WhisperKind::Tiny.label(), "tiny");
        assert_eq!(WhisperKind::HinglishApex.label(), "hinglish-apex");
        assert_eq!(WhisperKind::HinglishSwift.label(), "hinglish-swift");
    }

    #[test]
    fn window_fits_the_clip() {
        assert_eq!(audio_ctx(3.0), MIN_AUDIO_CTX); // short whisper: ~7.7 s
        assert_eq!(audio_ctx(8.0), 768); // 15 s window
        assert_eq!(audio_ctx(25.0), 0); // long: full window
    }

    #[test]
    fn second_opinion_when_words_are_missing() {
        // Real case: 3 s of whispering came back as "Yeah.".
        assert!(needs_second_opinion("Yeah.", 3.0));
        assert!(needs_second_opinion("", 2.0));
        assert!(!needs_second_opinion("There's a garden in my house.", 2.5));
        // Numbers take long to say: 4 tokens, but plenty of speech.
        assert!(!needs_second_opinion("Call me at 987-6543210.", 3.5));
        // Too little speech to judge (a tap, a cough).
        assert!(!needs_second_opinion("", 0.2));
    }

    #[test]
    fn repeated_sentences_are_dropped() {
        // Real Whisper outputs on short whispered clips.
        assert_eq!(
            drop_repeated_sentences(
                "Don't you know about whisper flow? Don't you know about whisper flow?"
            ),
            "Don't you know about whisper flow?"
        );
        assert_eq!(drop_repeated_sentences("Yes. No. Yes."), "Yes. No. Yes.");
        assert_eq!(
            drop_repeated_sentences("No trailing stop"),
            "No trailing stop"
        );
    }

    #[test]
    fn prompt_echoes_are_dropped() {
        let prompt = Some("Sruthi, Kubernetes.");
        assert!(echoes_prompt("Sruthi, Kubernetes.", prompt));
        assert!(!echoes_prompt("Sruthi is the best.", prompt));
        assert!(!echoes_prompt("Sruthi.", Some("Sruthi.")));
    }

    /// Times each stage on the newest saved recordings: Parakeet, Whisper
    /// on the graphics card, Whisper on the CPU, and the AI clean-up.
    /// `cargo test --release --lib stage_speed -- --ignored --nocapture`
    /// (SRUTHI_BENCH_COUNT=8 picks how many recordings.)
    #[test]
    #[ignore]
    fn stage_speed() {
        let _ = env_logger::builder()
            .is_test(true)
            .filter_level(log::LevelFilter::Info)
            .try_init();
        let count: usize = std::env::var("SRUTHI_BENCH_COUNT")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(8);
        let words = crate::settings::load().dictionary;
        let mut files: Vec<_> = fs::read_dir(crate::recordings::folder())
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "wav"))
            .collect();
        files.sort();
        files.reverse();
        files.truncate(count);

        let mut parakeet = crate::model::find("parakeet-v3").unwrap().load().unwrap();
        parakeet.set_hotwords(&words);
        parakeet.set_latin_only(true);
        let started = Instant::now();
        let mut gpu = WhisperServer::start().expect("GPU Whisper");
        println!(
            "GPU Whisper loaded and warmed up in {} ms",
            started.elapsed().as_millis()
        );
        gpu.set_hotwords(&words);
        let mut cpu = WhisperCli::new(WhisperKind::Turbo).unwrap();
        cpu.set_hotwords(&words);
        let formatter = crate::format::Formatter::default();
        let llm = crate::format::llm::find(crate::format::llm::DEFAULT_LLM).unwrap();
        formatter.set_server(Some(crate::format::llm::LlmServer::start(llm).unwrap()));

        let options = TranscribeOptions::default();
        for file in files {
            let raw = transcribe_rs::audio::read_wav_samples(&file).unwrap();
            let Some(samples) = crate::enhance::prepare(&raw) else {
                continue;
            };
            let secs = samples.len() as f32 / TARGET_RATE as f32;
            let time = |f: &mut dyn FnMut() -> String| {
                let t = Instant::now();
                let text = f();
                (text, t.elapsed().as_millis())
            };
            let (p_text, p_ms) =
                time(&mut || parakeet.transcribe(&samples, &options).unwrap().text);
            let (g_text, g_ms) = time(&mut || {
                gpu.transcribe_raw(&samples, &options)
                    .map(|r| r.text)
                    .unwrap_or_else(|e| format!("<{e}>"))
            });
            // SRUTHI_BENCH_SKIP_CPU=1 skips the (slow) CPU Whisper run.
            let skip_cpu = std::env::var_os("SRUTHI_BENCH_SKIP_CPU").is_some();
            let (c_text, c_ms) = if skip_cpu {
                (String::from("(skipped)"), 0)
            } else {
                time(&mut || {
                    cpu.transcribe_raw(&samples, &options)
                        .map(|r| r.text)
                        .unwrap_or_else(|e| format!("<{e}>"))
                })
            };
            let (out, ai_ms) = time(&mut || {
                formatter.format(&p_text, true, &words, crate::context::AppKind::Other)
            });
            println!(
                "\n{} ({secs:.1}s)\n  parakeet {p_ms:>5} ms  {p_text}\n  gpu      {g_ms:>5} ms  {g_text}\n  cpu      {c_ms:>5} ms  {c_text}\n  format   {ai_ms:>5} ms  {out}",
                file.file_name().unwrap().to_string_lossy()
            );
        }
    }

    #[test]
    fn speech_time_ignores_pauses() {
        let mut audio = vec![0.0f32; TARGET_RATE as usize]; // 1 s pause
        audio.extend(vec![0.1f32; TARGET_RATE as usize * 2]); // 2 s "speech"
        let secs = speech_seconds(&audio);
        assert!((1.9..=2.1).contains(&secs), "{secs}");
    }
}
