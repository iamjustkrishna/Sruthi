mod audio;
mod benchmark;
mod caret;
mod chunks;
mod context;
mod dictation;
mod enhance;
mod format;
mod gpu;
mod holdkey;
mod memory;
mod mic_check;
mod model;
mod paste;
mod process;
mod recordings;
mod secrets;
mod settings;
mod stats;
mod timing;
#[cfg(windows)]
mod wasapi;
mod whisper;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use format::cloud::{self, Cloud, Provider};
use format::llm::{self, LlmInfo, LlmServer};
use serde::Serialize;
use settings::OverlayPosition;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, RunEvent, State, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

/// Push-to-talk key when the user hasn't picked one.
const DEFAULT_HOTKEY: &str = "Ctrl+Shift+Space";
const HISTORY_LIMIT: usize = 50;
/// How long the pill shows its check mark after the text lands.
const DONE_LINGER: Duration = Duration::from_millis(650);
/// Matches the exit animation in overlay.css.
const EXIT_ANIMATION: Duration = Duration::from_millis(180);
/// Gap between the text field and the pill, in logical pixels.
const CARET_GAP: f64 = 10.0;
/// Space between the pill and the taskbar in bottom mode, in logical pixels.
const BOTTOM_MARGIN: f64 = 36.0;

#[derive(Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum ModelState {
    Missing,
    Downloading,
    Loading,
    Ready,
    Error,
}

#[derive(Clone, Serialize)]
struct ModelEntry {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    languages: &'static str,
    size_mb: u32,
    installed: bool,
}

#[derive(Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum LlmState {
    /// Smart formatting is switched off.
    Off,
    Missing,
    Downloading,
    Starting,
    Ready,
    Error,
}

#[derive(Clone, Serialize)]
struct LlmEntry {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    size_mb: u32,
    /// Includes the llama.cpp runtime when it isn't installed yet.
    download_mb: u32,
    installed: bool,
}

#[derive(Clone, Serialize)]
struct LlmStatus {
    state: LlmState,
    detail: Option<String>,
    active: &'static str,
    models: Vec<LlmEntry>,
    downloading: Option<&'static str>,
}

#[derive(Clone, Serialize)]
struct Status {
    /// State of the active model.
    model: ModelState,
    detail: Option<String>,
    active: &'static str,
    models: Vec<ModelEntry>,
    /// Model currently downloading, if any.
    downloading: Option<&'static str>,
    overlay: OverlayPosition,
    smart: bool,
    llm: LlmStatus,
    cloud: CloudStatus,
    dictionary: Vec<String>,
    snippets: Vec<format::snippets::Snippet>,
    hotkey: String,
    instant_mic: bool,
    auto_enter: bool,
    send_key: settings::SendKey,
    send_rules: Vec<settings::SendRule>,
    /// The window palette choice.
    theme: settings::Theme,
    microphone: Option<String>,
    microphones: Vec<String>,
    default_microphone: Option<String>,
    /// Shown in the main window when the microphone delivers no sound.
    mic_warning: Option<String>,
    /// The install state of the graphics card, compared with the device
    /// choice, for the window's hint lines.
    gpu: GpuStatus,
    device: settings::Device,
    /// True while the dictation engine itself runs on the card.
    engine_on_gpu: bool,
    /// Why this load went to the CPU after all (from `whisper::take_load_note`).
    gpu_note: Option<String>,
    /// The unanswered memory-gate question, if any.
    memory_gate: Option<MemoryGateStatus>,
    /// Why the instant mic isn't in charge (shown in the window).
    mic_fallback: Option<String>,
    /// True while the card runtime is downloading.
    gpu_downloading: bool,
    history: Vec<recordings::Entry>,
    /// The "Your dictation" card numbers.
    usage: stats::Usage,
}

/// Where the active whisper model runs.
#[derive(Clone, Serialize)]
struct GpuStatus {
    /// The dedicated card ("AMD Radeon RX 6500M"), or None.
    name: Option<String>,
    /// Whether the card's whisper.cpp build is installed.
    runtime_installed: bool,
}

/// The unanswered "RAM is tight" question, as the window renders it.
#[derive(Clone, Serialize)]
struct MemoryGateStatus {
    message: String,
    /// True when the blocked path was the card (the third option makes
    /// sense only then).
    gpu_path: bool,
    /// When the plan was the CPU but the card exists: the lighter path
    /// takes one click ("use-card" = switch and load, "download" = fetch
    /// the runtime first). None when the card can't help here.
    card_option: Option<&'static str>,
}

/// Payload of the `dictation` event, which drives the overlay pill.
#[derive(Clone, Serialize)]
#[serde(tag = "phase", rename_all = "lowercase")]
enum Dictation {
    Recording,
    /// The words so far, while the key is still held.
    Partial {
        text: String,
    },
    Transcribing,
    Done {
        text: String,
    },
    Error {
        message: String,
    },
}

/// Cloud providers for Smart formatting, for the main window.
#[derive(Clone, Serialize)]
struct CloudStatus {
    /// The provider in use, if any (None: the local model).
    active: Option<&'static str>,
    providers: Vec<CloudEntry>,
}

#[derive(Clone, Serialize)]
struct CloudEntry {
    id: &'static str,
    name: &'static str,
    model: String,
    default_model: &'static str,
    key_saved: bool,
    key_url: &'static str,
}

fn cloud_status() -> CloudStatus {
    let saved = settings::load();
    CloudStatus {
        active: saved
            .cloud
            .as_deref()
            .and_then(Provider::find)
            .map(Provider::id),
        providers: cloud::PROVIDERS
            .iter()
            .map(|&p| CloudEntry {
                id: p.id(),
                name: p.name(),
                model: saved
                    .cloud_models
                    .get(p.id())
                    .cloned()
                    .unwrap_or_else(|| p.default_model().to_string()),
                default_model: p.default_model(),
                key_saved: secrets::get(p.id()).is_some(),
                key_url: p.key_url(),
            })
            .collect(),
    }
}

/// Points Smart formatting at the chosen cloud provider (or back at the
/// local model).
fn sync_cloud(app: &AppHandle) {
    let saved = settings::load();
    let cloud = saved
        .cloud
        .as_deref()
        .and_then(Provider::find)
        .filter(|_| saved.smart)
        .and_then(
            |p| match Cloud::new(p, saved.cloud_models.get(p.id()).cloned()) {
                Ok(cloud) => Some(cloud),
                Err(e) => {
                    log::warn!("Cloud clean-up is off: {e}");
                    None
                }
            },
        );
    if let Some(cloud) = &cloud {
        log::info!(
            "Smart formatting uses {} ({})",
            cloud.provider.name(),
            cloud.model
        );
    }
    app.state::<AppState>().formatter.set_cloud(cloud);
    emit_status(app);
}

#[derive(Clone, Serialize)]
struct DownloadProgress {
    id: &'static str,
    downloaded: u64,
    total: u64,
}

struct Inner {
    active: &'static model::ModelInfo,
    downloading: Option<&'static str>,
    model_state: ModelState,
    detail: Option<String>,
    history: Vec<recordings::Entry>,
    mic_warning: Option<String>,
    /// Why the instant mic isn't in charge (shown in the window).
    mic_fallback: Option<String>,
    /// The unanswered memory-gate question (message + whether it blocks
    /// the card path), or None.
    memory_gate: Option<MemoryGateStatus>,
    session: Option<dictation::Session>,
    /// True from key release until the text has been pasted.
    busy: bool,
    /// Process name of the app this dictation is going into, captured at
    /// press time for the usage stats (the session keeps only the kind).
    app_target: Option<String>,
    llm_active: &'static LlmInfo,
    llm_state: LlmState,
    llm_detail: Option<String>,
    llm_downloading: Option<&'static str>,
}

struct AppState {
    inner: Mutex<Inner>,
    /// Always-ready microphone (None when instant start is off or failed).
    mic: Mutex<Option<Arc<audio::Mic>>>,
    engine: dictation::Engine,
    formatter: Arc<format::Formatter>,
    /// Bumped every time the pill is shown, so a delayed hide from an
    /// earlier dictation never hides a newer one.
    overlay_generation: AtomicU64,
    /// Usage numbers (stats.json), shown in the main window.
    stats: Mutex<stats::Stats>,
    /// The memory gate's answer channel, Some while a load waits.
    gate: Mutex<Option<std::sync::mpsc::Sender<GateChoice>>>,
    /// True while the card runtime downloads.
    gpu_downloading: AtomicBool,
}

/// What the user answered to the memory-gate question.
enum GateChoice {
    /// Load the model anyway, on the planned device.
    Anyway,
    /// Not now; the model stays unloaded.
    Later,
    /// Keep the card out of it from now on: the device becomes CPU.
    PreferCpu,
}

impl AppState {
    fn new() -> Self {
        // The card shouldn't start empty: dictations recorded before the
        // stats existed are replayed into it exactly once (guarded by a
        // flag saved in stats.json). The recordings index is small, so
        // loading it twice (here and for `history`) is not worth avoiding.
        let mut stats = stats::load();
        stats.backfill_history(recordings::load());
        stats.save();

        Self {
            mic: Mutex::new(None),
            inner: Mutex::new(Inner {
                active: model::selected_model(),
                downloading: None,
                model_state: ModelState::Missing,
                detail: None,
                history: recordings::load().into_iter().take(HISTORY_LIMIT).collect(),
                mic_warning: None,
                mic_fallback: None,
                memory_gate: None,
                session: None,
                busy: false,
                app_target: None,
                llm_active: selected_llm(),
                llm_state: LlmState::Missing,
                llm_detail: None,
                llm_downloading: None,
            }),
            engine: Arc::new(Mutex::new(None)),
            formatter: {
                let formatter = format::Formatter::default();
                formatter.set_snippets(settings::load().snippets);
                Arc::new(formatter)
            },
            overlay_generation: AtomicU64::new(0),
            stats: Mutex::new(stats),
            gate: Mutex::new(None),
            gpu_downloading: AtomicBool::new(false),
        }
    }

    fn status(&self) -> Status {
        let inner = self.inner.lock().unwrap();
        Status {
            model: inner.model_state,
            detail: inner.detail.clone(),
            active: inner.active.id,
            models: model::CATALOG
                .iter()
                .map(|m| ModelEntry {
                    id: m.id,
                    name: m.name,
                    description: m.description,
                    languages: m.languages,
                    size_mb: m.size_mb,
                    installed: m.is_installed(),
                })
                .collect(),
            downloading: inner.downloading,
            overlay: settings::load().overlay,
            smart: settings::load().smart,
            dictionary: settings::load().dictionary,
            snippets: settings::load().snippets,
            llm: LlmStatus {
                state: inner.llm_state,
                detail: inner.llm_detail.clone(),
                active: inner.llm_active.id,
                models: llm::CATALOG
                    .iter()
                    .map(|m| LlmEntry {
                        id: m.id,
                        name: m.name,
                        description: m.description,
                        size_mb: m.size_mb,
                        download_mb: m.download_mb(),
                        installed: m.is_installed() && llm::runtime_installed(),
                    })
                    .collect(),
                downloading: inner.llm_downloading,
            },
            cloud: cloud_status(),
            hotkey: current_hotkey(),
            instant_mic: settings::load().instant_mic,
            auto_enter: settings::load().auto_enter,
            send_key: settings::load().send_key,
            send_rules: settings::load().send_rules,
            theme: settings::load().theme,
            microphone: settings::load().microphone,
            microphones: list_microphone_names(),
            default_microphone: audio::default_device_name(),
            mic_warning: inner.mic_warning.clone(),
            gpu: GpuStatus {
                name: gpu::device_name(),
                runtime_installed: whisper::runtime_vulkan_installed(),
            },
            device: settings::load().device,
            engine_on_gpu: inner.active.uses_whisper() && whisper::gpu_in_use(),
            gpu_note: whisper::take_load_note(),
            memory_gate: inner.memory_gate.clone(),
            mic_fallback: inner.mic_fallback.clone(),
            gpu_downloading: self.gpu_downloading.load(Ordering::SeqCst),
            history: inner.history.clone(),
            usage: self.stats.lock().unwrap().usage(&stats::today_key()),
        }
    }
}

fn emit_status(app: &AppHandle) {
    let _ = app.emit("status-changed", app.state::<AppState>().status());
}

fn set_model_state(app: &AppHandle, model_state: ModelState, detail: Option<String>) {
    {
        let state = app.state::<AppState>();
        let mut inner = state.inner.lock().unwrap();
        inner.model_state = model_state;
        inner.detail = detail;
    }
    emit_status(app);
}

/// Loads `info` in the background and makes it the engine, unless the user
/// picked a different model while it was loading. `interactive` marks a
/// load the user asked for (selection, download finish, device switch) —
/// those may pop the window open when the memory gate has to ask first.
fn load_model(app: AppHandle, info: &'static model::ModelInfo, interactive: bool) {
    *app.state::<AppState>().engine.lock().unwrap() = None;
    set_model_state(&app, ModelState::Loading, None);
    thread::spawn(move || {
        // The gate: ask before loading when RAM is too tight to be kind
        // about it. The card path is a proxy (the engine reports where it
        // really went once it runs). Only Whisper models can use the card.
        let will_use_gpu = info.uses_whisper()
            && settings::load().device != settings::Device::Cpu
            && whisper::runtime_vulkan_installed();
        if memory::is_tight(memory::free_ram_mb(), info.size_mb, will_use_gpu) {
            // When the plan is the CPU but the card exists (with or without
            // its runtime), the gate offers the lighter path instead.
            let card_option =
                if !info.uses_whisper() || will_use_gpu || gpu::device_name().is_none() {
                    None
                } else if settings::load().device == settings::Device::Cpu
                    && whisper::runtime_vulkan_installed()
                {
                    Some("use-card")
                } else {
                    Some("download")
                };
            match ask_memory_gate(&app, info.size_mb, will_use_gpu, card_option, interactive) {
                GateChoice::Later => {
                    let state = app.state::<AppState>();
                    let mut inner = state.inner.lock().unwrap();
                    inner.model_state = ModelState::Missing;
                    inner.detail = Some("Not loaded: memory is tight".into());
                    drop(inner);
                    emit_status(&app);
                    return;
                }
                GateChoice::PreferCpu => {
                    let mut saved = settings::load();
                    saved.device = settings::Device::Cpu;
                    if let Err(e) = settings::save(&saved) {
                        log::warn!("Couldn't save the CPU choice: {e}");
                    }
                }
                GateChoice::Anyway => {}
            }
            // The question is answered; back to a plain loading state.
            let state = app.state::<AppState>();
            {
                let mut inner = state.inner.lock().unwrap();
                inner.memory_gate = None;
                inner.detail = None;
            }
            emit_status(&app);
        }
        let loaded = info.load();
        let state = app.state::<AppState>();
        if state.inner.lock().unwrap().active.id != info.id {
            return;
        }
        match loaded {
            Ok(mut m) => {
                m.set_hotwords(&settings::load().dictionary);
                // Dictation is English: never switch to Cyrillic or Greek
                // (Parakeet V3 wrote "Sruthi" as "Шутом").
                m.set_latin_only(true);
                *state.engine.lock().unwrap() = Some(m);
                set_model_state(&app, ModelState::Ready, None);
            }
            Err(e) => set_model_state(
                &app,
                ModelState::Error,
                Some(format!("Couldn't load {}: {e}", info.name)),
            ),
        }
    });
}

/// Asks the user whether to load `model_mb` when RAM is tight: raises the
/// question on the window, pops the window open when the load was
/// user-initiated, and blocks this loading thread until the answer.
fn ask_memory_gate(
    app: &AppHandle,
    model_mb: u32,
    gpu_path: bool,
    card_option: Option<&'static str>,
    interactive: bool,
) -> GateChoice {
    let free = memory::free_ram_mb().unwrap_or(0);
    let needed = memory::needed_mb(model_mb);
    // What the question is really about: the model living in RAM (CPU
    // path) or the PC being nearly out of memory altogether (card path).
    let message = if gpu_path {
        format!(
            "Very little RAM free ({free} MB) — the card load only stages through RAM, but the PC is close to the edge."
        )
    } else if card_option.is_some() {
        format!(
            "Not much RAM free ({free} MB) — the model needs about {needed} MB on the CPU; on the card it stays out of your RAM."
        )
    } else {
        format!("Not much RAM free ({free} MB) — the model needs about {needed} MB.")
    };
    let (tx, rx) = std::sync::mpsc::channel::<GateChoice>();
    {
        let state = app.state::<AppState>();
        let mut slot = state.gate.lock().unwrap();
        // A second ask while a question is already open proceeds without
        // the gate (the window is busy deciding) and drops the older
        // proposer — its load sees the channel close and steps aside.
        if let Some(old) = slot.replace(tx) {
            drop(old);
        }
        let mut inner = state.inner.lock().unwrap();
        inner.model_state = ModelState::Loading;
        inner.detail = Some("Waiting because memory is tight".into());
        inner.memory_gate = Some(MemoryGateStatus {
            message,
            gpu_path,
            card_option,
        });
    }
    emit_status(app);
    if interactive {
        show_main(app);
    }
    match rx.recv() {
        Ok(choice) => choice,
        // The proposer went away (a newer load replaced it, or the app
        // closed): treat it like "not now".
        Err(_) => GateChoice::Later,
    }
}

/// The window answered the memory-gate question: "anyway", "later" or
/// "cpu" (the last one only appears when the blocked path was the card).
#[tauri::command]
fn memory_decision(app: AppHandle, decision: String) -> Result<(), String> {
    let choice = match decision.as_str() {
        "anyway" => GateChoice::Anyway,
        "later" => GateChoice::Later,
        "cpu" => GateChoice::PreferCpu,
        // Take the card path instead: move the device off the CPU pin and
        // let the waiting load re-read it.
        "card-use" => {
            let mut saved = settings::load();
            saved.device = settings::Device::Auto;
            if let Err(e) = settings::save(&saved) {
                log::warn!("Couldn't save the card choice: {e}");
            }
            GateChoice::Anyway
        }
        // The card needs its runtime first: cancel the CPU load and race
        // the download; the finished download reloads on the card on its own.
        "card-download" => {
            let mut saved = settings::load();
            saved.device = settings::Device::Auto;
            if let Err(e) = settings::save(&saved) {
                log::warn!("Couldn't save the card choice: {e}");
            }
            GateChoice::Later
        }
        other => return Err(format!("Unknown memory decision: {other}")),
    };
    if let Some(tx) = app.state::<AppState>().gate.lock().unwrap().take() {
        tx.send(choice)
            .map_err(|_| "Nobody is waiting for that".to_string())?;
    }
    if decision == "card-download" {
        if let Err(e) = download_gpu_runtime(app) {
            log::warn!("Card runtime download didn't start: {e}");
        }
    }
    Ok(())
}

/// Moves speech between the card and the CPU and reloads the active model
/// on the new home.
#[tauri::command]
fn set_device(app: AppHandle, device: settings::Device) -> Result<(), String> {
    let mut saved = settings::load();
    saved.device = device;
    settings::save(&saved).map_err(|e| e.to_string())?;
    emit_status(&app);
    let active = app.state::<AppState>().inner.lock().unwrap().active;
    load_model(app, active, true);
    Ok(())
}

/// Downloads the card engine once; on success reloads the active whisper
/// model so it can jump onto the card.
#[tauri::command]
fn download_gpu_runtime(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    if whisper::runtime_vulkan_installed() {
        return Err("The graphics-card runtime is already installed".into());
    }
    if state.gpu_downloading.swap(true, Ordering::SeqCst) {
        return Err("The graphics-card runtime is already downloading".into());
    }
    emit_status(&app);
    thread::spawn(move || {
        let mut last_percent = u64::MAX;
        let result = whisper::download_gpu_runtime(&mut |downloaded, total| {
            // Emit at most once per percent to keep the UI responsive.
            let percent = if total > 0 {
                downloaded * 100 / total
            } else {
                downloaded >> 20
            };
            if percent != last_percent {
                last_percent = percent;
                let _ = app.emit(
                    "download-progress",
                    DownloadProgress {
                        id: "gpu-runtime",
                        downloaded,
                        total,
                    },
                );
            }
        });
        let state = app.state::<AppState>();
        state.gpu_downloading.store(false, Ordering::SeqCst);
        match result {
            Ok(()) => {
                emit_status(&app);
                let active = state.inner.lock().unwrap().active;
                if active.uses_whisper() {
                    load_model(app, active, true);
                }
            }
            Err(e) => {
                let _ = app.emit(
                    "download-error",
                    format!("Graphics-card runtime download failed: {e}"),
                );
                emit_status(&app);
            }
        }
    });
    Ok(())
}

fn selected_llm() -> &'static LlmInfo {
    settings::load()
        .llm
        .and_then(|id| llm::find(&id))
        .unwrap_or_else(|| llm::find(llm::DEFAULT_LLM).expect("default LLM in catalog"))
}

fn set_llm_state(app: &AppHandle, llm_state: LlmState, detail: Option<String>) {
    {
        let state = app.state::<AppState>();
        let mut inner = state.inner.lock().unwrap();
        inner.llm_state = llm_state;
        inner.llm_detail = detail;
    }
    emit_status(app);
}

/// Starts or stops llama-server to match the Smart formatting setting and
/// the chosen model.
fn sync_llm(app: AppHandle) {
    let state = app.state::<AppState>();
    let info = state.inner.lock().unwrap().llm_active;
    if !settings::load().smart {
        state.formatter.set_server(None);
        return set_llm_state(&app, LlmState::Off, None);
    }
    if !info.is_installed() || !llm::runtime_installed() {
        state.formatter.set_server(None);
        return set_llm_state(&app, LlmState::Missing, None);
    }
    if state.formatter.server_model() == Some(info.id) {
        return set_llm_state(&app, LlmState::Ready, None);
    }
    state.formatter.set_server(None);
    set_llm_state(&app, LlmState::Starting, None);
    thread::spawn(move || {
        let started = LlmServer::start(info);
        let state = app.state::<AppState>();
        // The user may have switched model or turned Smart off meanwhile.
        let still_wanted =
            settings::load().smart && state.inner.lock().unwrap().llm_active.id == info.id;
        match started {
            Ok(server) if still_wanted => {
                state.formatter.set_server(Some(server));
                set_llm_state(&app, LlmState::Ready, None);
            }
            Ok(_) => {} // Dropping it stops the process.
            Err(e) => set_llm_state(
                &app,
                LlmState::Error,
                Some(format!("Couldn't start {}: {e}", info.name)),
            ),
        }
    });
}

/// Opens or closes the always-ready microphone to match the setting.
fn sync_mic(app: AppHandle) {
    let wanted = settings::load().instant_mic;
    let state = app.state::<AppState>();
    if !wanted {
        if let Some(mic) = state.mic.lock().unwrap().take() {
            mic.stop();
        }
        return;
    }
    if state.mic.lock().unwrap().is_some() {
        return;
    }
    thread::spawn(
        move || match audio::Mic::start(settings::load().microphone) {
            Ok(mic) => {
                let state = app.state::<AppState>();
                if settings::load().instant_mic {
                    *state.mic.lock().unwrap() = Some(mic);
                    // The ready mic is back in charge; the window's
                    // fallback note goes away.
                    if state.inner.lock().unwrap().mic_fallback.take().is_some() {
                        emit_status(&app);
                    }
                } else {
                    mic.stop();
                }
            }
            // Dictation still works, opening the mic on each key press —
            // and the window explains why.
            Err(e) => {
                log::warn!("Couldn't keep the microphone ready ({e}); will open it per dictation");
                let state = app.state::<AppState>();
                let mut inner = state.inner.lock().unwrap();
                let message =
                    format!("Instant start failed ({e}); the microphone opens at each press");
                if inner.mic_fallback.as_deref() != Some(&message) {
                    inner.mic_fallback = Some(message);
                    drop(inner);
                    emit_status(&app);
                }
            }
        },
    );
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn show_overlay(app: &AppHandle) {
    app.state::<AppState>()
        .overlay_generation
        .fetch_add(1, Ordering::SeqCst);
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.show();
    }
}

/// Plays the pill's exit animation after `delay`, then hides the window,
/// unless the pill was shown again in the meantime.
fn schedule_hide(app: &AppHandle, delay: Duration) {
    let generation = app
        .state::<AppState>()
        .overlay_generation
        .load(Ordering::SeqCst);
    let app = app.clone();
    thread::spawn(move || {
        let current = || {
            app.state::<AppState>()
                .overlay_generation
                .load(Ordering::SeqCst)
        };
        thread::sleep(delay);
        if current() != generation {
            return;
        }
        let _ = app.emit_to("overlay", "overlay-exit", ());
        thread::sleep(EXIT_ANIMATION);
        if current() == generation {
            if let Some(w) = app.get_webview_window("overlay") {
                let _ = w.hide();
            }
        }
    });
}

/// Moves the pill under the text field being typed into, or centered just
/// above the taskbar, per the setting. Never follows the mouse.
fn place_overlay(app: &AppHandle) {
    let Some(w) = app.get_webview_window("overlay") else {
        return;
    };
    let Ok(size) = w.outer_size() else { return };
    let (width, height) = (size.width as i32, size.height as i32);

    let field = match settings::load().overlay {
        OverlayPosition::Field => caret::writing_focus(),
        OverlayPosition::Bottom => None,
    };
    // The field's screen, else the screen the user is on, else the primary.
    let monitor = field
        .as_ref()
        .and_then(|c| {
            app.monitor_from_point(c.x as f64, c.y as f64)
                .ok()
                .flatten()
        })
        .or_else(|| {
            let p = app.cursor_position().ok()?;
            app.monitor_from_point(p.x, p.y).ok().flatten()
        })
        .or_else(|| w.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else { return };
    // Work area excludes the taskbar.
    let area = monitor.work_area();
    let (left, top) = (area.position.x, area.position.y);
    let (right, bottom) = (left + area.size.width as i32, top + area.size.height as i32);
    let scale = monitor.scale_factor();

    let (x, y, align) = match field {
        Some(c) => {
            let gap = (CARET_GAP * scale) as i32;
            // Below the insertion point, or above it when there's no room.
            let below = c.y + c.height + gap;
            let y = if below + height <= bottom {
                below
            } else {
                c.y - gap - height
            };
            let x = (c.x - (8.0 * scale) as i32).clamp(left, right - width);
            (x, y.clamp(top, bottom - height), "start")
        }
        None => {
            let margin = (BOTTOM_MARGIN * scale) as i32;
            (
                left + (right - left - width) / 2,
                bottom - height - margin,
                "center",
            )
        }
    };
    let _ = w.set_position(PhysicalPosition::new(x, y));
    let _ = app.emit_to("overlay", "overlay-align", align);
}

/// Result box payload: the text plus the little usage line under it.
#[derive(Clone, Serialize)]
struct ResultText {
    text: String,
    meta: Option<String>,
}

/// Starts showing the result box (text + Copy) for a dictation made while
/// no text field was focused. The box page sizes itself to the text and
/// then calls `place_result`, which positions and shows the window.
fn show_result(app: &AppHandle, text: &str, meta: Option<String>) {
    // The box takes the pill's place at the bottom of the screen.
    schedule_hide(app, Duration::ZERO);
    let _ = app.emit_to(
        "result",
        "result-show",
        ResultText {
            text: text.to_owned(),
            meta,
        },
    );
}

fn hide_result_box(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("result") {
        if w.is_visible().unwrap_or(false) {
            let _ = app.emit_to("result", "result-hide", ());
            let _ = w.hide();
        }
    }
}

/// Sends the live mic level to the pill about 30 times a second while
/// recording.
fn stream_levels(app: &AppHandle, level: Arc<AtomicU32>) {
    let app = app.clone();
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(33));
            if app
                .state::<AppState>()
                .inner
                .lock()
                .unwrap()
                .session
                .is_none()
            {
                break;
            }
            let value = f32::from_bits(level.load(Ordering::Relaxed));
            let _ = app.emit("audio-level", value);
        }
    });
}

fn report_error(app: &AppHandle, message: String) {
    log::error!("{message}");
    let _ = app.emit("dictation", Dictation::Error { message });
    show_overlay(app);
    schedule_hide(app, Duration::from_millis(2500));
}

fn on_hotkey(app: &AppHandle, key_state: ShortcutState) {
    let state = app.state::<AppState>();
    match key_state {
        ShortcutState::Pressed => {
            hide_result_box(app);
            // The clock starts before anything the press does, so key →
            // live recording is honest in the timings log.
            let press = Instant::now();
            let mut inner = state.inner.lock().unwrap();
            // Holding the key auto-repeats Pressed; ignore everything but the first.
            if inner.session.is_some() || inner.busy {
                return;
            }
            if inner.model_state != ModelState::Ready {
                drop(inner);
                show_main(app);
                return;
            }
            let locked = press.elapsed();
            let saved = settings::load();
            let after_settings = press.elapsed();
            // Instant from the always-ready mic; otherwise open it now.
            let mic = state.mic.lock().unwrap().clone();
            let mut mode = dictation::StartMode::Instant;
            let started = match mic {
                // The prewarmed recorder takes the press; open failures only
                // happen at sync-time and surface in the window there.
                Some(mic) => Ok(mic.record()),
                None => {
                    mode = dictation::StartMode::Cold("instant mic not ready".into());
                    audio::Recording::start(saved.microphone)
                }
            };
            let after_mic = press.elapsed();
            if matches!(mode, dictation::StartMode::Cold(_)) && saved.instant_mic {
                // The press went live the slow way; the window says why
                // until the instant mic manages again.
                if inner.mic_fallback.is_none() {
                    inner.mic_fallback =
                        Some("The instant microphone wasn't ready; opened at the press".into());
                }
            }
            let note = format!(
                "lock {} settings {} mic {}",
                locked.as_millis(),
                (after_settings - locked).as_millis(),
                (after_mic - after_settings).as_millis()
            );
            match started {
                Ok(rec) => {
                    let level = rec.level();
                    // The page lookup (UI Automation) runs off the press
                    // path; formatting reads the kind when the text is ready.
                    let kind = Arc::new(Mutex::new(context::AppKind::Other));
                    let on_partial = {
                        let app = app.clone();
                        Arc::new(move |text: String| {
                            let _ = app.emit("dictation", Dictation::Partial { text });
                        })
                    };
                    inner.session = Some(dictation::Session::start(
                        rec,
                        state.engine.clone(),
                        state.formatter.clone(),
                        saved.smart,
                        saved.dictionary,
                        kind.clone(),
                        press,
                        mode,
                        note,
                        on_partial,
                    ));
                    drop(inner);
                    // The pill is up before the caret lookup: the press
                    // shows recording immediately.
                    let _ = app.emit("dictation", Dictation::Recording);
                    show_overlay(app);
                    // Levels start now, not after the caret lookup: the wave
                    // must move from the first moment.
                    stream_levels(app, level.clone());
                    let uia = app.clone();
                    thread::spawn(move || {
                        let target = context::active_app();
                        *kind.lock().unwrap() = target.kind;
                        {
                            let state = uia.state::<AppState>();
                            let mut inner = state.inner.lock().unwrap();
                            inner.app_target = Some(target.process.clone());
                        }
                        log::info!(
                            "Dictating into {} ({:?}): {:?}",
                            target.process,
                            target.kind,
                            target.title
                        );
                    });
                    // Finding the text field can take a moment (UI Automation),
                    // so place the pill after it is already visible.
                    let placed = app.clone();
                    thread::spawn(move || {
                        place_overlay(&placed);
                    });
                }
                Err(e) => {
                    drop(inner);
                    report_error(app, format!("Microphone error: {e}"));
                }
            }
        }
        ShortcutState::Released => {
            let session = {
                let mut inner = state.inner.lock().unwrap();
                let Some(session) = inner.session.take() else {
                    return;
                };
                inner.busy = true;
                session
            };
            let _ = app.emit("dictation", Dictation::Transcribing);
            let app = app.clone();
            thread::spawn(move || {
                let result = transcribe_and_paste(&app, session);
                app.state::<AppState>().inner.lock().unwrap().busy = false;
                match result {
                    Ok(text) => {
                        let linger = if text.is_empty() {
                            Duration::ZERO
                        } else {
                            DONE_LINGER
                        };
                        let _ = app.emit("dictation", Dictation::Done { text });
                        schedule_hide(&app, linger);
                        emit_status(&app);
                    }
                    Err(e) => report_error(&app, e.to_string()),
                }
            });
        }
    }
}

fn transcribe_and_paste(app: &AppHandle, session: dictation::Session) -> anyhow::Result<String> {
    let released = Instant::now();
    timing::reset();
    let app_kind = session.app();
    // Key press → the mic actually streaming. Tiny waits aren't logged;
    // real cold starts are, with the reason.
    let start_delay = session.start_delay();
    if start_delay > Duration::from_millis(50) {
        timing::add("start", start_delay);
        timing::set_start_mode(&session.start_detail());
    }
    let outcome = session.finish()?;
    log::info!(
        "Final text ready {} ms after release",
        released.elapsed().as_millis()
    );

    let state = app.state::<AppState>();
    if outcome.dead_mic {
        let warning = format!(
            "No sound is coming from \"{}\". Choose another microphone in Sruthi.",
            outcome.device
        );
        state.inner.lock().unwrap().mic_warning = Some(warning.clone());
        emit_status(app);
        anyhow::bail!(warning);
    }
    if state.inner.lock().unwrap().mic_warning.take().is_some() {
        emit_status(app);
    }

    // record_usage takes the target; the history entry needs it too.
    let app_name = state.inner.lock().unwrap().app_target.clone().unwrap_or_default();
    if !outcome.text.is_empty() {
        let speech_s = outcome.audio.len() as f32 / audio::TARGET_RATE as f32;
        if caret::text_field_focused() == Some(false) {
            // Nowhere to type: show the text with a Copy button instead.
            let sample = record_usage(app, &outcome.heard, &outcome.text, speech_s, released);
            timing::measure("result-box", || {
                show_result(app, &outcome.text, result_meta(&sample, speech_s))
            });
        } else {
            // The pill can't take focus, so it stays up while the paste lands.
            // Trailing space keeps back-to-back dictations from running together;
            // when Enter follows, the message is done and the space would only
            // sit at the end of what gets sent.
            let saved = settings::load();
            let enter = saved.auto_enter;
            let pasted = if enter {
                outcome.text.clone()
            } else {
                format!("{} ", outcome.text)
            };
            timing::measure("paste", || paste::paste_text(&pasted))?;
            if enter {
                // The window that took the paste is the one still in front.
                let target = context::active_app();
                let key = paste::send_key_for(
                    saved.send_key,
                    &saved.send_rules,
                    &target.process,
                    &target.title,
                );
                log::info!("Sending with {key:?} in {}", target.process);
                timing::measure("send", || paste::press_send(key))?;
            }
            record_usage(app, &outcome.heard, &outcome.text, speech_s, released);
        }
    }
    if !outcome.audio.is_empty() {
        timing::write(
            app_kind,
            outcome.audio.len() as f32 / audio::TARGET_RATE as f32,
            released.elapsed(),
        );
    }

    // Keep the recording (even when nothing was heard, to help tune).
    if !outcome.audio.is_empty() {
        match recordings::add(&outcome.audio, &outcome.heard, &outcome.text, &app_name) {
            Ok(entry) => {
                let mut inner = state.inner.lock().unwrap();
                inner.history.insert(0, entry);
                inner.history.truncate(HISTORY_LIMIT);
            }
            Err(e) => log::warn!("Couldn't save the recording: {e}"),
        }
    }
    Ok(outcome.text)
}

/// Counts this dictation in the usage stats and tells the overlay windows
/// the new pace. `released` is the key-release instant; the wait is
/// computed here, which happens right after the text has landed.
fn record_usage(
    app: &AppHandle,
    heard: &str,
    text: &str,
    speech_s: f32,
    released: Instant,
) -> stats::Sample {
    let state = app.state::<AppState>();
    // Captured at press time (the session keeps only the app kind).
    let app_name = state
        .inner
        .lock()
        .unwrap()
        .app_target
        .take()
        .unwrap_or_else(|| "unknown".to_owned());
    let mut stats = state.stats.lock().unwrap();
    let sample = stats.record(stats::Input {
        heard,
        text,
        speech_s,
        latency_ms: Some(released.elapsed().as_millis() as u64),
        app: Some(&app_name),
        day: &stats::today_key(),
    });
    stats.save();
    let _ = app.emit("dictation-stats", &sample);
    sample
}

/// The "+27 words · 6.4 s · 210 wpm" line under the result box text.
fn result_meta(sample: &stats::Sample, speech_s: f32) -> Option<String> {
    if sample.words == 0 {
        return None;
    }
    let start = format!("+{} words · {speech_s:.1} s", sample.words);
    sample.wpm.map(|wpm| format!("{start} · {wpm:.0} wpm"))
}

/// The main window's hold-to-dictate button drives the same flow as the
/// global hotkey: press starts recording, release finishes and pastes.
#[tauri::command]
fn press_dictation(app: AppHandle) {
    on_hotkey(&app, ShortcutState::Pressed);
}

#[tauri::command]
fn release_dictation(app: AppHandle) {
    on_hotkey(&app, ShortcutState::Released);
}

/// Clears the usage statistics for this PC and refreshes the main window.
#[tauri::command]
fn reset_usage(app: AppHandle) -> Result<(), String> {
    {
        let state = app.state::<AppState>();
        let mut stats = state.stats.lock().unwrap();
        stats.reset();
        stats.save();
    }
    emit_status(&app);
    Ok(())
}

fn lookup(id: &str) -> Result<&'static model::ModelInfo, String> {
    model::find(id).ok_or_else(|| format!("Unknown model: {id}"))
}

#[tauri::command]
fn get_status(state: State<AppState>) -> Status {
    state.status()
}

#[tauri::command]
fn download_model(app: AppHandle, id: String) -> Result<(), String> {
    let info = lookup(&id)?;
    {
        let state = app.state::<AppState>();
        let mut inner = state.inner.lock().unwrap();
        if let Some(other) = inner.downloading {
            return Err(format!("Already downloading {other}"));
        }
        inner.downloading = Some(info.id);
        if inner.active.id == info.id {
            inner.model_state = ModelState::Downloading;
            inner.detail = None;
        }
    }
    emit_status(&app);

    thread::spawn(move || {
        let mut last_percent = u64::MAX;
        let result = info.download(|downloaded, total| {
            // Emit at most once per percent to keep the UI responsive.
            let percent = if total > 0 {
                downloaded * 100 / total
            } else {
                downloaded >> 20
            };
            if percent != last_percent {
                last_percent = percent;
                let _ = app.emit(
                    "download-progress",
                    DownloadProgress {
                        id: info.id,
                        downloaded,
                        total,
                    },
                );
            }
        });

        let is_active = {
            let state = app.state::<AppState>();
            let mut inner = state.inner.lock().unwrap();
            inner.downloading = None;
            inner.active.id == info.id
        };
        match result {
            Ok(()) if is_active => load_model(app.clone(), info, true),
            Ok(()) => emit_status(&app),
            Err(e) if is_active => set_model_state(
                &app,
                ModelState::Error,
                Some(format!("Download failed: {e}")),
            ),
            Err(e) => {
                emit_status(&app);
                let _ = app.emit(
                    "download-error",
                    format!("{} download failed: {e}", info.name),
                );
            }
        }
    });
    Ok(())
}

#[tauri::command]
fn select_model(app: AppHandle, id: String) -> Result<(), String> {
    let info = lookup(&id)?;
    if !info.is_installed() {
        return Err(format!("Download {} first", info.name));
    }
    let mut saved = settings::load();
    saved.model = Some(info.id.to_string());
    settings::save(&saved).map_err(|e| e.to_string())?;
    app.state::<AppState>().inner.lock().unwrap().active = info;
    load_model(app, info, true);
    Ok(())
}

#[tauri::command]
fn delete_model(app: AppHandle, id: String) -> Result<(), String> {
    let info = lookup(&id)?;
    {
        let state = app.state::<AppState>();
        let inner = state.inner.lock().unwrap();
        if inner.active.id == info.id {
            return Err("Switch to another model before deleting this one".into());
        }
        if model::parts_of(inner.active.id).contains(&info.id) {
            return Err(format!(
                "{} is in use by {}; switch models first",
                info.name, inner.active.name
            ));
        }
        if inner.downloading == Some(info.id) {
            return Err("Wait for the download to finish".into());
        }
    }
    info.remove().map_err(|e| e.to_string())?;
    emit_status(&app);
    Ok(())
}

#[tauri::command]
fn set_overlay_position(app: AppHandle, position: OverlayPosition) -> Result<(), String> {
    let mut saved = settings::load();
    saved.overlay = position;
    settings::save(&saved).map_err(|e| e.to_string())?;
    emit_status(&app);
    Ok(())
}

fn lookup_llm(id: &str) -> Result<&'static LlmInfo, String> {
    llm::find(id).ok_or_else(|| format!("Unknown model: {id}"))
}

#[tauri::command]
fn set_smart(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut saved = settings::load();
    saved.smart = enabled;
    settings::save(&saved).map_err(|e| e.to_string())?;
    sync_cloud(&app);
    sync_llm(app);
    Ok(())
}

fn lookup_provider(id: &str) -> Result<Provider, String> {
    Provider::find(id).ok_or_else(|| format!("Unknown provider: {id}"))
}

/// Picks the cloud provider for Smart formatting, or None for the local model.
#[tauri::command]
fn set_cloud_provider(app: AppHandle, provider: Option<String>) -> Result<(), String> {
    let provider = provider.as_deref().map(lookup_provider).transpose()?;
    if let Some(p) = provider {
        if secrets::get(p.id()).is_none() {
            return Err(format!("Add your {} API key first.", p.name()));
        }
    }
    let mut saved = settings::load();
    saved.cloud = provider.map(|p| p.id().to_string());
    settings::save(&saved).map_err(|e| e.to_string())?;
    sync_cloud(&app);
    Ok(())
}

#[tauri::command]
fn save_cloud_key(app: AppHandle, provider: String, key: String) -> Result<(), String> {
    let p = lookup_provider(&provider)?;
    if key.trim().is_empty() {
        return Err("The API key is empty.".into());
    }
    secrets::set(p.id(), &key).map_err(|e| e.to_string())?;
    sync_cloud(&app);
    Ok(())
}

#[tauri::command]
fn delete_cloud_key(app: AppHandle, provider: String) -> Result<(), String> {
    let p = lookup_provider(&provider)?;
    secrets::delete(p.id());
    let mut saved = settings::load();
    if saved.cloud.as_deref() == Some(p.id()) {
        saved.cloud = None;
        settings::save(&saved).map_err(|e| e.to_string())?;
    }
    sync_cloud(&app);
    Ok(())
}

#[tauri::command]
fn set_cloud_model(app: AppHandle, provider: String, model: String) -> Result<(), String> {
    let p = lookup_provider(&provider)?;
    let mut saved = settings::load();
    let model = model.trim();
    if model.is_empty() || model == p.default_model() {
        saved.cloud_models.remove(p.id());
    } else {
        saved
            .cloud_models
            .insert(p.id().to_string(), model.to_string());
    }
    settings::save(&saved).map_err(|e| e.to_string())?;
    sync_cloud(&app);
    Ok(())
}

#[derive(Serialize)]
struct CloudTest {
    text: String,
    ms: u64,
}

/// Cleans up a sample dictation with the provider, to check the key and model.
#[tauri::command]
async fn test_cloud(provider: String) -> Result<CloudTest, String> {
    let p = lookup_provider(&provider)?;
    tauri::async_runtime::spawn_blocking(move || {
        let saved = settings::load();
        let cloud =
            Cloud::new(p, saved.cloud_models.get(p.id()).cloned()).map_err(|e| e.to_string())?;
        let started = Instant::now();
        let sample = "um so I have ten no twenty dogs and they all need food";
        let ruled = format::rules::apply(sample);
        let text = cloud
            .clean_up(&ruled, &saved.dictionary, context::AppKind::Other)
            .map_err(|e| format!("{e:#}"))?;
        Ok(CloudTest {
            text,
            ms: started.elapsed().as_millis() as u64,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn download_llm(app: AppHandle, id: String) -> Result<(), String> {
    let info = lookup_llm(&id)?;
    {
        let state = app.state::<AppState>();
        let mut inner = state.inner.lock().unwrap();
        if let Some(other) = inner.llm_downloading {
            return Err(format!("Already downloading {other}"));
        }
        inner.llm_downloading = Some(info.id);
        if inner.llm_active.id == info.id {
            inner.llm_state = LlmState::Downloading;
            inner.llm_detail = None;
        }
    }
    emit_status(&app);

    thread::spawn(move || {
        let mut last_percent = u64::MAX;
        let result = info.download(|downloaded, total| {
            let percent = if total > 0 {
                downloaded * 100 / total
            } else {
                downloaded >> 20
            };
            if percent != last_percent {
                last_percent = percent;
                let _ = app.emit(
                    "llm-progress",
                    DownloadProgress {
                        id: info.id,
                        downloaded,
                        total,
                    },
                );
            }
        });
        app.state::<AppState>()
            .inner
            .lock()
            .unwrap()
            .llm_downloading = None;
        match result {
            Ok(()) => sync_llm(app.clone()),
            Err(e) => {
                emit_status(&app);
                let _ = app.emit(
                    "download-error",
                    format!("{} download failed: {e}", info.name),
                );
            }
        }
    });
    Ok(())
}

#[tauri::command]
fn select_llm(app: AppHandle, id: String) -> Result<(), String> {
    let info = lookup_llm(&id)?;
    let mut saved = settings::load();
    saved.llm = Some(info.id.to_string());
    settings::save(&saved).map_err(|e| e.to_string())?;
    app.state::<AppState>().inner.lock().unwrap().llm_active = info;
    sync_llm(app);
    Ok(())
}

#[tauri::command]
fn delete_llm(app: AppHandle, id: String) -> Result<(), String> {
    let info = lookup_llm(&id)?;
    {
        let state = app.state::<AppState>();
        let inner = state.inner.lock().unwrap();
        if inner.llm_active.id == info.id {
            return Err("Switch to another model before deleting this one".into());
        }
        if inner.llm_downloading == Some(info.id) {
            return Err("Wait for the download to finish".into());
        }
    }
    info.remove().map_err(|e| e.to_string())?;
    emit_status(&app);
    Ok(())
}

/// Max length of a dictionary entry; longer is almost certainly a mistake.
const MAX_WORD_LEN: usize = 60;

/// Lets the speech model favour dictionary words while decoding.
fn set_hotwords(app: &AppHandle, words: &[String]) {
    if let Some(model) = app.state::<AppState>().engine.lock().unwrap().as_mut() {
        model.set_hotwords(words);
    }
}

#[tauri::command]
fn add_word(app: AppHandle, word: String) -> Result<(), String> {
    let word = word.split_whitespace().collect::<Vec<_>>().join(" ");
    if word.is_empty() || word.len() > MAX_WORD_LEN {
        return Err(format!(
            "Enter a word or short phrase (up to {MAX_WORD_LEN} characters)"
        ));
    }
    let mut saved = settings::load();
    if !saved
        .dictionary
        .iter()
        .any(|w| w.eq_ignore_ascii_case(&word))
    {
        saved.dictionary.push(word);
        saved.dictionary.sort_by_key(|w| w.to_lowercase());
        settings::save(&saved).map_err(|e| e.to_string())?;
        set_hotwords(&app, &saved.dictionary);
    }
    emit_status(&app);
    Ok(())
}

#[tauri::command]
fn remove_word(app: AppHandle, word: String) -> Result<(), String> {
    let mut saved = settings::load();
    saved.dictionary.retain(|w| w != &word);
    settings::save(&saved).map_err(|e| e.to_string())?;
    set_hotwords(&app, &saved.dictionary);
    emit_status(&app);
    Ok(())
}

/// Sets (or clears, when empty) what you actually said in one dictation.
#[tauri::command]
fn set_reference(app: AppHandle, id: String, text: Option<String>) -> Result<(), String> {
    let text = text
        .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|t| !t.is_empty());
    recordings::set_reference(&id, text.clone()).map_err(|e| e.to_string())?;
    for entry in app
        .state::<AppState>()
        .inner
        .lock()
        .unwrap()
        .history
        .iter_mut()
        .filter(|e| e.id == id)
    {
        entry.reference = text.clone();
    }
    emit_status(&app);
    Ok(())
}

/// What one model made of one recording.
#[derive(Serialize)]
struct RerunResult {
    model_id: String,
    name: String,
    heard: String,
    /// `heard` with the dictionary and the instant rules applied, like a
    /// dictation without the AI step.
    text: String,
    ms: u64,
}

/// Runs `work` with a ready engine for `model_id`: the live one when that is
/// the active model, otherwise a fresh load that is dropped afterwards.
fn with_engine<T>(
    app: &AppHandle,
    model_id: &str,
    work: impl FnOnce(&dictation::Engine, &'static model::ModelInfo) -> anyhow::Result<T>,
) -> Result<T, String> {
    let info = model::find(model_id).ok_or("That model isn't in the catalog")?;
    if !info.is_installed() {
        return Err(format!("{} isn't downloaded yet", info.name));
    }
    let state = app.state::<AppState>();
    {
        let inner = state.inner.lock().unwrap();
        if inner.session.is_some() || inner.busy {
            return Err("Wait for the current dictation to finish".into());
        }
    }
    if state.inner.lock().unwrap().active.id == info.id {
        if state.engine.lock().unwrap().is_none() {
            return Err(format!("{} is still loading", info.name));
        }
        return work(&state.engine, info).map_err(|e| e.to_string());
    }
    if memory::is_tight(memory::free_ram_mb(), info.size_mb, false) {
        return Err(format!(
            "Not enough free memory to try {} next to the model in use. Close something or try a smaller model.",
            info.name
        ));
    }
    let mut loaded = info
        .load()
        .map_err(|e| format!("Couldn't load {}: {e}", info.name))?;
    loaded.set_hotwords(&settings::load().dictionary);
    loaded.set_latin_only(true);
    let engine: dictation::Engine = Arc::new(Mutex::new(Some(loaded)));
    work(&engine, info).map_err(|e| e.to_string())
}

/// The transcript the app would paste without its AI step.
fn rules_text(heard: &str) -> String {
    let words = settings::load().dictionary;
    format::rules::tidy_lines(&format::rules::apply(&format::dictionary::apply(
        heard, &words,
    )))
}

/// Transcribes one saved dictation again with `model_id`.
#[tauri::command]
async fn rerun_recording(
    app: AppHandle,
    id: String,
    model_id: String,
) -> Result<RerunResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let audio = recordings::read_samples(&id).map_err(|e| e.to_string())?;
        with_engine(&app, &model_id, |engine, info| {
            let started = Instant::now();
            let heard = dictation::transcribe_audio(engine, &audio)?;
            Ok(RerunResult {
                model_id: info.id.to_string(),
                name: info.name.to_string(),
                text: rules_text(&heard),
                heard,
                ms: started.elapsed().as_millis() as u64,
            })
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Progress of a benchmark run, for the Models page.
#[derive(Clone, Serialize)]
struct BenchmarkProgress {
    model: String,
    step: usize,
    of: usize,
}

static BENCHMARK_RUNNING: AtomicBool = AtomicBool::new(false);
/// The newest corrected dictations a benchmark uses.
const BENCHMARK_CLIPS: usize = 40;

#[tauri::command]
fn get_benchmark() -> Vec<benchmark::Score> {
    benchmark::load()
}

/// Scores each of `models` on your corrected dictations (the newest
/// `BENCHMARK_CLIPS`), one model at a time, and remembers the result.
#[tauri::command]
async fn run_benchmark(
    app: AppHandle,
    models: Vec<String>,
) -> Result<Vec<benchmark::Score>, String> {
    if BENCHMARK_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("A benchmark is already running".into());
    }
    let result = tauri::async_runtime::spawn_blocking(move || {
        let clips: Vec<(String, String)> = recordings::load()
            .into_iter()
            .filter_map(|e| {
                e.reference
                    .filter(|r| !r.trim().is_empty())
                    .map(|r| (e.id, r))
            })
            .take(BENCHMARK_CLIPS)
            .collect();
        if clips.is_empty() {
            return Err(
                "Correct a few dictations on Home first; they are the answer key.".to_string(),
            );
        }
        let audio: Vec<Vec<f32>> = clips
            .iter()
            .map(|(id, _)| recordings::read_samples(id).map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;
        let mut scores = benchmark::load();
        let mut first_error = None;
        let mut any_new = false;
        for (step, model_id) in models.iter().enumerate() {
            let name = model::find(model_id).map_or(model_id.as_str(), |m| m.name);
            let _ = app.emit(
                "benchmark-progress",
                BenchmarkProgress {
                    model: name.to_string(),
                    step,
                    of: models.len(),
                },
            );
            let scored = with_engine(&app, model_id, |engine, info| {
                let (mut words, mut wrong, mut audio_s) = (0, 0, 0.0f32);
                let started = Instant::now();
                for ((_, reference), samples) in clips.iter().zip(&audio) {
                    let text = rules_text(&dictation::transcribe_audio(engine, samples)?);
                    let (edits, n) = benchmark::word_errors(reference, &text);
                    wrong += edits;
                    words += n;
                    audio_s += samples.len() as f32 / audio::TARGET_RATE as f32;
                }
                Ok(benchmark::Score {
                    model_id: info.id.to_string(),
                    name: info.name.to_string(),
                    clips: clips.len(),
                    words,
                    wrong,
                    total_ms: started.elapsed().as_millis() as u64,
                    audio_s,
                    at_ms: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0),
                })
            });
            match scored {
                Ok(score) => {
                    any_new = true;
                    scores.retain(|s| s.model_id != score.model_id);
                    scores.push(score);
                }
                Err(e) => first_error = first_error.or(Some(e)),
            }
        }
        benchmark::save(scores.clone()).map_err(|e| e.to_string())?;
        match first_error {
            // Some models scored and one could not (memory, not installed):
            // the new scores are kept; the reason is shown if none came in.
            Some(e) if !any_new => Err(e),
            _ => Ok(scores),
        }
    })
    .await
    .map_err(|e| e.to_string());
    BENCHMARK_RUNNING.store(false, Ordering::SeqCst);
    result?
}

/// Longest spoken phrase and longest expansion a snippet may have.
const MAX_TRIGGER_LEN: usize = 60;
const MAX_SNIPPET_LEN: usize = 4000;

/// Adds a snippet, or replaces the one called `replacing` (an edit that may
/// also rename the trigger).
#[tauri::command]
fn save_snippet(
    app: AppHandle,
    trigger: String,
    text: String,
    exact: bool,
    replacing: Option<String>,
) -> Result<(), String> {
    let trigger = trigger.split_whitespace().collect::<Vec<_>>().join(" ");
    if trigger.is_empty() || trigger.len() > MAX_TRIGGER_LEN {
        return Err(format!(
            "Say a short phrase to trigger it (up to {MAX_TRIGGER_LEN} characters)"
        ));
    }
    if !trigger.chars().any(char::is_alphanumeric) {
        return Err("The phrase needs at least one letter or number".into());
    }
    if text.trim().is_empty() || text.len() > MAX_SNIPPET_LEN {
        return Err(format!(
            "Enter the text to type (up to {MAX_SNIPPET_LEN} characters)"
        ));
    }
    let mut saved = settings::load();
    let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    // A different snippet already answers to this phrase.
    if saved.snippets.iter().any(|s| {
        same(&s.trigger, &trigger) && !replacing.as_deref().is_some_and(|r| same(r, &s.trigger))
    }) {
        return Err("Another snippet already uses that phrase".into());
    }
    let snippet = format::snippets::Snippet {
        trigger,
        text: text.trim_end().to_string(),
        exact,
    };
    match replacing
        .as_deref()
        .and_then(|r| saved.snippets.iter().position(|s| same(&s.trigger, r)))
    {
        Some(at) => saved.snippets[at] = snippet,
        None => saved.snippets.push(snippet),
    }
    settings::save(&saved).map_err(|e| e.to_string())?;
    app.state::<AppState>()
        .formatter
        .set_snippets(saved.snippets);
    emit_status(&app);
    Ok(())
}

#[tauri::command]
fn remove_snippet(app: AppHandle, trigger: String) -> Result<(), String> {
    let mut saved = settings::load();
    saved.snippets.retain(|s| s.trigger != trigger);
    settings::save(&saved).map_err(|e| e.to_string())?;
    app.state::<AppState>()
        .formatter
        .set_snippets(saved.snippets);
    emit_status(&app);
    Ok(())
}

fn current_hotkey() -> String {
    settings::load()
        .hotkey
        .unwrap_or_else(|| DEFAULT_HOTKEY.to_string())
}

/// Parses "Ctrl+Alt+D" style labels ("Win" means the Windows key).
fn parse_hotkey(label: &str) -> Result<Shortcut, String> {
    let normalized = label
        .split('+')
        .map(|part| match part.trim() {
            "Win" => "Super",
            other => other,
        })
        .collect::<Vec<_>>()
        .join("+");
    normalized
        .parse::<Shortcut>()
        .map_err(|_| format!("\"{label}\" isn't a valid shortcut"))
}

fn list_microphone_names() -> Vec<String> {
    #[cfg(windows)]
    {
        wasapi::device_names()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// Closes and reopens the always-ready mic (after a settings change).
fn restart_mic(app: &AppHandle) {
    if let Some(mic) = app.state::<AppState>().mic.lock().unwrap().take() {
        mic.stop();
    }
    sync_mic(app.clone());
}

#[tauri::command]
fn set_microphone(app: AppHandle, name: Option<String>) -> Result<(), String> {
    let mut saved = settings::load();
    saved.microphone = name;
    settings::save(&saved).map_err(|e| e.to_string())?;
    app.state::<AppState>().inner.lock().unwrap().mic_warning = None;
    restart_mic(&app);
    emit_status(&app);
    Ok(())
}

#[derive(Clone, Serialize)]
struct MicProbe {
    name: String,
    is_default: bool,
    /// Background level heard in one second (raw capture).
    level: f32,
    /// Receives any sound at all.
    working: bool,
    volume: f32,
    muted: bool,
}

/// Listens to each microphone for a second (raw capture) to show which
/// ones actually receive sound. A working mic always picks up some room
/// noise; a dead one delivers exact silence.
#[tauri::command]
async fn probe_microphones() -> Vec<MicProbe> {
    tauri::async_runtime::spawn_blocking(|| {
        let default = audio::default_device_name();
        let info = mic_check::microphones();
        list_microphone_names()
            .into_iter()
            .map(|name| {
                #[cfg(windows)]
                let level = wasapi::probe(&name, 1.0).map(|(rms, _)| rms).unwrap_or(0.0);
                #[cfg(not(windows))]
                let level = 0.0;
                let details = info.iter().find(|m| m.name == name);
                MicProbe {
                    is_default: default.as_deref() == Some(name.as_str()),
                    working: level >= audio::DEAD_MIC_RMS,
                    level,
                    volume: details.map(|m| m.volume).unwrap_or(1.0),
                    muted: details.is_some_and(|m| m.muted),
                    name,
                }
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

#[tauri::command]
fn set_instant_mic(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut saved = settings::load();
    saved.instant_mic = enabled;
    settings::save(&saved).map_err(|e| e.to_string())?;
    sync_mic(app.clone());
    emit_status(&app);
    Ok(())
}

#[tauri::command]
fn set_auto_enter(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut saved = settings::load();
    saved.auto_enter = enabled;
    settings::save(&saved).map_err(|e| e.to_string())?;
    emit_status(&app);
    Ok(())
}

const MAX_SEND_RULES: usize = 30;

#[tauri::command]
fn set_send_key(app: AppHandle, key: settings::SendKey) -> Result<(), String> {
    let mut saved = settings::load();
    saved.send_key = key;
    settings::save(&saved).map_err(|e| e.to_string())?;
    emit_status(&app);
    Ok(())
}

/// Replaces the per-app send rules. Blank and duplicate rules are dropped.
#[tauri::command]
fn set_send_rules(app: AppHandle, rules: Vec<settings::SendRule>) -> Result<(), String> {
    let mut clean: Vec<settings::SendRule> = Vec::new();
    for r in rules {
        let matches = r.matches.split_whitespace().collect::<Vec<_>>().join(" ");
        if matches.is_empty() || matches.len() > 60 {
            continue;
        }
        if clean.iter().any(|c| c.matches.eq_ignore_ascii_case(&matches)) {
            continue;
        }
        clean.push(settings::SendRule { matches, key: r.key });
    }
    if clean.len() > MAX_SEND_RULES {
        return Err(format!("Up to {MAX_SEND_RULES} rules"));
    }
    let mut saved = settings::load();
    saved.send_rules = clean;
    settings::save(&saved).map_err(|e| e.to_string())?;
    emit_status(&app);
    Ok(())
}

#[tauri::command]
fn set_theme(app: AppHandle, theme: settings::Theme) -> Result<(), String> {
    let mut saved = settings::load();
    saved.theme = theme;
    settings::save(&saved).map_err(|e| e.to_string())?;
    // Results box follows along too; the main window applies its own copy.
    let _ = app.emit("theme-changed", theme);
    emit_status(&app);
    Ok(())
}

/// The saved palette choice, read once by the result window at startup.
#[tauri::command]
fn get_theme() -> settings::Theme {
    settings::load().theme
}

/// Routes a bare held key (so far: Alt) through the same press/release flow
/// as the global hotkey.
fn hold_key_handler(app: &AppHandle) -> impl Fn(bool) + Clone + Send + 'static {
    let app = app.clone();
    move |pressed| {
        on_hotkey(
            &app,
            if pressed {
                ShortcutState::Pressed
            } else {
                ShortcutState::Released
            },
        )
    }
}

#[tauri::command]
fn set_hotkey(app: AppHandle, hotkey: String) -> Result<(), String> {
    if holdkey::is_hold_key(&hotkey) {
        // A bare hold never goes through shortcut registration.
        let label = hotkey.trim();
        let shortcuts = app.global_shortcut();
        if let Ok(old) = parse_hotkey(&current_hotkey()) {
            let _ = shortcuts.unregister(old);
        }
        holdkey::spawn(hold_key_handler(&app))
            .map_err(|e| format!("Can't watch the {label} key: {e}"))?;
        let mut saved = settings::load();
        saved.hotkey = Some(label.to_string());
        settings::save(&saved).map_err(|e| e.to_string())?;
        emit_status(&app);
        return Ok(());
    }
    let new = parse_hotkey(&hotkey)?;
    if new.mods.is_empty() && !hotkey_ok_alone(&hotkey) {
        return Err("Add Ctrl, Alt, Shift or Win, or pick a key like F9 or Pause".into());
    }
    let old_label = current_hotkey();
    if old_label == hotkey {
        return Ok(());
    }
    let shortcuts = app.global_shortcut();
    if let Ok(old) = parse_hotkey(&old_label) {
        let _ = shortcuts.unregister(old);
    }
    holdkey::stop();
    if shortcuts.register(new).is_err() {
        // Taken by another app: put the old one back.
        if holdkey::is_hold_key(&old_label) {
            let _ = holdkey::spawn(hold_key_handler(&app));
        } else if let Ok(old) = parse_hotkey(&old_label) {
            let _ = shortcuts.register(old);
        }
        return Err(format!(
            "{hotkey} is already used by another app. Try another one."
        ));
    }
    let mut saved = settings::load();
    saved.hotkey = Some(hotkey);
    settings::save(&saved).map_err(|e| e.to_string())?;
    emit_status(&app);
    Ok(())
}

/// Keys that work as a push-to-talk key without modifiers.
fn hotkey_ok_alone(label: &str) -> bool {
    matches!(label, "Pause" | "ScrollLock" | "Insert")
        || (label.starts_with('F') && label[1..].parse::<u8>().is_ok())
}

/// Shortcuts worth suggesting, with what else commonly uses them.
const HOTKEY_CANDIDATES: &[(&str, &str)] = &[
    (
        "Alt",
        "Left or right Alt, held while speaking; Alt+Tab shares the key",
    ),
    ("Ctrl+Alt+Space", ""),
    ("Alt+Shift+Space", ""),
    ("Ctrl+Alt+D", ""),
    (
        "Ctrl+Shift+Space",
        "Parameter hints in VS Code and other editors",
    ),
    ("Ctrl+Alt+V", "Paste special in some apps"),
    ("Ctrl+Shift+D", "Bookmark all tabs in Chrome and Edge"),
    (
        "Ctrl+Space",
        "Autocomplete in editors; input switching on some PCs",
    ),
    ("Alt+Space", "Window menu in Windows; PowerToys Run"),
    ("F9", "Debuggers, refresh in Outlook"),
    ("Pause", "Rarely used, but missing on many laptops"),
];

#[derive(Clone, Serialize)]
struct HotkeyOption {
    hotkey: &'static str,
    /// "current", "free" or "taken" (registered by another app).
    status: &'static str,
    /// Apps that commonly use it ("" when nothing common does).
    note: &'static str,
    recommended: bool,
}

/// Checks each candidate by briefly trying to register it: Windows refuses
/// shortcuts another app has registered. (Shortcuts apps handle only while
/// focused can't be detected, hence the notes.)
#[tauri::command]
fn hotkey_options(app: AppHandle) -> Vec<HotkeyOption> {
    let current = current_hotkey();
    let shortcuts = app.global_shortcut();
    let mut options: Vec<HotkeyOption> = HOTKEY_CANDIDATES
        .iter()
        .map(|&(hotkey, note)| {
            let status = if hotkey == current {
                "current"
            } else if holdkey::is_hold_key(hotkey) {
                // Candidates get registered as an availability probe; holds
                // are never registered, so there is nothing to check.
                "free"
            } else {
                match parse_hotkey(hotkey) {
                    Ok(shortcut) if shortcuts.register(shortcut).is_ok() => {
                        let _ = shortcuts.unregister(shortcut);
                        "free"
                    }
                    _ => "taken",
                }
            };
            HotkeyOption {
                hotkey,
                status,
                note,
                recommended: false,
            }
        })
        .collect();
    // Recommend the first three that are free and not commonly used.
    for option in options
        .iter_mut()
        .filter(|o| o.status == "free" && o.note.is_empty())
        .take(3)
    {
        option.recommended = true;
    }
    #[cfg(windows)]
    if let Some(alt) = options.iter_mut().find(|o| o.hotkey == "Alt") {
        // The comfortable one: no chord to remember, hold to speak.
        alt.recommended = true;
    }
    // Bare holds are a Windows feature today.
    #[cfg(not(windows))]
    options.retain(|o| o.hotkey != "Alt");
    options
}

#[tauri::command]
fn copy_text(text: String) -> Result<(), String> {
    arboard::Clipboard::new()
        .and_then(|mut c| c.set_text(text))
        .map_err(|e| e.to_string())
}

/// Width of the result box, in logical pixels.
const RESULT_WIDTH: f64 = 460.0;
/// Gap between the result box and the taskbar, in logical pixels.
const RESULT_MARGIN: f64 = 12.0;

/// Sizes the result box to `height` (logical px, measured by the page),
/// pins it centered just above the taskbar, and shows it.
#[tauri::command]
fn place_result(app: AppHandle, height: f64) {
    let Some(w) = app.get_webview_window("result") else {
        return;
    };
    let height = height.clamp(96.0, 320.0);
    let _ = w.set_size(tauri::LogicalSize::new(RESULT_WIDTH, height));
    let monitor = app
        .cursor_position()
        .ok()
        .and_then(|p| app.monitor_from_point(p.x, p.y).ok().flatten())
        .or_else(|| w.primary_monitor().ok().flatten());
    if let Some(monitor) = monitor {
        let area = monitor.work_area();
        let scale = monitor.scale_factor();
        let (w_px, h_px) = ((RESULT_WIDTH * scale) as i32, (height * scale) as i32);
        let x = area.position.x + (area.size.width as i32 - w_px) / 2;
        let y = area.position.y + area.size.height as i32 - h_px - (RESULT_MARGIN * scale) as i32;
        let _ = w.set_position(PhysicalPosition::new(x, y));
    }
    let _ = w.show();
}

#[tauri::command]
fn hide_result(app: AppHandle) {
    hide_result_box(&app);
}

#[tauri::command]
fn get_recording(id: String) -> Result<tauri::ipc::Response, String> {
    recordings::read_wav(&id)
        .map(tauri::ipc::Response::new)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_recording(app: AppHandle, id: String) -> Result<(), String> {
    recordings::remove(&id).map_err(|e| e.to_string())?;
    app.state::<AppState>()
        .inner
        .lock()
        .unwrap()
        .history
        .retain(|e| e.id != id);
    emit_status(&app);
    Ok(())
}

#[tauri::command]
fn open_recordings_folder() -> Result<(), String> {
    let folder = recordings::folder();
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let opener = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(&folder)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| on_hotkey(app, event.state()))
                .build(),
        )
        .manage(AppState::new())
        .setup(|app| {
            let handle = app.handle().clone();

            let hotkey = current_hotkey();
            let registered = if holdkey::is_hold_key(&hotkey) {
                holdkey::spawn(hold_key_handler(&handle)).map_err(|e| e.to_string())
            } else {
                parse_hotkey(&hotkey)
                    .map_err(|e| e.to_string())
                    .and_then(|shortcut| {
                        app.global_shortcut()
                            .register(shortcut)
                            .map_err(|e| e.to_string())
                    })
            };
            if let Err(e) = registered {
                set_model_state(
                    &handle,
                    ModelState::Error,
                    Some(format!(
                        "Couldn't use {hotkey} as the dictation key ({e}). Pick another one below."
                    )),
                );
            }

            let open = MenuItem::with_id(app, "open", "Open Sruthi", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Sruthi")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show_main(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;

            // The pill's window is wider than the pill; let clicks pass
            // through its transparent area to the app underneath.
            if let Some(overlay) = app.get_webview_window("overlay") {
                let _ = overlay.set_ignore_cursor_events(true);
            }

            // Closing the window keeps dictation running in the tray.
            if let Some(main) = app.get_webview_window("main") {
                let w = main.clone();
                main.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = w.hide();
                    }
                });
            }

            let active = app.state::<AppState>().inner.lock().unwrap().active;
            if active.is_installed() {
                load_model(handle.clone(), active, false);
            }
            sync_cloud(&handle);
            sync_llm(handle.clone());
            sync_mic(handle);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            download_model,
            select_model,
            delete_model,
            download_gpu_runtime,
            set_device,
            memory_decision,
            set_overlay_position,
            set_smart,
            download_llm,
            select_llm,
            delete_llm,
            set_cloud_provider,
            save_cloud_key,
            delete_cloud_key,
            set_cloud_model,
            test_cloud,
            add_word,
            remove_word,
            save_snippet,
            remove_snippet,
            set_reference,
            rerun_recording,
            run_benchmark,
            get_benchmark,
            set_hotkey,
            hotkey_options,
            set_instant_mic,
            set_auto_enter,
            set_send_key,
            set_send_rules,
            set_theme,
            get_theme,
            set_microphone,
            probe_microphones,
            get_recording,
            delete_recording,
            copy_text,
            place_result,
            hide_result,
            open_recordings_folder,
            press_dictation,
            release_dictation,
            reset_usage
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                // Stop llama-server together with the app, and close the mic.
                let state = app.state::<AppState>();
                state.formatter.set_server(None);
                if let Some(mic) = state.mic.lock().unwrap().take() {
                    mic.stop();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{HOTKEY_CANDIDATES, hotkey_ok_alone, parse_hotkey};

    #[test]
    fn parses_hotkey_labels_from_the_ui() {
        for label in [
            "Ctrl+Shift+Space",
            "Ctrl+Alt+D",
            "Alt+Shift+Space",
            "Win+Alt+K",
            "Ctrl+Alt+1",
            "F9",
            "Pause",
        ] {
            assert!(parse_hotkey(label).is_ok(), "{label}");
        }
        for (label, _) in HOTKEY_CANDIDATES {
            if label.eq_ignore_ascii_case("Alt") {
                continue; // a bare hold, never registered through shortcuts
            }
            assert!(parse_hotkey(label).is_ok(), "{label}");
        }
        assert!(parse_hotkey("Ctrl+Nope").is_err());
    }

    #[test]
    fn only_some_keys_work_alone() {
        assert!(hotkey_ok_alone("F9") && hotkey_ok_alone("Pause"));
        assert!(!hotkey_ok_alone("D") && !hotkey_ok_alone("Space"));
    }

    // The bare-Alt hold key exists only on Windows (the listener below).
    #[cfg(target_os = "windows")]
    #[test]
    fn alt_is_suggested_as_a_hold_key() {
        assert!(HOTKEY_CANDIDATES.iter().any(|(label, _)| *label == "Alt"));
        assert!(super::holdkey::is_hold_key("alt"));
    }
}
