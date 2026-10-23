//! Which app the user is dictating into, so the text can be formatted to
//! suit it: an email gets a greeting, paragraphs and a sign-off; a chat
//! message stays short and casual; code editors and terminals get the
//! words as spoken.

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AppKind {
    Email,
    Chat,
    Document,
    Code,
    Other,
}

#[derive(Clone, Debug, Serialize)]
pub struct ActiveApp {
    /// Executable name, lowercase (e.g. "chrome.exe").
    pub process: String,
    pub title: String,
    pub kind: AppKind,
}

/// The app that currently has keyboard focus.
pub fn active_app() -> ActiveApp {
    let (process, title) = foreground_window();
    let kind = classify(&process, &title);
    ActiveApp {
        process,
        title,
        kind,
    }
}

const BROWSERS: &[&str] = &[
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "brave.exe",
    "opera.exe",
    "arc.exe",
    "vivaldi.exe",
    "zen.exe",
];

fn classify(process: &str, title: &str) -> AppKind {
    let process = process.to_lowercase();
    let title = title.to_lowercase();
    let is = |names: &[&str]| names.iter().any(|n| process == *n);
    let titled = |words: &[&str]| words.iter().any(|w| title.contains(w));

    if is(&[
        "outlook.exe",
        "olk.exe",
        "hxoutlook.exe",
        "thunderbird.exe",
        "mailspring.exe",
    ]) {
        return AppKind::Email;
    }
    if is(&[
        "slack.exe",
        "discord.exe",
        "whatsapp.exe",
        "teams.exe",
        "ms-teams.exe",
        "telegram.exe",
        "signal.exe",
    ]) {
        return AppKind::Chat;
    }
    if is(&[
        "code.exe",
        "cursor.exe",
        "windsurf.exe",
        "zed.exe",
        "devenv.exe",
        "idea64.exe",
        "pycharm64.exe",
        "webstorm64.exe",
        "studio64.exe",
        "windowsterminal.exe",
        "cmd.exe",
        "powershell.exe",
        "pwsh.exe",
        "wezterm-gui.exe",
        "alacritty.exe",
        "antigravity ide.exe",
        "antigravity.exe",
    ]) {
        return AppKind::Code;
    }
    if is(&[
        "winword.exe",
        "notion.exe",
        "obsidian.exe",
        "onenote.exe",
        "notepad.exe",
        "wordpad.exe",
    ]) {
        return AppKind::Document;
    }
    if is(BROWSERS) {
        // Browsers put the site in the window title ("Inbox (3) - … - Gmail").
        if titled(&[
            "gmail",
            "outlook",
            "yahoo mail",
            "proton mail",
            "zoho mail",
            "- mail",
            "compose",
            "inbox",
        ]) {
            return AppKind::Email;
        }
        if titled(&[
            "whatsapp",
            "slack",
            "discord",
            "messenger",
            "telegram",
            "microsoft teams",
            "| teams",
            "chatgpt",
            "claude",
        ]) {
            return AppKind::Chat;
        }
        if titled(&["github", "gitlab", "stack overflow", "codepen", "replit"]) {
            return AppKind::Code;
        }
        if titled(&[
            "google docs",
            "- docs",
            "notion",
            "confluence",
            "microsoft word",
            "dropbox paper",
            "linkedin",
        ]) {
            return AppKind::Document;
        }
    }
    AppKind::Other
}

#[cfg(windows)]
fn foreground_window() -> (String, String) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    };

    // SAFETY: plain Win32 queries into local buffers; the process handle is
    // closed before returning.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return (String::new(), String::new());
        }
        let mut title = [0u16; 512];
        let len = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32).max(0) as usize;
        let title = String::from_utf16_lossy(&title[..len]);

        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let mut process = String::new();
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if !handle.is_null() {
            let mut path = [0u16; 1024];
            let mut size = path.len() as u32;
            if QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, path.as_mut_ptr(), &mut size)
                != 0
            {
                let full = String::from_utf16_lossy(&path[..size as usize]);
                process = full.rsplit(['\\', '/']).next().unwrap_or("").to_lowercase();
            }
            CloseHandle(handle);
        }
        (process, title)
    }
}

#[cfg(not(windows))]
fn foreground_window() -> (String, String) {
    (String::new(), String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_apps() {
        assert_eq!(
            classify("chrome.exe", "Inbox (3) - example@gmail.com - Gmail"),
            AppKind::Email
        );
        assert_eq!(
            classify("msedge.exe", "Mail - Krishna - Outlook"),
            AppKind::Email
        );
        assert_eq!(classify("OUTLOOK.EXE", "Inbox"), AppKind::Email);
        assert_eq!(classify("chrome.exe", "(2) WhatsApp"), AppKind::Chat);
        assert_eq!(classify("slack.exe", "general - Team"), AppKind::Chat);
        assert_eq!(classify("code.exe", "main.rs - Sruthi"), AppKind::Code);
        assert_eq!(classify("WindowsTerminal.exe", "PowerShell"), AppKind::Code);
        assert_eq!(
            classify("chrome.exe", "Project plan - Google Docs"),
            AppKind::Document
        );
        assert_eq!(classify("winword.exe", "Report.docx"), AppKind::Document);
        assert_eq!(classify("chrome.exe", "YouTube"), AppKind::Other);
    }

    /// Prints the app that has focus right now:
    /// `cargo test --lib active_app_now -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn active_app_now() {
        println!("{:?}", active_app());
    }
}
