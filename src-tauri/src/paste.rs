//! Inserts text into whichever app has focus: put it on the clipboard, send
//! the paste shortcut, then restore what the clipboard held before.

use std::thread;
use std::time::Duration;

use anyhow::{Result, anyhow};
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

use crate::settings::{SendKey, SendRule};

pub fn paste_text(text: &str) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new()?;
    let previous = clipboard.get_text().ok();
    clipboard.set_text(text)?;
    // Give the clipboard owner a moment before the target app reads it.
    thread::sleep(Duration::from_millis(40));

    send_paste_shortcut()?;

    // Restore only after the target app has had time to read the clipboard.
    thread::sleep(Duration::from_millis(200));
    if let Some(prev) = previous {
        let _ = clipboard.set_text(prev);
    }
    Ok(())
}

fn send_paste_shortcut() -> Result<()> {
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| anyhow!("{e}"))?;

    #[cfg(target_os = "macos")]
    let (modifier, v) = (Key::Meta, Key::Unicode('v'));
    // Virtual-key code for V, so the shortcut works on any keyboard layout.
    #[cfg(target_os = "windows")]
    let (modifier, v) = (Key::Control, Key::Other(0x56));
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let (modifier, v) = (Key::Control, Key::Unicode('v'));

    enigo
        .key(modifier, Direction::Press)
        .map_err(|e| anyhow!("{e}"))?;
    let clicked = enigo.key(v, Direction::Click).map_err(|e| anyhow!("{e}"));
    // Always release the modifier, even if the click failed.
    enigo
        .key(modifier, Direction::Release)
        .map_err(|e| anyhow!("{e}"))?;
    clicked
}

/// Which key sends, for the app and window title that have focus: the first
/// rule whose text appears in either wins, then the default.
pub fn send_key_for(default: SendKey, rules: &[SendRule], process: &str, title: &str) -> SendKey {
    let haystack = format!("{process} {title}").to_lowercase();
    rules
        .iter()
        .find(|r| {
            !r.matches.trim().is_empty() && haystack.contains(&r.matches.trim().to_lowercase())
        })
        .map_or(default, |r| r.key)
}

/// Presses the send key in the focused app, once the pasted text has landed.
pub fn press_send(key: SendKey) -> Result<()> {
    let modifier = match key {
        SendKey::Off => return Ok(()),
        SendKey::Enter => None,
        SendKey::CtrlEnter => Some(Key::Control),
        SendKey::ShiftEnter => Some(Key::Shift),
        SendKey::AltEnter => Some(Key::Alt),
    };
    // Let the target app finish handling the paste before the key arrives.
    thread::sleep(Duration::from_millis(60));
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| anyhow!("{e}"))?;
    if let Some(m) = modifier {
        enigo.key(m, Direction::Press).map_err(|e| anyhow!("{e}"))?;
    }
    let clicked = enigo
        .key(Key::Return, Direction::Click)
        .map_err(|e| anyhow!("{e}"));
    // Always release the modifier, even if the click failed.
    if let Some(m) = modifier {
        enigo
            .key(m, Direction::Release)
            .map_err(|e| anyhow!("{e}"))?;
    }
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(matches: &str, key: SendKey) -> SendRule {
        SendRule {
            matches: matches.into(),
            key,
        }
    }

    #[test]
    fn the_default_key_applies_when_no_rule_fits() {
        let rules = [rule("github", SendKey::CtrlEnter)];
        assert_eq!(
            send_key_for(SendKey::Enter, &rules, "slack.exe", "general"),
            SendKey::Enter
        );
    }

    #[test]
    fn rules_match_the_app_or_the_window_title_in_any_case() {
        let rules = [
            rule("GitHub", SendKey::CtrlEnter),
            rule("code.exe", SendKey::Off),
        ];
        assert_eq!(
            send_key_for(
                SendKey::Enter,
                &rules,
                "chrome.exe",
                "Issue #4 · github.com"
            ),
            SendKey::CtrlEnter
        );
        assert_eq!(
            send_key_for(SendKey::Enter, &rules, "Code.exe", "main.rs"),
            SendKey::Off
        );
    }

    #[test]
    fn the_first_matching_rule_wins_and_blank_rules_never_match() {
        let rules = [
            rule("  ", SendKey::Off),
            rule("linkedin", SendKey::CtrlEnter),
            rule("chrome", SendKey::ShiftEnter),
        ];
        assert_eq!(
            send_key_for(SendKey::Enter, &rules, "chrome.exe", "Feed | LinkedIn"),
            SendKey::CtrlEnter
        );
    }
}
