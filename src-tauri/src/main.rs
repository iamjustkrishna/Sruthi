// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // WebView2 parks unfocused overlay windows at ~1 fps, which freezes the
    // pill's wave. These flags keep the renderer running. Set before any
    // webview exists so the environment picks them up.
    if std::env::var_os("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").is_none() {
        // SAFETY: single-threaded here, before Tauri starts any thread.
        unsafe {
            std::env::set_var(
                "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
                "--disable-background-timer-throttling --disable-backgrounding-occluded-windows --disable-renderer-backgrounding",
            );
        }
    }
    sruthi_lib::run()
}
