mod audio;
mod caret;
mod context;
mod dictation;
mod enhance;
mod format;
mod gpu;
mod mic_check;
mod model;
mod paste;
mod process;
mod recordings;
mod settings;
mod timing;
#[cfg(windows)]
mod wasapi;
mod whisper;

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, RunEvent, State, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use format::llm::{self, LlmInfo, LlmServer};
use settings::OverlayPosition;

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
    dictionary: Vec<String>,
    hotkey: String,
    instant_mic: bool,
    microphone: Option<String>,
    microphones: Vec<String>,
    default_microphone: Option<String>,
    /// Shown in the main window when the microphone delivers no sound.
    mic_warning: Option<String>,
    history: Vec<recordings::Entry>,
}

/// Payload of the `dictation` event, which drives the overlay pill.
#[derive(Clone, Serialize)]
#[serde(tag = "phase", rename_all = "lowercase")]
enum Dictation {
    Recording,
    Transcribing,
    Done { text: String },
    Error { message: String },
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
    session: Option<dictation::Session>,
    /// True from key release until the text has been pasted.
    busy: bool,
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
}

impl AppState {
    fn new() -> Self {
        Self {
            mic: Mutex::new(None),
            inner: Mutex::new(Inner {
                active: model::selected_model(),
                downloading: None,
                model_state: ModelState::Missing,
                detail: None,
                history: recordings::load().into_iter().take(HISTORY_LIMIT).collect(),
                mic_warning: None,
                session: None,
                busy: false,
                llm_active: selected_llm(),
                llm_state: LlmState::Missing,
                llm_detail: None,
                llm_downloading: None,
            }),
            engine: Arc::new(Mutex::new(None)),
            formatter: Arc::new(format::Formatter::default()),
            overlay_generation: AtomicU64::new(0),
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
            hotkey: current_hotkey(),
            instant_mic: settings::load().instant_mic,
            microphone: settings::load().microphone,
            microphones: list_microphone_names(),
            default_microphone: audio::default_device_name(),
            mic_warning: inner.mic_warning.clone(),
            history: inner.history.clone(),
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
/// picked a different model while it was loading.
fn load_model(app: AppHandle, info: &'static model::ModelInfo) {
    *app.state::<AppState>().engine.lock().unwrap() = None;
    set_model_state(&app, ModelState::Loading, None);
    thread::spawn(move || {
        let loaded = info.load();
        let state = app.state::<AppState>();
        if state.inner.lock().unwrap().active.id != info.id {
            return;
        }
        match loaded {
            Ok(mut m) => {
                m.set_hotwords(&settings::load().dictionary);
                // Dictation is English: never switch to Cyrillic or Greek
                // (Parakeet V3 wrote "Srutam" as "Шутом").
                m.set_latin_only(true);
                *state.engine.lock().unwrap() = Some(m);
                set_model_state(&app, ModelState::Ready, None);
            }
            Err(e) => set_model_state(&app, ModelState::Error, Some(format!("Couldn't load {}: {e}", info.name))),
        }
    });
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
        let still_wanted = settings::load().smart && state.inner.lock().unwrap().llm_active.id == info.id;
        match started {
            Ok(server) if still_wanted => {
                state.formatter.set_server(Some(server));
                set_llm_state(&app, LlmState::Ready, None);
            }
            Ok(_) => {} // Dropping it stops the process.
            Err(e) => set_llm_state(&app, LlmState::Error, Some(format!("Couldn't start {}: {e}", info.name))),
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
    thread::spawn(move || match audio::Mic::start(settings::load().microphone) {
        Ok(mic) => {
            let state = app.state::<AppState>();
            if settings::load().instant_mic {
                *state.mic.lock().unwrap() = Some(mic);
            } else {
                mic.stop();
            }
        }
        // Dictation still works, opening the mic on each key press.
        Err(e) => log::warn!("Couldn't keep the microphone ready ({e}); will open it per dictation"),
    });
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn show_overlay(app: &AppHandle) {
    app.state::<AppState>().overlay_generation.fetch_add(1, Ordering::SeqCst);
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.show();
    }
}

/// Plays the pill's exit animation after `delay`, then hides the window,
/// unless the pill was shown again in the meantime.
fn schedule_hide(app: &AppHandle, delay: Duration) {
    let generation = app.state::<AppState>().overlay_generation.load(Ordering::SeqCst);
    let app = app.clone();
    thread::spawn(move || {
        let current = || app.state::<AppState>().overlay_generation.load(Ordering::SeqCst);
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
    let Some(w) = app.get_webview_window("overlay") else { return };
    let Ok(size) = w.outer_size() else { return };
    let (width, height) = (size.width as i32, size.height as i32);

    let field = match settings::load().overlay {
        OverlayPosition::Field => caret::writing_focus(),
        OverlayPosition::Bottom => None,
    };
    // The field's screen, else the screen the user is on, else the primary.
    let monitor = field
        .as_ref()
        .and_then(|c| app.monitor_from_point(c.x as f64, c.y as f64).ok().flatten())
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
            let y = if below + height <= bottom { below } else { c.y - gap - height };
            let x = (c.x - (8.0 * scale) as i32).clamp(left, right - width);
            (x, y.clamp(top, bottom - height), "start")
        }
        None => {
            let margin = (BOTTOM_MARGIN * scale) as i32;
            (left + (right - left - width) / 2, bottom - height - margin, "center")
        }
    };
    let _ = w.set_position(PhysicalPosition::new(x, y));
    let _ = app.emit_to("overlay", "overlay-align", align);
}

/// Starts showing the result box (text + Copy) for a dictation made while
/// no text field was focused. The box page sizes itself to the text and
/// then calls `place_result`, which positions and shows the window.
fn show_result(app: &AppHandle, text: &str) {
    // The box takes the pill's place at the bottom of the screen.
    schedule_hide(app, Duration::ZERO);
    let _ = app.emit_to("result", "result-show", text);
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
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(33));
        if app.state::<AppState>().inner.lock().unwrap().session.is_none() {
            break;
        }
        let value = f32::from_bits(level.load(Ordering::Relaxed));
        let _ = app.emit_to("overlay", "audio-level", value);
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
            // Instant from the always-ready mic; otherwise open it now.
            let mic = state.mic.lock().unwrap().clone();
            let started = match mic {
                Some(mic) => Ok(mic.record()),
                None => audio::Recording::start(settings::load().microphone),
            };
            match started {
                Ok(rec) => {
                    let level = rec.level();
                    let saved = settings::load();
                    let target = context::active_app();
                    log::info!("Dictating into {} ({:?}): {:?}", target.process, target.kind, target.title);
                    inner.session = Some(dictation::Session::start(
                        rec,
                        state.engine.clone(),
                        state.formatter.clone(),
                        saved.smart,
                        saved.dictionary,
                        target.kind,
                    ));
                    drop(inner);
                    // Finding the text field can take a moment (UI Automation),
                    // so do it off the main thread; the mic is already running.
                    let app = app.clone();
                    thread::spawn(move || {
                        place_overlay(&app);
                        if app.state::<AppState>().inner.lock().unwrap().session.is_none() {
                            return; // Key already released; the pill shows "transcribing" instead.
                        }
                        let _ = app.emit("dictation", Dictation::Recording);
                        show_overlay(&app);
                        stream_levels(&app, level);
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
                let Some(session) = inner.session.take() else { return };
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
                        let linger = if text.is_empty() { Duration::ZERO } else { DONE_LINGER };
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
    let outcome = session.finish()?;
    log::info!("Final text ready {} ms after release", released.elapsed().as_millis());

    let state = app.state::<AppState>();
    if outcome.dead_mic {
        let warning = format!(
            "No sound is coming from \"{}\". Choose another microphone in Srutam Dictate.",
            outcome.device
        );
        state.inner.lock().unwrap().mic_warning = Some(warning.clone());
        emit_status(app);
        anyhow::bail!(warning);
    }
    if state.inner.lock().unwrap().mic_warning.take().is_some() {
        emit_status(app);
    }

    if !outcome.text.is_empty() {
        if caret::text_field_focused() == Some(false) {
            // Nowhere to type: show the text with a Copy button instead.
            timing::measure("result-box", || show_result(app, &outcome.text));
        } else {
            // The pill can't take focus, so it stays up while the paste lands.
            // Trailing space keeps back-to-back dictations from running together.
            timing::measure("paste", || paste::paste_text(&format!("{} ", outcome.text)))?;
        }
    }
    if !outcome.audio.is_empty() {
        timing::write(app_kind, outcome.audio.len() as f32 / audio::TARGET_RATE as f32, released.elapsed());
    }

    // Keep the recording (even when nothing was heard, to help tune).
    if !outcome.audio.is_empty() {
        match recordings::add(&outcome.audio, &outcome.heard, &outcome.text) {
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
            let percent = if total > 0 { downloaded * 100 / total } else { downloaded >> 20 };
            if percent != last_percent {
                last_percent = percent;
                let _ = app.emit("download-progress", DownloadProgress { id: info.id, downloaded, total });
            }
        });

        let is_active = {
            let state = app.state::<AppState>();
            let mut inner = state.inner.lock().unwrap();
            inner.downloading = None;
            inner.active.id == info.id
        };
        match result {
            Ok(()) if is_active => load_model(app.clone(), info),
            Ok(()) => emit_status(&app),
            Err(e) if is_active => {
                set_model_state(&app, ModelState::Error, Some(format!("Download failed: {e}")))
            }
            Err(e) => {
                emit_status(&app);
                let _ = app.emit("download-error", format!("{} download failed: {e}", info.name));
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
    load_model(app, info);
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
            return Err(format!("{} is in use by {}; switch models first", info.name, inner.active.name));
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
    sync_llm(app);
    Ok(())
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
            let percent = if total > 0 { downloaded * 100 / total } else { downloaded >> 20 };
            if percent != last_percent {
                last_percent = percent;
                let _ = app.emit("llm-progress", DownloadProgress { id: info.id, downloaded, total });
            }
        });
        app.state::<AppState>().inner.lock().unwrap().llm_downloading = None;
        match result {
            Ok(()) => sync_llm(app.clone()),
            Err(e) => {
                emit_status(&app);
                let _ = app.emit("download-error", format!("{} download failed: {e}", info.name));
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
        return Err(format!("Enter a word or short phrase (up to {MAX_WORD_LEN} characters)"));
    }
    let mut saved = settings::load();
    if !saved.dictionary.iter().any(|w| w.eq_ignore_ascii_case(&word)) {
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

fn current_hotkey() -> String {
    settings::load().hotkey.unwrap_or_else(|| DEFAULT_HOTKEY.to_string())
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
    normalized.parse::<Shortcut>().map_err(|_| format!("\"{label}\" isn't a valid shortcut"))
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
fn set_hotkey(app: AppHandle, hotkey: String) -> Result<(), String> {
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
    if shortcuts.register(new).is_err() {
        // Taken by another app: put the old one back.
        if let Ok(old) = parse_hotkey(&old_label) {
            let _ = shortcuts.register(old);
        }
        return Err(format!("{hotkey} is already used by another app. Try another one."));
    }
    let mut saved = settings::load();
    saved.hotkey = Some(hotkey);
    settings::save(&saved).map_err(|e| e.to_string())?;
    emit_status(&app);
    Ok(())
}

/// Keys that work as a push-to-talk key without modifiers.
fn hotkey_ok_alone(label: &str) -> bool {
    matches!(label, "Pause" | "ScrollLock" | "Insert") || (label.starts_with('F') && label[1..].parse::<u8>().is_ok())
}

/// Shortcuts worth suggesting, with what else commonly uses them.
const HOTKEY_CANDIDATES: &[(&str, &str)] = &[
    ("Ctrl+Alt+Space", ""),
    ("Alt+Shift+Space", ""),
    ("Ctrl+Alt+D", ""),
    ("Ctrl+Shift+Space", "Parameter hints in VS Code and other editors"),
    ("Ctrl+Alt+V", "Paste special in some apps"),
    ("Ctrl+Shift+D", "Bookmark all tabs in Chrome and Edge"),
    ("Ctrl+Space", "Autocomplete in editors; input switching on some PCs"),
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
            } else {
                match parse_hotkey(hotkey) {
                    Ok(shortcut) if shortcuts.register(shortcut).is_ok() => {
                        let _ = shortcuts.unregister(shortcut);
                        "free"
                    }
                    _ => "taken",
                }
            };
            HotkeyOption { hotkey, status, note, recommended: false }
        })
        .collect();
    // Recommend the first three that are free and not commonly used.
    for option in options.iter_mut().filter(|o| o.status == "free" && o.note.is_empty()).take(3) {
        option.recommended = true;
    }
    options
}

#[tauri::command]
fn copy_text(text: String) -> Result<(), String> {
    arboard::Clipboard::new().and_then(|mut c| c.set_text(text)).map_err(|e| e.to_string())
}

/// Width of the result box, in logical pixels.
const RESULT_WIDTH: f64 = 460.0;
/// Gap between the result box and the taskbar, in logical pixels.
const RESULT_MARGIN: f64 = 12.0;

/// Sizes the result box to `height` (logical px, measured by the page),
/// pins it centered just above the taskbar, and shows it.
#[tauri::command]
fn place_result(app: AppHandle, height: f64) {
    let Some(w) = app.get_webview_window("result") else { return };
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
    recordings::read_wav(&id).map(tauri::ipc::Response::new).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_recording(app: AppHandle, id: String) -> Result<(), String> {
    recordings::remove(&id).map_err(|e| e.to_string())?;
    app.state::<AppState>().inner.lock().unwrap().history.retain(|e| e.id != id);
    emit_status(&app);
    Ok(())
}

#[tauri::command]
fn open_recordings_folder() -> Result<(), String> {
    let folder = recordings::folder();
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let opener = if cfg!(windows) { "explorer" } else if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    std::process::Command::new(opener).arg(&folder).spawn().map_err(|e| e.to_string())?;
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
            let registered = parse_hotkey(&hotkey)
                .map_err(|e| e.to_string())
                .and_then(|shortcut| app.global_shortcut().register(shortcut).map_err(|e| e.to_string()));
            if let Err(e) = registered {
                set_model_state(
                    &handle,
                    ModelState::Error,
                    Some(format!("Couldn't use {hotkey} as the dictation key ({e}). Pick another one below.")),
                );
            }

            let open = MenuItem::with_id(app, "open", "Open Srutam Dictate", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Srutam Dictate")
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
                load_model(handle.clone(), active);
            }
            sync_llm(handle.clone());
            sync_mic(handle);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            download_model,
            select_model,
            delete_model,
            set_overlay_position,
            set_smart,
            download_llm,
            select_llm,
            delete_llm,
            add_word,
            remove_word,
            set_hotkey,
            hotkey_options,
            set_instant_mic,
            set_microphone,
            probe_microphones,
            get_recording,
            delete_recording,
            copy_text,
            place_result,
            hide_result,
            open_recordings_folder
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
    use super::{hotkey_ok_alone, parse_hotkey, HOTKEY_CANDIDATES};

    #[test]
    fn parses_hotkey_labels_from_the_ui() {
        for label in ["Ctrl+Shift+Space", "Ctrl+Alt+D", "Alt+Shift+Space", "Win+Alt+K", "Ctrl+Alt+1", "F9", "Pause"] {
            assert!(parse_hotkey(label).is_ok(), "{label}");
        }
        for (label, _) in HOTKEY_CANDIDATES {
            assert!(parse_hotkey(label).is_ok(), "{label}");
        }
        assert!(parse_hotkey("Ctrl+Nope").is_err());
    }

    #[test]
    fn only_some_keys_work_alone() {
        assert!(hotkey_ok_alone("F9") && hotkey_ok_alone("Pause"));
        assert!(!hotkey_ok_alone("D") && !hotkey_ok_alone("Space"));
    }
}
