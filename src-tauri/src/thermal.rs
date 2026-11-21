// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Sruthi contributors

//! A quiet watchdog for hot laptops.
//!
//! Windows does not expose CPU temperature to normal apps, so this reads
//! the two signals it does give us: how busy the CPU is and how far its
//! effective clock has fallen (throttling). When a dictation is running and
//! those stay bad, the model is stopped instead of letting the machine cook
//! or shut down. A long heavy run also trips the guard even if the clock
//! reading is unavailable.
//!
//! The state machine is pure and tested; the Windows sampler only supplies
//! numbers. The guard is deliberately conservative: it never stops a job
//! without an active dictation, and it keeps the next heavy job waiting
//! until the machine has cooled.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::AppHandle;

use crate::settings;

const SAMPLE_EVERY: Duration = Duration::from_secs(2);
/// Busy enough that sustained use deserves attention.
const HOT_LOAD: u8 = 88;
/// Effective clock below this fraction of the nominal clock means throttling.
/// A deep drop is used, not a mild one: laptops on battery legitimately run
/// at a lower clock, and that is not a thermal emergency.
const HOT_CLOCK: u8 = 70;
/// A heavy model running this long is stopped even when the clock looks fine.
const HEAVY_JOB_LIMIT: Duration = Duration::from_secs(90);
/// Minimum time before another heavy job is allowed.
const COOLDOWN: Duration = Duration::from_secs(75);
/// Thermal log is trimmed to its newest half past this size.
const MAX_LOG_BYTES: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Normal,
    Warm,
    Cooling,
}

/// What the main window and the model list show.
#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub state: State,
    /// CPU busy percentage over the last sample window.
    pub load: u8,
    /// Effective clock as a percentage of the nominal clock.
    pub clock_pct: u8,
    /// Seconds left before heavy models are allowed again.
    pub cooling_left_s: u64,
    /// Why the guard is in this state, when there is something to say.
    pub message: Option<String>,
    /// False when the user turned the guard off.
    pub guard: bool,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            state: State::Normal,
            load: 0,
            clock_pct: 100,
            cooling_left_s: 0,
            message: None,
            guard: true,
        }
    }
}

#[derive(Clone)]
struct JobInfo {
    model: String,
    audio_s: f32,
    heavy: bool,
    started: Instant,
}

struct Live {
    state: State,
    load: u8,
    clock_pct: u8,
    message: Option<String>,
    cooling_until: Option<Instant>,
    /// Consecutive busy samples, used by `classify`.
    hot_streak: u32,
    /// Consecutive calm samples during a cooldown.
    calm_streak: u32,
}

static LIVE: Mutex<Live> = Mutex::new(Live {
    state: State::Normal,
    load: 0,
    clock_pct: 100,
    message: None,
    cooling_until: None,
    hot_streak: 0,
    calm_streak: 0,
});
static JOB: Mutex<Option<JobInfo>> = Mutex::new(None);
static STOP: AtomicBool = AtomicBool::new(false);
static ENABLED: AtomicBool = AtomicBool::new(true);
static STARTED: AtomicBool = AtomicBool::new(false);
static APP: OnceLock<AppHandle> = OnceLock::new();

/// Starts the background monitor once. Safe to call from setup.
pub fn start(app: AppHandle) {
    ENABLED.store(settings::load().thermal, Ordering::SeqCst);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = APP.set(app.clone());
    std::thread::spawn(move || monitor(app));
}

/// Current state for the window. Never blocks on the sampler.
pub fn snapshot() -> Snapshot {
    let live = LIVE.lock().unwrap();
    let left = live
        .cooling_until
        .map(|until| until.saturating_duration_since(Instant::now()).as_secs())
        .unwrap_or(0);
    Snapshot {
        state: live.state,
        load: live.load,
        clock_pct: live.clock_pct,
        cooling_left_s: left,
        message: live.message.clone(),
        guard: ENABLED.load(Ordering::SeqCst),
    }
}

pub fn is_cooling() -> bool {
    LIVE.lock().unwrap().state == State::Cooling
}

/// Turns the guard on or off without a restart. Turning it off does not
/// rescue a job already stopped; it only stops future interventions.
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::SeqCst);
    if !enabled {
        let mut live = LIVE.lock().unwrap();
        live.state = State::Normal;
        live.message = None;
        live.cooling_until = None;
        live.hot_streak = 0;
        live.calm_streak = 0;
    }
}

pub fn is_warm() -> bool {
    !matches!(LIVE.lock().unwrap().state, State::Normal)
}

/// True once the guard has asked the current run to stop. Inference code
/// checks this before and after work so a killed child cannot be restarted
/// and a new chunk cannot begin.
pub fn stop_requested() -> bool {
    STOP.load(Ordering::SeqCst)
}

/// Starts a guarded dictation. The returned value ends the job when it is
/// dropped, including on an error path.
pub fn begin_job(model: &str, audio_s: f32, heavy: bool) -> Result<Job, String> {
    if !ENABLED.load(Ordering::SeqCst) {
        // A stop from before the guard was switched off must not poison
        // every later run.
        STOP.store(false, Ordering::SeqCst);
        *JOB.lock().unwrap() = None;
        return Ok(Job { tracked: false });
    }
    let snap = snapshot();
    if snap.state == State::Cooling {
        let wait = snap.cooling_left_s.max(1);
        return Err(format!(
            "Your PC is still cooling down. Try again in about {wait} s."
        ));
    }
    STOP.store(false, Ordering::SeqCst);
    *JOB.lock().unwrap() = Some(JobInfo {
        model: model.to_owned(),
        audio_s,
        heavy,
        started: Instant::now(),
    });
    Ok(Job { tracked: true })
}

/// A running dictation. Dropping it always ends the job, even when the
/// pipeline returns early.
pub struct Job {
    tracked: bool,
}

impl Drop for Job {
    fn drop(&mut self) {
        if self.tracked {
            *JOB.lock().unwrap() = None;
        }
    }
}

/// Threads for a speech job. The guard trades a little speed for a cooler
/// machine; without it the previous physical-core count is kept.
pub fn safe_threads() -> usize {
    let physical = std::thread::available_parallelism()
        .map(|n| (n.get() / 2).max(2))
        .unwrap_or(2);
    if ENABLED.load(Ordering::SeqCst) {
        physical.min(4)
    } else {
        physical.max(4)
    }
}

/// Previews run the model while the key is still held. Heavy models skip
/// them so the heat only starts after release, when the user is waiting.
pub fn previews_allowed(heavy: bool) -> bool {
    !heavy || !ENABLED.load(Ordering::SeqCst)
}

#[derive(Debug, PartialEq)]
enum Verdict {
    Normal,
    Warm,
    Hot(&'static str),
}

/// Pure decision used by the monitor and its tests. `hot_streak` counts
/// consecutive samples at or above `HOT_LOAD`.
fn classify(
    load: u8,
    clock_pct: u8,
    hot_streak: u32,
    job_secs: f32,
    heavy: bool,
    on_ac: bool,
) -> Verdict {
    if heavy && job_secs >= HEAVY_JOB_LIMIT.as_secs_f32() {
        return Verdict::Hot("A heavy model ran for over 90 seconds");
    }
    // Clock-throttling only counts on wall power: a battery-limited clock
    // is a power policy, not a heat warning.
    if on_ac && load >= HOT_LOAD && clock_pct < HOT_CLOCK && hot_streak >= 5 {
        return Verdict::Hot("The CPU is throttling under a long load");
    }
    if load >= 96 && hot_streak >= 20 {
        return Verdict::Hot("The CPU has been pinned for too long");
    }
    if load >= 70 && hot_streak >= 6 {
        return Verdict::Warm;
    }
    Verdict::Normal
}

fn monitor(app: AppHandle) {
    let mut sampler = Sampler::new();
    let mut ticks: u64 = 0;
    loop {
        std::thread::sleep(SAMPLE_EVERY);
        if !ENABLED.load(Ordering::SeqCst) {
            let mut live = LIVE.lock().unwrap();
            if live.state != State::Normal || live.message.is_some() {
                live.state = State::Normal;
                live.message = None;
                live.cooling_until = None;
                drop(live);
                crate::emit_status(&app);
            }
            continue;
        }

        ticks = ticks.saturating_add(1);
        let (load, clock_pct) = sampler.sample();
        let on_ac = ac_power();
        let now = Instant::now();
        let job = JOB.lock().unwrap().clone();
        let job_secs = job
            .as_ref()
            .map(|j| now.saturating_duration_since(j.started).as_secs_f32())
            .unwrap_or(0.0);
        let heavy = job.as_ref().is_some_and(|j| j.heavy);
        // A sample every 30 s during a run, so a stopped or slow job has
        // evidence in thermal.log even when no threshold was crossed.
        if ticks % 15 == 0 {
            if let Some(j) = job.as_ref() {
                note(&format!(
                    "sample load {load}% clock {clock_pct}% ac {} job {} {:.1}s",
                    if on_ac { "yes" } else { "no" },
                    j.model,
                    job_secs
                ));
            }
        }

        let mut live = LIVE.lock().unwrap();
        live.load = load;
        live.clock_pct = clock_pct;
        if load >= HOT_LOAD {
            live.hot_streak = live.hot_streak.saturating_add(1);
        } else {
            live.hot_streak = 0;
        }

        if live.state == State::Cooling {
            let until = live.cooling_until.unwrap_or(now);
            let calm = load < 35 && clock_pct >= 90;
            live.calm_streak = if calm {
                live.calm_streak.saturating_add(1)
            } else {
                0
            };
            let cooled = now >= until || (live.calm_streak >= 8 && job.is_none());
            if cooled {
                live.state = State::Normal;
                live.message = None;
                live.cooling_until = None;
                live.calm_streak = 0;
                drop(live);
                note("cooled down; models allowed again");
                crate::emit_status(&app);
                // llama-server may have been stopped during the incident.
                crate::sync_llm(app.clone());
            }
            continue;
        }

        match classify(load, clock_pct, live.hot_streak, job_secs, heavy, on_ac) {
            Verdict::Hot(reason) => {
                let detail = match &job {
                    Some(j) => format!(
                        "{reason}. Stopped {} on {:.1} s of speech to protect this PC.",
                        j.model, j.audio_s
                    ),
                    None => format!("{reason}. Heavy models are paused for a bit."),
                };
                live.state = State::Cooling;
                live.message = Some(detail.clone());
                live.cooling_until = Some(now + COOLDOWN);
                live.calm_streak = 0;
                drop(live);

                note(&format!(
                    "hot: load {load}% clock {clock_pct}% job {}",
                    job.as_ref()
                        .map(|j| format!("{} {:.1}s", j.model, j.audio_s))
                        .unwrap_or_else(|| "none".into())
                ));
                if job.is_some() {
                    STOP.store(true, Ordering::SeqCst);
                    crate::process::kill_active();
                    crate::pause_llm_for_heat(&app);
                    crate::show_thermal_warning(&app, &detail);
                } else {
                    // No dictation to blame, but a held-key preview may be
                    // running: stop it, drop the idle LLM and pause heavy
                    // models so the machine can recover.
                    STOP.store(true, Ordering::SeqCst);
                    crate::process::kill_active();
                    crate::pause_llm_for_heat(&app);
                    crate::emit_status(&app);
                }
            }
            Verdict::Warm => {
                if live.state != State::Warm {
                    live.state = State::Warm;
                    live.message = Some(
                        "The PC is working hard; Smart clean-up is paused until it settles.".into(),
                    );
                    live.cooling_until = None;
                    drop(live);
                    crate::emit_status(&app);
                }
            }
            Verdict::Normal => {
                if live.state == State::Warm {
                    live.state = State::Normal;
                    live.message = None;
                    drop(live);
                    crate::emit_status(&app);
                }
            }
        }
    }
}

fn note(line: &str) {
    let path = settings::app_root().join("thermal.log");
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
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write;
        let _ = writeln!(file, "{}  {line}", crate::timing::stamp());
    }
}

// ---------------------------------------------------------------------------
// Windows sampler

#[cfg(windows)]
struct Sampler {
    last: Option<(u64, u64)>,
    /// Effective clock from Windows' performance counters. This is the only
    /// reading that follows AMD throttling; `PROCESSOR_POWER_INFORMATION`
    /// reports the nominal clock forever on this class of CPU.
    clock: Option<ClockCounter>,
}

#[cfg(windows)]
impl Sampler {
    fn new() -> Self {
        Self {
            last: None,
            clock: ClockCounter::new(),
        }
    }

    fn sample(&mut self) -> (u8, u8) {
        let load = self.load();
        let clock = self
            .clock
            .as_mut()
            .and_then(ClockCounter::read)
            .unwrap_or_else(clock_pct);
        (load, clock)
    }

    fn load(&mut self) -> u8 {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::GetSystemTimes;

        let mut idle = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: GetSystemTimes fills the three FILETIME values we pass.
        if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
            return 0;
        }
        let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
        let idle = ticks(idle);
        // Kernel time includes idle time on Windows.
        let total = ticks(kernel) + ticks(user);
        let previous = self.last.replace((idle, total));
        let Some((prev_idle, prev_total)) = previous else {
            return 0;
        };
        let total_delta = total.saturating_sub(prev_total);
        let idle_delta = idle.saturating_sub(prev_idle);
        if total_delta == 0 {
            return 0;
        }
        let busy = total_delta.saturating_sub(idle_delta);
        ((busy * 100 / total_delta).min(100)) as u8
    }
}

/// Reads `\Processor Information(_Total)\% Processor Performance` through
/// PDH: the effective clock as a percentage of the nominal clock. The
/// counter is English regardless of the Windows display language.
#[cfg(windows)]
struct ClockCounter {
    query: windows::Win32::System::Performance::PDH_HQUERY,
    counter: windows::Win32::System::Performance::PDH_HCOUNTER,
}

#[cfg(windows)]
impl ClockCounter {
    fn new() -> Option<Self> {
        use windows::Win32::System::Performance::{
            PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
            PdhOpenQueryW,
        };
        use windows::core::PCWSTR;

        // SAFETY: the query and counter handles are checked before use and
        // closed in Drop. The path is a valid NUL-terminated UTF-16 string.
        unsafe {
            let mut query = PDH_HQUERY::default();
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
                return None;
            }
            let path: Vec<u16> = r"\Processor Information(_Total)\% Processor Performance"
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let mut counter = PDH_HCOUNTER::default();
            if PdhAddEnglishCounterW(query, PCWSTR(path.as_ptr()), 0, &mut counter) != 0 {
                let _ = PdhCloseQuery(query);
                return None;
            }
            // The first collection only primes the rate counter.
            let _ = PdhCollectQueryData(query);
            Some(Self { query, counter })
        }
    }

    fn read(&mut self) -> Option<u8> {
        use windows::Win32::System::Performance::{
            PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE,
            PdhCollectQueryData, PdhGetFormattedCounterValue,
        };

        // SAFETY: both handles are live for this struct's lifetime, and the
        // value is only read after a successful formatted read.
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return None;
            }
            let mut value = PDH_FMT_COUNTERVALUE::default();
            if PdhGetFormattedCounterValue(self.counter, PDH_FMT_DOUBLE, None, &mut value) != 0 {
                return None;
            }
            if value.CStatus != PDH_CSTATUS_VALID_DATA && value.CStatus != PDH_CSTATUS_NEW_DATA {
                return None;
            }
            Some(value.Anonymous.doubleValue.clamp(0.0, 200.0) as u8)
        }
    }
}

#[cfg(windows)]
impl Drop for ClockCounter {
    fn drop(&mut self) {
        use windows::Win32::System::Performance::PdhCloseQuery;
        // SAFETY: the handle came from PdhOpenQueryW and is closed once.
        unsafe {
            let _ = PdhCloseQuery(self.query);
        }
    }
}

#[cfg(windows)]
fn clock_pct() -> u8 {
    use windows_sys::Win32::System::Power::{
        CallNtPowerInformation, PROCESSOR_POWER_INFORMATION, ProcessorInformation,
    };

    let count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let mut info: Vec<PROCESSOR_POWER_INFORMATION> = Vec::with_capacity(count);
    // SAFETY: the buffer is zeroed, has `count` elements, and its byte
    // length is passed exactly as CallNtPowerInformation expects.
    let status = unsafe {
        info.set_len(count);
        CallNtPowerInformation(
            ProcessorInformation,
            std::ptr::null(),
            0,
            info.as_mut_ptr().cast(),
            (count * std::mem::size_of::<PROCESSOR_POWER_INFORMATION>()) as u32,
        )
    };
    if status != 0 {
        return 100;
    }
    let max: u64 = info.iter().map(|p| p.MaxMhz as u64).sum();
    let current: u64 = info.iter().map(|p| p.CurrentMhz as u64).sum();
    if max == 0 || current == 0 {
        return 100;
    }
    ((current * 100 / max).min(100)) as u8
}

// ---------------------------------------------------------------------------
// Other platforms: job duration only, no CPU sensor assumptions.

#[cfg(windows)]
fn ac_power() -> bool {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut status = SYSTEM_POWER_STATUS::default();
    // SAFETY: GetSystemPowerStatus fills the struct we pass. Unknown state
    // counts as wall power: that keeps the guard conservative.
    if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
        return true;
    }
    status.ACLineStatus == 1
}

#[cfg(not(windows))]
fn ac_power() -> bool {
    true
}

#[cfg(not(windows))]
struct Sampler;

#[cfg(not(windows))]
impl Sampler {
    fn new() -> Self {
        Self
    }

    fn sample(&mut self) -> (u8, u8) {
        (0, 100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quiet_machine_stays_normal() {
        assert_eq!(classify(20, 100, 0, 2.0, true, true), Verdict::Normal);
        assert_eq!(classify(60, 100, 3, 2.0, false, true), Verdict::Normal);
    }

    #[test]
    fn sustained_load_warms_then_stops_when_throttling() {
        assert_eq!(classify(75, 100, 6, 2.0, false, true), Verdict::Warm);
        assert_eq!(classify(90, 65, 4, 2.0, false, true), Verdict::Normal);
        assert!(matches!(
            classify(90, 65, 5, 2.0, false, true),
            Verdict::Hot(_)
        ));
        // A low clock on battery is power policy, not heat.
        assert_eq!(classify(90, 65, 20, 2.0, false, false), Verdict::Warm);
    }

    #[test]
    fn a_pinned_cpu_is_stopped_even_without_a_clock_reading() {
        assert_eq!(classify(97, 100, 19, 2.0, false, true), Verdict::Warm);
        assert!(matches!(
            classify(97, 100, 20, 2.0, false, true),
            Verdict::Hot(_)
        ));
    }

    #[test]
    fn heavy_jobs_have_a_hard_duration_cap() {
        assert!(matches!(
            classify(20, 100, 0, 90.0, true, true),
            Verdict::Hot(_)
        ));
        assert_eq!(classify(20, 100, 0, 90.0, false, true), Verdict::Normal);
    }

    #[test]
    fn jobs_are_tracked_and_stop_flag_is_per_job() {
        // These globals are shared with the monitor thread; only exercise
        // them when no monitor was started by another test.
        STOP.store(true, Ordering::SeqCst);
        let job = begin_job("parakeet-v3", 2.0, false).unwrap();
        assert!(!stop_requested());
        drop(job);
        assert!(JOB.lock().unwrap().is_none());
    }

    #[test]
    #[ignore = "reads live Windows counters"]
    fn sampler_reads_live_counters() {
        let mut sampler = Sampler::new();
        let _ = sampler.sample();
        std::thread::sleep(Duration::from_secs(2));
        let (load, clock) = sampler.sample();
        println!(
            "load={load}% clock={clock}% ac={} safe_threads={}",
            ac_power(),
            safe_threads()
        );
        assert!(clock > 0);
    }
}
