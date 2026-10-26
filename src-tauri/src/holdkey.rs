//! The bare Alt key as a hold-to-talk dictation key.
//!
//! Global shortcut registration can only express "modifier + key" combos,
//! so plain Alt — as natural as push-to-talk gets — can't move through that
//! system. This module watches the low-level keyboard stream instead: while
//! either Alt is held, the same press/release flow as the global hotkey
//! drives dictation. Auto-repeats, injected keys and keys other than Alt
//! change nothing, and Alt held together with Ctrl (AltGr typing on
//! international keyboards) never starts a recording. Alt keeps its usual
//! meaning everywhere; the hook only listens, never swallows.
//!
//! Windows only today; everywhere else the normal shortcut stays in charge.

// The low-level watcher below is Windows-only; these stay unused elsewhere.
#[cfg(windows)]
use std::sync::Mutex;
#[cfg(windows)]
use std::sync::mpsc;
#[cfg(windows)]
use std::thread;

/// Whether `label` names a bare held key (so far, Alt) instead of a
/// modifier+key shortcut.
pub fn is_hold_key(label: &str) -> bool {
    cfg!(windows) && label.trim().eq_ignore_ascii_case("Alt")
}

/// Starts watching the hold key, telling `handler` about each press
/// (true) and release (false). Replaces any running watcher.
pub fn spawn<F>(handler: F) -> Result<(), String>
where
    F: Fn(bool) + Clone + Send + 'static,
{
    #[cfg(not(windows))]
    {
        let _ = handler;
        Ok(())
    }
    #[cfg(windows)]
    {
        stop();

        let (tx, rx) = mpsc::channel::<u8>();
        *EVENTS.lock().unwrap() = Some(tx);
        let (done_tx, done_rx) = mpsc::channel::<()>();
        *DONE.lock().unwrap() = Some(done_rx);

        thread::Builder::new()
            .name("sruthi-alt-events".into())
            .spawn({
                let handler = handler.clone();
                move || {
                    for signal in rx {
                        handler(signal == PRESSED);
                    }
                }
            })
            .map_err(|e| e.to_string())?;

        thread::Builder::new()
            .name("sruthi-alt-hook".into())
            .spawn(move || {
                run_hook(&handler);
                let _ = done_tx.send(());
            })
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// Stops the watcher, waiting (briefly) for its hook to unhook itself.
pub fn stop() {
    #[cfg(not(windows))]
    {}
    #[cfg(windows)]
    {
        if let Some(running) = HOOK.lock().unwrap().take() {
            unsafe {
                let _ = PostThreadMessageW(running.thread_id, WM_QUIT, 0, 0);
            }
            // The unhook must happen on the hook's own thread, so after
            // posting the quit message we wait for that thread to finish.
            if let Some(done) = DONE.lock().unwrap().take() {
                let _ = done.recv_timeout(std::time::Duration::from_millis(2000));
            }
        }
        // Dropping the sender ends the event thread after any stragglers.
        *EVENTS.lock().unwrap() = None;
    }
}

/// Where the hook proc pushes transitions for the event thread. Swapped out
/// whenever a new watcher spawns, which also lets the old event thread end.
#[cfg(windows)]
const PRESSED: u8 = 1;
#[cfg(windows)]
const RELEASED: u8 = 2;

/// Which Alt keys are physically down: one bit per key (left, right, or a
/// synthetic generic one that some remappers and virtual machines report).
#[cfg(windows)]
static HELD: Mutex<u32> = Mutex::new(0);

/// Where the hook proc pushes transitions for the event thread. Swapped out
/// whenever a new watcher spawns, which also lets the old event thread end.
#[cfg(windows)]
static EVENTS: Mutex<Option<mpsc::Sender<u8>>> = Mutex::new(None);

/// The running hook's thread id, for the quit message that stops it, plus
/// an acknowledge channel the hook thread signals on its way out.
#[cfg(windows)]
struct RunningHook {
    thread_id: u32,
}
#[cfg(windows)]
static HOOK: Mutex<Option<RunningHook>> = Mutex::new(None);
#[cfg(windows)]
static DONE: Mutex<Option<mpsc::Receiver<()>>> = Mutex::new(None);

/// Which physical Alt the event comes from.
#[cfg(windows)]
const BIT_LEFT: u32 = 1;
#[cfg(windows)]
const BIT_RIGHT: u32 = 2;
#[cfg(windows)]
const BIT_GENERIC: u32 = 4;

/// Applies one physical down or up for `bit`. Returns Some(true) when the
/// recording should begin (first Alt down), Some(false) when it should end
/// (last Alt up), and None when nothing changed: an auto-repeat, or a
/// second Alt while one is already held.
#[cfg(windows)]
fn transition(mask: &mut u32, down: bool, bit: u32) -> Option<bool> {
    if down {
        if *mask & bit == 0 {
            *mask |= bit;
            if *mask == bit {
                return Some(true);
            }
        }
    } else if *mask & bit != 0 {
        *mask &= !bit;
        if *mask == 0 {
            return Some(false);
        }
    }
    None
}

/// Installs the low-level hook and pumps messages on the calling thread
/// until `stop()` posts WM_QUIT. Unhooks itself before returning.
#[cfg(windows)]
fn run_hook(handler: &impl Fn(bool)) {
    let hook =
        unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), std::ptr::null_mut(), 0) };
    if hook.is_null() {
        return;
    }
    *HOOK.lock().unwrap() = Some(RunningHook {
        thread_id: unsafe { GetCurrentThreadId() },
    });

    let mut msg = unsafe { std::mem::zeroed::<MSG>() };
    // Blocks until a message arrives; WM_QUIT makes it return zero.
    while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {}

    unsafe {
        UnhookWindowsHookEx(hook);
    }
    *HOOK.lock().unwrap() = None;

    // Heading out mid-run: end any recording this key started, so the app
    // doesn't stay armed awaiting a release that never arrives.
    let mut mask = HELD.lock().unwrap();
    if *mask != 0 {
        *mask = 0;
        drop(mask);
        handler(false);
    }
}

/// Alt's low-level hook: observes the stream, touches nothing.
#[cfg(windows)]
unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let info = unsafe { *(lparam as *const KBDLLHOOKSTRUCT) };
        let key = info.vkCode as u16;
        let is_alt = key == VK_LMENU || key == VK_RMENU || key == VK_MENU;
        if is_alt && info.flags & LLKHF_INJECTED == 0 {
            let bit = match key {
                VK_LMENU => BIT_LEFT,
                VK_RMENU => BIT_RIGHT,
                _ => BIT_GENERIC,
            };
            let down = info.flags & LLKHF_UP == 0;
            // Ctrl+Alt is AltGr typing and Ctrl+Alt+… combos, not
            // hold-and-speak; only a plain Alt hold starts recording.
            if down && unsafe { GetAsyncKeyState(VK_CONTROL as i32) as u16 & 0x8000 } != 0 {
                // stay silent
            } else {
                let mut mask = HELD.lock().unwrap();
                if let Some(pressed) = transition(&mut mask, down, bit) {
                    drop(mask);
                    if let Some(sender) = EVENTS.lock().unwrap().as_ref() {
                        let _ = sender.send(if pressed { PRESSED } else { RELEASED });
                    }
                }
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

#[cfg(windows)]
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
#[cfg(windows)]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_LMENU, VK_MENU, VK_RMENU,
};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLKHF_UP, MSG,
    PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_QUIT,
};

#[cfg(windows)]
#[cfg(test)]
mod tests {
    use super::{BIT_LEFT, BIT_RIGHT, is_hold_key, transition};

    #[test]
    fn alt_alone_names_the_hold_key() {
        assert!(is_hold_key("Alt"));
        assert!(is_hold_key(" alt "));
        assert!(!is_hold_key("Ctrl"));
        assert!(!is_hold_key("Alt+Space"));
    }

    #[test]
    fn first_alt_down_starts_and_the_last_up_ends() {
        let mut mask = 0;
        assert_eq!(transition(&mut mask, true, BIT_LEFT), Some(true));
        assert_eq!(transition(&mut mask, true, BIT_LEFT), None); // auto-repeat
        assert_eq!(transition(&mut mask, true, BIT_RIGHT), None);
        assert_eq!(transition(&mut mask, false, BIT_LEFT), None);
        assert_eq!(transition(&mut mask, false, BIT_RIGHT), Some(false));
        assert_eq!(mask, 0);
    }

    #[test]
    fn stray_ups_change_nothing() {
        let mut mask = 0;
        assert_eq!(transition(&mut mask, false, BIT_LEFT), None);
        assert_eq!(mask, 0);
    }
}
