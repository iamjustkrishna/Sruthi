//! Child processes (llama-server, whisper-cli) that must never outlive the
//! app.

use std::process::{Child, Command};

/// Hides the console window a child process would otherwise open.
pub fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// Puts `child` in a Windows job object that kills it when this app's
/// process ends, even on a crash or a force-close, so llama-server never
/// lingers holding memory.
#[cfg(windows)]
pub fn kill_with_app(child: &Child) {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
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
        if let Err(e) = unsafe { AssignProcessToJobObject(HANDLE(*job as *mut _), HANDLE(child.as_raw_handle())) } {
            log::warn!("Couldn't tie llama-server to the app's lifetime: {e}");
        }
    }
}

#[cfg(not(windows))]
pub fn kill_with_app(_child: &Child) {}
