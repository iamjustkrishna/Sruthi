// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Sruthi contributors

//! Child processes (llama-server, whisper-cli) that must never outlive the
//! app.

use std::process::{Child, Command};
use std::sync::Mutex;

/// Children that may be running inference right now. The thermal guard
/// terminates these when the PC is under stress; a normal job removes its
/// entry when it exits.
static ACTIVE: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Hides the console window a child process would otherwise open and keeps
/// model work below normal priority, so the app the user is typing into
/// stays responsive while a large model runs.
pub fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
        cmd.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// Tracks a child until it exits. Dropping the returned value removes it
/// from the kill list; holding it while a server runs keeps it cancellable.
pub struct ActiveChild {
    pid: u32,
}

impl Drop for ActiveChild {
    fn drop(&mut self) {
        ACTIVE.lock().unwrap().retain(|pid| *pid != self.pid);
    }
}

/// Registers a spawned child for thermal cancellation.
pub fn track(child: &Child) -> ActiveChild {
    let pid = child.id();
    ACTIVE.lock().unwrap().push(pid);
    ActiveChild { pid }
}

/// Terminates every registered inference child. Called only by the thermal
/// guard when it has decided a run must stop.
pub fn kill_active() {
    let pids = ACTIVE.lock().unwrap().clone();
    for pid in pids {
        terminate(pid);
        log::warn!("Thermal guard stopped process {pid}");
    }
}

#[cfg(windows)]
fn terminate(pid: u32) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
    // SAFETY: OpenProcess either returns a valid handle or an error, both
    // handled here. The handle is closed exactly once.
    unsafe {
        if let Ok(handle) = OpenProcess(PROCESS_TERMINATE, false, pid) {
            let _ = TerminateProcess(handle, 1);
            let _ = CloseHandle(handle);
        }
    }
}

#[cfg(unix)]
fn terminate(pid: u32) {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    // SAFETY: plain POSIX signal. ESRCH (already gone) is harmless.
    unsafe {
        kill(pid as i32, 15);
    }
}

#[cfg(not(any(windows, unix)))]
fn terminate(_pid: u32) {}

/// Puts `child` in a Windows job object that kills it when this app's
/// process ends, even on a crash or a force-close, so llama-server never
/// lingers holding memory.
#[cfg(windows)]
pub fn kill_with_app(child: &Child) {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    // One job for the app's lifetime; its handle is closed by Windows when
    // the app exits, which is what triggers the kill.
    static JOB: OnceLock<Option<isize>> = OnceLock::new();
    let job = JOB.get_or_init(|| {
        // SAFETY: plain Win32 calls with a zeroed, correctly sized struct.
        unsafe {
            let job = CreateJobObjectW(None, None).ok()?;
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of_val(&info) as u32,
            )
            .ok()?;
            Some(job.0 as isize)
        }
    });
    if let Some(job) = job {
        // SAFETY: both handles are valid for the duration of the call.
        if let Err(e) = unsafe {
            AssignProcessToJobObject(HANDLE(*job as *mut _), HANDLE(child.as_raw_handle()))
        } {
            log::warn!("Couldn't tie llama-server to the app's lifetime: {e}");
        }
    }
}

#[cfg(not(windows))]
pub fn kill_with_app(_child: &Child) {}
