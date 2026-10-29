// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Sruthi contributors

//! Where the time goes in each dictation. Each stage reports how long it
//! took (on the dictation's thread), and one line per dictation is added to
//! `timings.log` in the app folder:
//!
//! `2026-10-17 14:20:05  Chat   audio 3.2s  parakeet 240  whisper 610  ai 820  paste 35  total 1710 ms`

use std::cell::RefCell;
use std::fs::OpenOptions;
use std::io::Write;
use std::time::{Duration, Instant};

use crate::context::AppKind;

/// The log is trimmed back to its newest half once it grows past this.
const MAX_LOG_BYTES: u64 = 512 * 1024;

thread_local! {
    static STAGES: RefCell<Vec<(&'static str, u128)>> = const { RefCell::new(Vec::new()) };
    /// How the press went live (`instant` or `cold:<why>`), shown at the
    /// `start` stage's side because that stage is the one it shaped.
    static START_MODE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Starts a new dictation's timings on this thread.
pub fn reset() {
    STAGES.with(|s| s.borrow_mut().clear());
    START_MODE.with(|m| *m.borrow_mut() = None);
}

/// Labels how the press went live, for the `start` stage's log row.
pub fn set_start_mode(label: &str) {
    START_MODE.with(|m| {
        *m.borrow_mut() = if label.is_empty() {
            None
        } else {
            Some(label.to_owned())
        }
    });
}

/// Records a stage's time.
pub fn add(stage: &'static str, took: Duration) {
    STAGES.with(|s| s.borrow_mut().push((stage, took.as_millis())));
}

/// Runs `f` and records how long it took as `stage`.
pub fn measure<T>(stage: &'static str, f: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let out = f();
    add(stage, started.elapsed());
    out
}

/// The stages recorded so far, e.g. "parakeet 240  whisper 610".
pub fn summary() -> String {
    STAGES.with(|s| {
        let mode = START_MODE.with(|m| m.borrow().clone());
        s.borrow()
            .iter()
            .map(|(stage, ms)| match (&**stage, &mode) {
                ("start", Some(label)) => format!("{stage} {ms} {label}"),
                _ => format!("{stage} {ms}"),
            })
            .collect::<Vec<_>>()
            .join("  ")
    })
}

/// Writes this dictation's line to the log (and the dev console).
pub fn write(app: AppKind, audio_secs: f32, total: Duration) {
    let line = format!(
        "{}  {:<8} audio {audio_secs:.1}s  {}  total {} ms",
        now(),
        format!("{app:?}"),
        summary(),
        total.as_millis()
    );
    log::info!("Timing: {line}");
    let path = crate::settings::app_root().join("timings.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            let keep = text
                .lines()
                .skip(text.lines().count() / 2)
                .collect::<Vec<_>>()
                .join("\n");
            let _ = std::fs::write(&path, keep + "\n");
        }
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{line}");
    }
}

#[cfg(windows)]
fn now() -> String {
    // SAFETY: GetLocalTime only fills in the struct it's given.
    let t = unsafe {
        let mut t = std::mem::zeroed();
        windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t);
        t
    };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

#[cfg(not(windows))]
fn now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix {secs}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_start_stage_carries_its_mode() {
        reset();
        add("start", Duration::from_millis(12));
        set_start_mode("instant");
        assert!(summary().contains("start 12 instant"));

        reset();
        add("start", Duration::from_millis(640));
        set_start_mode("cold:instant mic not ready");
        assert!(summary().contains("start 640 cold:instant mic not ready"));

        // Other stages never grow a suffix, and a stale mode is cleared.
        reset();
        add("stop", Duration::from_millis(5));
        assert!(summary().contains("stop 5"));
        assert!(!summary().contains("instant"));
    }
}
