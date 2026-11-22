// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Sruthi contributors

//! Live resource counters for the main window.
//!
//! Everything here is sampled on this PC and held in memory. Nothing is
//! uploaded and nothing is written to disk.
//!
//! Windows exposes GPU activity through the Performance Data Helper (PDH)
//! "GPU Engine" counters, and dedicated video-memory use through DXGI. Those
//! are the same sources Task Manager uses. CPU comes from `GetSystemTimes`,
//! system RAM from `GlobalMemoryStatusEx`, and Sruthi's own working set from
//! `GetProcessMemoryInfo`.
//!
//! The sampler is deliberately conservative: a missing counter is `None`,
//! never zero. A macOS build reports unavailable instead of borrowing Windows
//! numbers. There is no temperature sensor here, because Windows does not give
//! normal apps one; the heat guard keeps using load and effective clock.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

const SAMPLE_EVERY: Duration = Duration::from_secs(1);
const HISTORY_SAMPLES: usize = 60;
const MIB: u64 = 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    /// False on platforms where this build cannot measure the values.
    pub supported: bool,
    /// Where the numbers came from, shown beside the page.
    pub source: &'static str,
    /// An honest explanation when one or more sources are missing.
    pub note: Option<String>,
    pub sampled_at_ms: u64,
    pub gpu: Option<Gpu>,
    pub vram: Option<Vram>,
    pub cpu: Option<Cpu>,
    pub ram: Option<Ram>,
    /// Oldest first. `ago_s` is seconds before the newest sample.
    pub history: Vec<Sample>,
    pub thermal: crate::thermal::Snapshot,
}

#[derive(Clone, Debug, Serialize)]
pub struct Gpu {
    pub name: Option<String>,
    /// Busy percentage across the selected adapter's engines, capped at 100.
    pub load_pct: Option<f32>,
    /// Sruthi's own share of the same engines, capped at 100.
    pub process_pct: Option<f32>,
    pub engines: Vec<Engine>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Engine {
    pub name: &'static str,
    pub pct: f32,
}

#[derive(Clone, Debug, Serialize)]
pub struct Vram {
    pub used_mb: Option<u64>,
    pub total_mb: Option<u64>,
    pub process_mb: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Cpu {
    pub load_pct: Option<f32>,
    pub threads: u32,
    /// Effective clock as a percentage of nominal, from the heat guard.
    pub clock_pct: u8,
}

#[derive(Clone, Debug, Serialize)]
pub struct Ram {
    pub used_mb: Option<u64>,
    pub total_mb: Option<u64>,
    pub process_mb: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Sample {
    pub ago_s: u64,
    pub gpu_pct: Option<f32>,
    pub vram_pct: Option<f32>,
    pub cpu_pct: Option<f32>,
    pub ram_pct: Option<f32>,
}

#[derive(Default)]
struct History {
    values: VecDeque<Sample>,
}

impl History {
    fn push(&mut self, sample: Sample) {
        self.values.push_back(Sample { ago_s: 0, ..sample });
        while self.values.len() > HISTORY_SAMPLES {
            self.values.pop_front();
        }
    }

    fn view(&self) -> Vec<Sample> {
        let len = self.values.len();
        self.values
            .iter()
            .enumerate()
            .map(|(i, sample)| Sample {
                ago_s: (len.saturating_sub(1 + i)) as u64,
                ..sample.clone()
            })
            .collect()
    }
}

static LATEST: Mutex<Option<Snapshot>> = Mutex::new(None);
static STARTED: AtomicBool = AtomicBool::new(false);

/// Current snapshot for the window. Never blocks on the sampler.
pub fn snapshot() -> Snapshot {
    LATEST.lock().unwrap().clone().unwrap_or_else(initial)
}

/// Starts the one-second sampler once. Safe to call from setup.
pub fn start(app: AppHandle) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    thread::spawn(move || {
        #[cfg(windows)]
        let mut sampler = WindowsSampler::new();
        loop {
            #[cfg(windows)]
            let sample = sampler.sample();
            #[cfg(not(windows))]
            let sample = unavailable();
            *LATEST.lock().unwrap() = Some(sample.clone());
            let _ = app.emit("resource-changed", sample);
            thread::sleep(SAMPLE_EVERY);
        }
    });
}

fn initial() -> Snapshot {
    #[cfg(windows)]
    {
        Snapshot {
            supported: true,
            source: "Windows PDH + DXGI + system counters",
            note: Some("Waiting for the first sample from this PC.".into()),
            sampled_at_ms: now_ms(),
            gpu: None,
            vram: None,
            cpu: None,
            ram: None,
            history: Vec::new(),
            thermal: crate::thermal::snapshot(),
        }
    }
    #[cfg(not(windows))]
    {
        unavailable()
    }
}

#[cfg(not(windows))]
fn unavailable() -> Snapshot {
    Snapshot {
        supported: false,
        source: "Unavailable on this platform",
        note: Some(
            "Live GPU, VRAM, CPU and RAM counters are measured on Windows in this build. \
             No values are shown here rather than showing made-up ones."
                .into(),
        ),
        sampled_at_ms: now_ms(),
        gpu: None,
        vram: None,
        cpu: None,
        ram: None,
        history: Vec::new(),
        thermal: crate::thermal::snapshot(),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn pct(part: u64, whole: u64) -> Option<f32> {
    if whole == 0 {
        None
    } else {
        Some(round1((part as f64 / whole as f64) * 100.0))
    }
}

fn round1(value: f64) -> f32 {
    ((value * 10.0).round() / 10.0) as f32
}

#[cfg(test)]
fn pressure(load: u8, clock_pct: u8) -> u8 {
    load.max(100u8.saturating_sub(clock_pct)).min(100)
}

fn engine_kind(instance: &str) -> &'static str {
    let Some((_, engine)) = instance.rsplit_once("engtype_") else {
        return "Other";
    };
    let engine = engine.to_ascii_lowercase();
    if engine.starts_with("3d") {
        "3D"
    } else if engine.starts_with("compute") {
        "Compute"
    } else if engine.starts_with("copy") {
        "Copy"
    } else if engine.starts_with("video") {
        "Video"
    } else {
        "Other"
    }
}

fn pid_from_instance(instance: &str) -> Option<u32> {
    let rest = instance.strip_prefix("pid_")?;
    rest.split('_').next()?.parse().ok()
}

fn luid_matches(instance: &str, luid: &str) -> bool {
    instance.to_ascii_lowercase().contains(luid)
}

#[cfg(windows)]
struct WindowsSampler {
    cpu: CpuTimes,
    gpu: Option<GpuCounters>,
    vram: Option<DxgiVram>,
    history: History,
    note: Option<String>,
}

#[cfg(windows)]
impl WindowsSampler {
    fn new() -> Self {
        let vram = DxgiVram::new();
        let luid = vram.as_ref().and_then(DxgiVram::luid_prefix);
        let gpu = GpuCounters::new(luid.as_deref());
        let mut notes = Vec::new();
        if gpu.is_none() {
            notes.push("GPU engine counters are unavailable on this PC".to_string());
        }
        if vram.is_none() {
            notes.push("dedicated video memory is unavailable on this PC".to_string());
        }
        let note = if notes.is_empty() {
            None
        } else {
            Some(format!(
                "{}. The affected cards show an em dash instead of a made-up number.",
                notes.join("; ")
            ))
        };
        Self {
            cpu: CpuTimes::default(),
            gpu,
            vram,
            history: History::default(),
            note,
        }
    }

    fn sample(&mut self) -> Snapshot {
        let thermal = crate::thermal::snapshot();
        let cpu_load = self.cpu.sample();
        let gpu_read = self.gpu.as_mut().and_then(GpuCounters::read);
        let vram_read = self.vram.as_mut().and_then(DxgiVram::read);
        let ram = read_ram();
        let process_ram = process_working_set();
        let threads = thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(0);

        let gpu = gpu_read.as_ref().map(|reading| Gpu {
            name: self.vram.as_ref().and_then(|v| v.name.clone()),
            load_pct: reading.load_pct,
            process_pct: reading.process_pct,
            engines: reading.engines.clone(),
        });

        let used_mb = vram_read
            .as_ref()
            .and_then(|v| v.used_mb)
            .or_else(|| gpu_read.as_ref().and_then(|g| g.adapter_vram_mb));
        let total_mb = self
            .vram
            .as_ref()
            .and_then(|v| v.total_mb)
            .or_else(|| vram_read.as_ref().and_then(|v| v.total_mb));
        let process_mb = gpu_read.as_ref().and_then(|g| g.process_vram_mb);
        let vram = used_mb.map_or_else(
            || {
                total_mb.map(|total| Vram {
                    used_mb: None,
                    total_mb: Some(total),
                    process_mb,
                })
            },
            |used| {
                Some(Vram {
                    used_mb: Some(used),
                    total_mb,
                    process_mb,
                })
            },
        );

        let cpu = Some(Cpu {
            load_pct: cpu_load,
            threads,
            clock_pct: thermal.clock_pct,
        });
        let ram = ram.map(|(used, total)| Ram {
            used_mb: Some(used),
            total_mb: Some(total),
            process_mb: process_ram,
        });

        let sample = Sample {
            ago_s: 0,
            gpu_pct: gpu.as_ref().and_then(|g| g.load_pct),
            vram_pct: vram.as_ref().and_then(|v| pct(v.used_mb?, v.total_mb?)),
            cpu_pct: cpu.as_ref().and_then(|c| c.load_pct),
            ram_pct: ram.as_ref().and_then(|r| pct(r.used_mb?, r.total_mb?)),
        };
        self.history.push(sample);

        Snapshot {
            supported: true,
            source: "Windows PDH + DXGI + system counters",
            note: self.note.clone(),
            sampled_at_ms: now_ms(),
            gpu,
            vram,
            cpu,
            ram,
            history: self.history.view(),
            thermal,
        }
    }
}

#[cfg(windows)]
#[derive(Default)]
struct CpuTimes {
    last: Option<(u64, u64)>,
}

#[cfg(windows)]
impl CpuTimes {
    fn sample(&mut self) -> Option<f32> {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::GetSystemTimes;

        let mut idle = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: GetSystemTimes fills the three FILETIME values we pass.
        if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
            return None;
        }
        let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
        let idle = ticks(idle);
        // Kernel time includes idle time on Windows.
        let total = ticks(kernel) + ticks(user);
        let previous = self.last.replace((idle, total));
        let (prev_idle, prev_total) = previous?;
        let total_delta = total.saturating_sub(prev_total);
        let idle_delta = idle.saturating_sub(prev_idle);
        if total_delta == 0 {
            return None;
        }
        let busy = total_delta.saturating_sub(idle_delta);
        Some(round1((busy as f64 / total_delta as f64) * 100.0))
    }
}

#[cfg(windows)]
struct DxgiVram {
    adapter: windows::Win32::Graphics::Dxgi::IDXGIAdapter3,
    name: Option<String>,
    total_mb: Option<u64>,
    luid: Option<String>,
}

#[cfg(windows)]
impl DxgiVram {
    fn new() -> Option<Self> {
        use windows::Win32::Graphics::Dxgi::{
            CreateDXGIFactory1, DXGI_ADAPTER_DESC1, IDXGIAdapter1, IDXGIFactory1,
        };
        use windows::core::Interface;

        // SAFETY: CreateDXGIFactory1 returns a valid factory or an error. The
        // adapter is kept alive by this struct, and its descendants are
        // released by COM when the interface drops.
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
            let mut best: Option<(IDXGIAdapter1, DXGI_ADAPTER_DESC1)> = None;
            let mut best_memory = 0usize;
            for i in 0.. {
                let Ok(adapter) = factory.EnumAdapters1(i) else {
                    break;
                };
                let Ok(desc) = adapter.GetDesc1() else {
                    continue;
                };
                // Skip the Microsoft Basic Render Driver.
                if desc.Flags & 2 != 0 {
                    continue;
                }
                if desc.DedicatedVideoMemory >= best_memory {
                    best_memory = desc.DedicatedVideoMemory;
                    best = Some((adapter, desc));
                }
            }
            let (adapter, desc) = best?;
            let name = String::from_utf16_lossy(
                &desc
                    .Description
                    .iter()
                    .copied()
                    .take_while(|c| *c != 0)
                    .collect::<Vec<_>>(),
            );
            let adapter: windows::Win32::Graphics::Dxgi::IDXGIAdapter3 = adapter.cast().ok()?;
            let luid = Some(format!(
                "luid_0x{:08x}_0x{:08x}",
                desc.AdapterLuid.HighPart, desc.AdapterLuid.LowPart
            ));
            let total_mb =
                (desc.DedicatedVideoMemory > 0).then_some(desc.DedicatedVideoMemory as u64 / MIB);
            Some(Self {
                adapter,
                name: (!name.is_empty()).then_some(name),
                total_mb,
                luid,
            })
        }
    }

    fn luid_prefix(&self) -> Option<String> {
        self.luid.clone()
    }

    fn read(&mut self) -> Option<VramReading> {
        use windows::Win32::Graphics::Dxgi::{
            DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO,
        };

        let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        // SAFETY: the adapter is live for this struct's lifetime.
        unsafe {
            self.adapter
                .QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info)
                .ok()?;
        }
        Some(VramReading {
            used_mb: Some(info.CurrentUsage / MIB),
            total_mb: self.total_mb,
        })
    }
}

#[cfg(windows)]
struct VramReading {
    used_mb: Option<u64>,
    total_mb: Option<u64>,
}

#[cfg(windows)]
struct GpuReading {
    load_pct: Option<f32>,
    process_pct: Option<f32>,
    engines: Vec<Engine>,
    process_vram_mb: Option<u64>,
    adapter_vram_mb: Option<u64>,
}

#[cfg(windows)]
struct GpuCounters {
    query: windows::Win32::System::Performance::PDH_HQUERY,
    engine: Option<windows::Win32::System::Performance::PDH_HCOUNTER>,
    process_vram: Option<windows::Win32::System::Performance::PDH_HCOUNTER>,
    adapter_vram: Option<windows::Win32::System::Performance::PDH_HCOUNTER>,
    luid: Option<String>,
}

#[cfg(windows)]
impl GpuCounters {
    fn new(luid: Option<&str>) -> Option<Self> {
        use windows::Win32::System::Performance::{
            PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
            PdhOpenQueryW,
        };
        use windows::core::PCWSTR;

        let add = |query: PDH_HQUERY, path: &str| {
            let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
            let mut counter = PDH_HCOUNTER::default();
            // SAFETY: the path is NUL-terminated and the query is live.
            if unsafe { PdhAddEnglishCounterW(query, PCWSTR(wide.as_ptr()), 0, &mut counter) } == 0
            {
                Some(counter)
            } else {
                None
            }
        };

        // SAFETY: the query is checked before use and closed below when no
        // counter can be attached.
        unsafe {
            let mut query = PDH_HQUERY::default();
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
                return None;
            }
            let engine = add(query, r"\GPU Engine(*)\Utilization Percentage");
            let process_vram = add(query, r"\GPU Process Memory(*)\Dedicated Usage");
            let adapter_vram = add(query, r"\GPU Adapter Memory(*)\Dedicated Usage");
            if engine.is_none() && process_vram.is_none() && adapter_vram.is_none() {
                let _ = PdhCloseQuery(query);
                return None;
            }
            // The first collection only primes the rate counters.
            let _ = PdhCollectQueryData(query);
            Some(Self {
                query,
                engine,
                process_vram,
                adapter_vram,
                luid: luid.map(str::to_owned),
            })
        }
    }

    fn read(&mut self) -> Option<GpuReading> {
        use windows::Win32::System::Performance::PdhCollectQueryData;

        // SAFETY: the query is live for this struct's lifetime.
        if unsafe { PdhCollectQueryData(self.query) } != 0 {
            return None;
        }
        let engine_items = self
            .engine
            .and_then(|counter| read_counter_array(counter))
            .unwrap_or_default();
        let process_items = self
            .process_vram
            .and_then(|counter| read_counter_array(counter))
            .unwrap_or_default();
        let adapter_items = self
            .adapter_vram
            .and_then(|counter| read_counter_array(counter))
            .unwrap_or_default();

        let engine_items = filter_luid(engine_items, self.luid.as_deref());
        let process_items = filter_luid(process_items, self.luid.as_deref());
        let adapter_items = filter_luid(adapter_items, self.luid.as_deref());

        let mut total = 0.0f64;
        let mut own = 0.0f64;
        let mut three_d = 0.0f64;
        let mut compute = 0.0f64;
        let mut copy = 0.0f64;
        let mut video = 0.0f64;
        for (name, value) in &engine_items {
            let value = value.clamp(0.0, 100.0);
            total += value;
            if pid_from_instance(name) == Some(self.pid()) {
                own += value;
            }
            match engine_kind(name) {
                "3D" => three_d += value,
                "Compute" => compute += value,
                "Copy" => copy += value,
                "Video" => video += value,
                _ => {}
            }
        }

        let process_vram = sum_for_pid(&process_items, self.pid());
        let adapter_vram = sum_all(&adapter_items);
        Some(GpuReading {
            load_pct: (!engine_items.is_empty()).then_some(round1(total.min(100.0))),
            process_pct: (!engine_items.is_empty()).then_some(round1(own.min(100.0))),
            engines: vec![
                Engine {
                    name: "3D",
                    pct: round1(three_d.min(100.0)),
                },
                Engine {
                    name: "Compute",
                    pct: round1(compute.min(100.0)),
                },
                Engine {
                    name: "Copy",
                    pct: round1(copy.min(100.0)),
                },
                Engine {
                    name: "Video",
                    pct: round1(video.min(100.0)),
                },
            ],
            process_vram_mb: process_vram.map(|bytes| bytes / MIB),
            adapter_vram_mb: adapter_vram.map(|bytes| bytes / MIB),
        })
    }

    fn pid(&self) -> u32 {
        std::process::id()
    }
}

#[cfg(windows)]
impl Drop for GpuCounters {
    fn drop(&mut self) {
        // SAFETY: the query is owned by this struct and is closed once.
        unsafe {
            let _ = windows::Win32::System::Performance::PdhCloseQuery(self.query);
        }
    }
}

#[cfg(windows)]
fn filter_luid(items: Vec<(String, f64)>, luid: Option<&str>) -> Vec<(String, f64)> {
    let Some(luid) = luid else {
        return items;
    };
    let filtered: Vec<_> = items
        .iter()
        .filter(|(name, _)| luid_matches(name, luid))
        .cloned()
        .collect();
    // Some drivers spell the LUID differently. If the filter finds nothing,
    // aggregate all engines and let the page call the source "system-wide".
    if filtered.is_empty() { items } else { filtered }
}

#[cfg(windows)]
fn sum_all(items: &[(String, f64)]) -> Option<u64> {
    if items.is_empty() {
        return None;
    }
    let total: f64 = items.iter().map(|(_, value)| value.max(0.0)).sum();
    (total.is_finite() && total > 0.0).then_some(total as u64)
}

#[cfg(windows)]
fn sum_for_pid(items: &[(String, f64)], pid: u32) -> Option<u64> {
    let total: f64 = items
        .iter()
        .filter(|(name, _)| pid_from_instance(name) == Some(pid))
        .map(|(_, value)| value.max(0.0))
        .sum();
    (total.is_finite() && total > 0.0).then_some(total as u64)
}

#[cfg(windows)]
fn read_counter_array(
    counter: windows::Win32::System::Performance::PDH_HCOUNTER,
) -> Option<Vec<(String, f64)>> {
    use std::mem::size_of;
    use windows::Win32::System::Performance::{
        PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE,
        PDH_INSUFFICIENT_BUFFER, PDH_MORE_DATA, PdhGetFormattedCounterArrayW,
    };

    let mut size = 0u32;
    let mut count = 0u32;
    // SAFETY: the first call asks PDH for the required buffer size; no buffer
    // is written when the pointer is null.
    let status = unsafe {
        PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, None)
    };
    if status != PDH_MORE_DATA && status != PDH_INSUFFICIENT_BUFFER {
        return None;
    }
    if size == 0 || count == 0 {
        return Some(Vec::new());
    }
    let item_size = size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
    let slots = (size as usize).div_ceil(item_size);
    let mut buffer = vec![PDH_FMT_COUNTERVALUE_ITEM_W::default(); slots];
    let mut size = (buffer.len() * item_size) as u32;
    let mut count = 0u32;
    // SAFETY: buffer is large enough for the size PDH returned, and PDH writes
    // at most `count` initialized items into it.
    let status = unsafe {
        PdhGetFormattedCounterArrayW(
            counter,
            PDH_FMT_DOUBLE,
            &mut size,
            &mut count,
            Some(buffer.as_mut_ptr()),
        )
    };
    if status != 0 {
        return None;
    }
    let count = (count as usize).min(buffer.len());
    let mut out = Vec::with_capacity(count);
    for item in buffer.iter().take(count) {
        let status = item.FmtValue.CStatus;
        if status != PDH_CSTATUS_VALID_DATA && status != PDH_CSTATUS_NEW_DATA {
            continue;
        }
        // SAFETY: the union is a double because PDH_FMT_DOUBLE was requested.
        let value = unsafe { item.FmtValue.Anonymous.doubleValue };
        // SAFETY: szName points into PDH's buffer, still alive during this call.
        let name = unsafe { item.szName.to_string().unwrap_or_default() };
        if name.is_empty() {
            continue;
        }
        out.push((name, value));
    }
    Some(out)
}

#[cfg(windows)]
fn read_ram() -> Option<(u64, u64)> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: status is a correctly sized, initialized MEMORYSTATUSEX.
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return None;
    }
    Some((
        status.ullTotalPhys - status.ullAvailPhys,
        status.ullTotalPhys,
    ))
}

#[cfg(windows)]
fn process_working_set() -> Option<u64> {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::GetCurrentProcess;

    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    // SAFETY: counters is initialized with its own size, as the API requires.
    unsafe {
        GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb)
            .ok()
            .map(|_| counters.WorkingSetSize as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_names_are_classified() {
        assert_eq!(
            engine_kind("pid_12_luid_0x0_0x1_phys_0_eng_0_engtype_3D"),
            "3D"
        );
        assert_eq!(
            engine_kind("pid_12_luid_0x0_0x1_phys_0_eng_1_engtype_Compute"),
            "Compute"
        );
        assert_eq!(
            engine_kind("pid_12_luid_0x0_0x1_phys_0_eng_2_engtype_Copy"),
            "Copy"
        );
        assert_eq!(
            engine_kind("pid_12_luid_0x0_0x1_phys_0_eng_3_engtype_VideoDecode"),
            "Video"
        );
        assert_eq!(engine_kind("something_else"), "Other");
    }

    #[test]
    fn process_ids_are_read_from_instances() {
        assert_eq!(
            pid_from_instance("pid_4820_luid_0x0_0x1_phys_0_eng_0_engtype_3D"),
            Some(4820)
        );
        assert_eq!(pid_from_instance("luid_0x0_0x1"), None);
        assert_eq!(pid_from_instance("pid_notanumber_x"), None);
    }

    #[test]
    fn percentages_do_not_invent_zero_when_the_total_is_unknown() {
        assert_eq!(pct(5, 10), Some(50.0));
        assert_eq!(pct(0, 0), None);
        assert_eq!(round1(12.349), 12.3);
    }

    #[test]
    fn pressure_falls_back_to_clock_throttling() {
        assert_eq!(pressure(40, 100), 40);
        assert_eq!(pressure(10, 62), 38);
        assert_eq!(pressure(90, 40), 90);
    }

    #[test]
    fn history_keeps_the_newest_sixty_in_order() {
        let mut history = History::default();
        for i in 0..65u32 {
            history.push(Sample {
                ago_s: 0,
                gpu_pct: Some(i as f32),
                vram_pct: None,
                cpu_pct: None,
                ram_pct: None,
            });
        }
        let view = history.view();
        assert_eq!(view.len(), HISTORY_SAMPLES);
        assert_eq!(view.first().unwrap().gpu_pct, Some(5.0));
        assert_eq!(view.last().unwrap().gpu_pct, Some(64.0));
        assert_eq!(view.first().unwrap().ago_s, 59);
        assert_eq!(view.last().unwrap().ago_s, 0);
    }
}
