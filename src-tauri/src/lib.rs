mod audio;
mod caret;
mod model;
mod paste;
mod settings;

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::anyhow;
use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, State, WindowEvent};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use transcribe_rs::{SpeechModel, TranscribeOptions};

use settings::OverlayPosition;

const HOTKEY_LABEL: &str = "Ctrl+Shift+Space";
/// Recordings shorter than this are treated as accidental taps.
const MIN_SAMPLES: usize = audio::TARGET_RATE as usize * 3 / 10;
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
    hotkey: &'static str,
    history: Vec<String>,
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
    history: Vec<String>,
    recording: Option<audio::Recording>,
    /// True from key release until the text has been pasted.
    busy: bool,
}

struct AppState {
    inner: Mutex<Inner>,
    engine: Arc<Mutex<Option<Box<dyn SpeechModel + Send>>>>,
    /// Bumped every time the pill is shown, so a delayed hide from an
    /// earlier dictation never hides a newer one.
    overlay_generation: AtomicU64,
}

impl AppState {
    fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                active: model::selected_model(),
                downloading: None,
                model_state: ModelState::Missing,
                detail: None,
                history: Vec::new(),
                recording: None,
                busy: false,
            }),
            engine: Arc::new(Mutex::new(None)),
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
            hotkey: HOTKEY_LABEL,
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
            Ok(m) => {
                *state.engine.lock().unwrap() = Some(m);
                set_model_state(&app, ModelState::Ready, None);
            }
            Err(e) => set_model_state(&app, ModelState::Error, Some(format!("Couldn't load {}: {e}", info.name))),
        }
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

/// Sends the live mic level to the pill about 30 times a second while
/// recording.
fn stream_levels(app: &AppHandle, level: Arc<AtomicU32>) {
    let app = app.clone();
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(33));
        if app.state::<AppState>().inner.lock().unwrap().recording.is_none() {
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
            let mut inner = state.inner.lock().unwrap();
            // Holding the key auto-repeats Pressed; ignore everything but the first.
            if inner.recording.is_some() || inner.busy {
                return;
            }
            if inner.model_state != ModelState::Ready {
                drop(inner);
                show_main(app);
                return;
            }
            match audio::Recording::start() {
                Ok(rec) => {
                    let level = rec.level();
                    inner.recording = Some(rec);
                    drop(inner);
                    // Finding the text field can take a moment (UI Automation),
                    // so do it off the main thread; the mic is already running.
                    let app = app.clone();
                    thread::spawn(move || {
                        place_overlay(&app);
                        if app.state::<AppState>().inner.lock().unwrap().recording.is_none() {
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
            let rec = {
                let mut inner = state.inner.lock().unwrap();
                let Some(rec) = inner.recording.take() else { return };
                inner.busy = true;
                rec
            };
            let _ = app.emit("dictation", Dictation::Transcribing);
            let app = app.clone();
            thread::spawn(move || {
                let result = transcribe_and_paste(&app, rec);
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

fn transcribe_and_paste(app: &AppHandle, rec: audio::Recording) -> anyhow::Result<String> {
    let samples = rec.stop()?;
    if samples.len() < MIN_SAMPLES {
        return Ok(String::new());
    }

    let state = app.state::<AppState>();
    let started = std::time::Instant::now();
    let text = {
        let mut engine = state.engine.lock().unwrap();
        let model = engine.as_mut().ok_or_else(|| anyhow!("Speech model isn't loaded"))?;
        model
            .transcribe(&samples, &TranscribeOptions::default())
            .map_err(|e| anyhow!("Transcription failed: {e}"))?
            .text
            .trim()
            .to_string()
    };
    log::info!(
        "Transcribed {:.1}s of audio in {} ms",
        samples.len() as f32 / audio::TARGET_RATE as f32,
        started.elapsed().as_millis()
    );
    if text.is_empty() {
        return Ok(text);
    }

    // The pill can't take focus, so it stays up while the paste lands.
    // Trailing space keeps back-to-back dictations from running together.
    paste::paste_text(&format!("{text} "))?;

    let mut inner = state.inner.lock().unwrap();
    inner.history.insert(0, text.clone());
    inner.history.truncate(HISTORY_LIMIT);
    Ok(text)
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

            let shortcut = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space);
            if let Err(e) = app.global_shortcut().register(shortcut) {
                set_model_state(&handle, ModelState::Error, Some(format!("Couldn't register {HOTKEY_LABEL}: {e}")));
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
                load_model(handle, active);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            download_model,
            select_model,
            delete_model,
            set_overlay_position
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
