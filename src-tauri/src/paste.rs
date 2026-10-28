//! Inserts text into whichever app has focus: put it on the clipboard, send
//! the paste shortcut, then restore what the clipboard held before.

use std::thread;
use std::time::Duration;

use anyhow::{Result, anyhow};
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

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

/// Presses Enter in the focused app, once the pasted text has landed.
pub fn press_enter() -> Result<()> {
    // Let the target app finish handling the paste before the Enter arrives.
    thread::sleep(Duration::from_millis(60));
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| anyhow!("{e}"))?;
    enigo
        .key(Key::Return, Direction::Click)
        .map_err(|e| anyhow!("{e}"))
}
