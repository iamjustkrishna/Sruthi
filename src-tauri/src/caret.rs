//! Finds where the user is writing, so the overlay pill can appear right
//! where the words will land.

/// Insertion point in physical screen pixels.
pub struct Caret {
    pub x: i32,
    pub y: i32,
    pub height: i32,
}

/// The insertion point of the focused text field, trying, in order:
/// 1. the classic Win32 caret (Notepad, most native text boxes; instant),
/// 2. UI Automation's caret/selection (Chrome, Edge, Electron apps like VS
///    Code, Word, modern Windows apps),
/// 3. the focused text box itself, when it's small enough to be a field.
pub fn writing_focus() -> Option<Caret> {
    #[cfg(windows)]
    {
        win32_caret().or_else(uia::focused_text_with_timeout)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Whether the focused element takes typed text: Some(false) on a web page,
/// a file list or the desktop, where pasting would do nothing. None when it
/// can't be told (UI Automation unavailable or too slow); callers should
/// then paste as usual rather than risk losing the text.
pub fn text_field_focused() -> Option<bool> {
    #[cfg(windows)]
    {
        if win32_caret().is_some() {
            return Some(true);
        }
        uia::focused_is_editable_with_timeout()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(windows)]
fn win32_caret() -> Option<Caret> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GUITHREADINFO, GetGUIThreadInfo};

    // SAFETY: GUITHREADINFO is plain data; cbSize is set as the API requires,
    // and both calls only write into the structs we pass.
    unsafe {
        let mut info: GUITHREADINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
        // Thread id 0 means "the foreground thread".
        if GetGUIThreadInfo(0, &mut info) == 0 || info.hwndCaret.is_null() {
            return None;
        }
        let rect = info.rcCaret;
        let height = rect.bottom - rect.top;
        if height <= 0 {
            return None;
        }
        let mut point = POINT {
            x: rect.left,
            y: rect.top,
        };
        if ClientToScreen(info.hwndCaret, &mut point) == 0 {
            return None;
        }
        Some(Caret {
            x: point.x,
            y: point.y,
            height,
        })
    }
}

#[cfg(windows)]
mod uia {
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize, SAFEARRAY,
    };
    use windows::Win32::System::Ole::{
        SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetLBound, SafeArrayGetUBound,
        SafeArrayUnaccessData,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
        IUIAutomationTextPattern2, IUIAutomationTextRange, IUIAutomationValuePattern,
        TextUnit_Character, UIA_ComboBoxControlTypeId, UIA_DocumentControlTypeId,
        UIA_EditControlTypeId, UIA_TextPattern2Id, UIA_TextPatternId, UIA_ValuePatternId,
    };
    use windows::core::BOOL;

    use super::Caret;

    /// A hung target app can block UI Automation calls, so give up quickly
    /// and let the caller fall back.
    const TIMEOUT: Duration = Duration::from_millis(250);

    /// Focused text boxes taller than this are treated as whole documents,
    /// whose outline says nothing about where the cursor is.
    const MAX_FIELD_HEIGHT: i32 = 160;

    pub fn focused_text_with_timeout() -> Option<Caret> {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(focused_text());
        });
        rx.recv_timeout(TIMEOUT).ok().flatten()
    }

    pub fn focused_is_editable_with_timeout() -> Option<bool> {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(focused_is_editable());
        });
        rx.recv_timeout(TIMEOUT).ok().flatten()
    }

    fn focused_is_editable() -> Option<bool> {
        // SAFETY: COM is initialized for this thread before any call and
        // uninitialized after every COM object has been dropped.
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok().ok()?;
            let result = (|| {
                let automation: IUIAutomation =
                    CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
                let element = automation.GetFocusedElement().ok()?;
                let control = element.CurrentControlType().ok()?;
                // A value the user can change: a text box, editable combo box.
                let writable_value = element
                    .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                    .ok()
                    .map(|v| !v.CurrentIsReadOnly().map(|b| b.as_bool()).unwrap_or(true));
                let has_text = element
                    .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
                    .is_ok();
                Some(if control == UIA_EditControlTypeId {
                    true
                } else if control == UIA_DocumentControlTypeId {
                    // Word documents are editable; a browser's web page is a
                    // read-only document (its value is the page address).
                    writable_value.unwrap_or(true)
                } else if control == UIA_ComboBoxControlTypeId {
                    writable_value.unwrap_or(false)
                } else {
                    // Custom text controls such as terminals expose text.
                    writable_value.unwrap_or(false) || has_text
                })
            })();
            CoUninitialize();
            result
        }
    }

    fn focused_text() -> Option<Caret> {
        // SAFETY: COM is initialized for this thread before any call and
        // uninitialized after every COM object has been dropped.
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok().ok()?;
            let result = (|| {
                let automation: IUIAutomation =
                    CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
                let element = automation.GetFocusedElement().ok()?;
                caret_from_text_pattern(&element).or_else(|| small_field_rect(&element))
            })();
            CoUninitialize();
            result
        }
    }

    unsafe fn caret_from_text_pattern(element: &IUIAutomationElement) -> Option<Caret> {
        unsafe {
            if let Ok(pattern) =
                element.GetCurrentPatternAs::<IUIAutomationTextPattern2>(UIA_TextPattern2Id)
            {
                let mut active = BOOL(0);
                if let Ok(range) = pattern.GetCaretRange(&mut active) {
                    if let Some(caret) = range_rect(&range) {
                        return Some(caret);
                    }
                }
            }
            let pattern = element
                .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
                .ok()?;
            let selection = pattern.GetSelection().ok()?;
            if selection.Length().ok()? < 1 {
                return None;
            }
            range_rect(&selection.GetElement(0).ok()?)
        }
    }

    /// Bounding box of a text range. An empty range (a plain caret) often
    /// has no box, so it's widened to one character and tried again.
    unsafe fn range_rect(range: &IUIAutomationTextRange) -> Option<Caret> {
        unsafe {
            if let Some(caret) = first_rect(range.GetBoundingRectangles().ok()?) {
                return Some(caret);
            }
            let widened = range.Clone().ok()?;
            widened.ExpandToEnclosingUnit(TextUnit_Character).ok()?;
            first_rect(widened.GetBoundingRectangles().ok()?)
        }
    }

    /// Reads the first [left, top, width, height] from a SAFEARRAY of
    /// doubles, then frees the array.
    unsafe fn first_rect(array: *mut SAFEARRAY) -> Option<Caret> {
        if array.is_null() {
            return None;
        }
        unsafe {
            let rect = (|| {
                let lower = SafeArrayGetLBound(array, 1).ok()?;
                let upper = SafeArrayGetUBound(array, 1).ok()?;
                if upper - lower + 1 < 4 {
                    return None;
                }
                let mut data = std::ptr::null_mut();
                SafeArrayAccessData(array, &mut data).ok()?;
                let values = std::slice::from_raw_parts(data as *const f64, 4);
                let caret = Caret {
                    x: values[0] as i32,
                    y: values[1] as i32,
                    height: (values[3] as i32).max(1),
                };
                let _ = SafeArrayUnaccessData(array);
                Some(caret)
            })();
            let _ = SafeArrayDestroy(array);
            rect
        }
    }

    unsafe fn small_field_rect(element: &IUIAutomationElement) -> Option<Caret> {
        let rect = unsafe { element.CurrentBoundingRectangle().ok()? };
        let height = rect.bottom - rect.top;
        if height <= 0 || height > MAX_FIELD_HEIGHT || rect.right - rect.left <= 0 {
            return None;
        }
        Some(Caret {
            x: rect.left,
            y: rect.top,
            height,
        })
    }
}

#[cfg(test)]
mod tests {
    /// Prints where the focused text field is. Run it, then click into a
    /// text box: `cargo test --lib caret -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn prints_writing_focus() {
        match super::writing_focus() {
            Some(c) => println!("writing focus at x={} y={} h={}", c.x, c.y, c.height),
            None => println!("no writing focus found"),
        }
        println!("text field focused: {:?}", super::text_field_focused());
    }
}
