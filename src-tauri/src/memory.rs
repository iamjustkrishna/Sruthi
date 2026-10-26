//! The system-memory gate: how much RAM is free, and whether loading a
//! speech model into it would squeeze an already-busy PC.

#[cfg(windows)]
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

/// Free RAM in MiB, or `None` when the OS doesn't answer (the gate then
/// never blocks). Windows reads it from GlobalMemoryStatusEx, macOS from
/// `vm_stat`; other systems count as "no reading".
#[cfg(windows)]
pub fn free_ram_mb() -> Option<u64> {
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: GlobalMemoryStatusEx only fills in the struct passed in; the
    // length field above is set first, as its docs require.
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return None;
    }
    Some(status.ullAvailPhys >> 20)
}

/// macOS has no GlobalMemoryStatusEx; `vm_stat` counts memory in pages
/// ("Pages free: 12345."), each `hw.pagesize` bytes. Free + speculative +
/// purgeable is the memory an app could realistically receive right now.
#[cfg(target_os = "macos")]
pub fn free_ram_mb() -> Option<u64> {
    let out = std::process::Command::new("/usr/bin/vm_stat")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let page_size: u64 = {
        let out = std::process::Command::new("/usr/sbin/sysctl")
            .args(["hw.pagesize"])
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()?
    };
    let pages = |label: &str| -> u64 {
        text.lines()
            .find_map(|l| {
                l.strip_prefix(label)?
                    .trim()
                    .trim_end_matches('.')
                    .parse::<u64>()
                    .ok()
            })
            .unwrap_or(0)
    };
    let free_pages =
        pages("Pages free: ") + pages("Pages speculative: ") + pages("Pages purgeable: ");
    Some(free_pages.saturating_mul(page_size) >> 20)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn free_ram_mb() -> Option<u64> {
    None
}

/// Room above the model file for the runtime and its working set — the
/// free RAM a CPU load wants before it's fair to the rest of the PC.
const CPU_HEADROOM_MB: u64 = 512;

/// Below this much free RAM Windows itself is close to the edge, so even a
/// card load (which keeps the model in VRAM) is worth asking about.
const CARD_GATE_MIN_FREE_MB: u64 = 400;

/// The free RAM a `model_mb` model wants on the CPU path.
pub fn needed_mb(model_mb: u32) -> u64 {
    u64::from(model_mb) + CPU_HEADROOM_MB
}

/// True when loading a `model_mb` model on this PC's current free memory is
/// the kind of squeeze the user should be asked about. `SRUTHI_GATE_MIN_FREE_MB`
/// forces the threshold for testing.
pub fn is_tight(free: Option<u64>, model_mb: u32, will_use_gpu: bool) -> bool {
    let Some(free) = free else {
        return false;
    };
    if let Ok(min) = std::env::var("SRUTHI_GATE_MIN_FREE_MB") {
        if let Ok(min) = min.parse::<u64>() {
            return free < min;
        }
    }
    if will_use_gpu {
        // The weights live in VRAM; system RAM only stages the read, so a
        // normally busy PC needs no question. Ask only near the edge.
        free < CARD_GATE_MIN_FREE_MB
    } else {
        free < needed_mb(model_mb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn free_ram_is_reported() {
        assert!(free_ram_mb().is_some_and(|m| m > 0));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn free_ram_is_reported() {
        assert!(free_ram_mb().is_some_and(|m| m > 0));
    }

    #[test]
    fn needed_mb_is_the_model_plus_headroom() {
        assert_eq!(needed_mb(834), 834 + CPU_HEADROOM_MB);
    }

    // One test on purpose: env vars are process-wide and these assertions
    // share the override (tests otherwise run in parallel).
    #[test]
    fn gate_thresholds() {
        // Apex is 834 MB; at 1.0 GB free a CPU load is a real squeeze.
        assert!(is_tight(Some(1_000), 834, false));
        assert!(!is_tight(Some(3_000), 834, false));
        // Parakeet V3 is 478 MB: the usual busy PC still fits it.
        assert!(!is_tight(Some(1_117), 478, false));
        // The card keeps the weights in VRAM: ~1 GB free is normal, no ask.
        assert!(!is_tight(Some(1_117), 834, true));
        assert!(!is_tight(Some(64_000), 834, true));
        assert!(is_tight(Some(300), 834, true));
        // No reading means never blocked; the load fails on its own if so.
        assert!(!is_tight(None, 834, false));

        // The override forces the threshold, and removing it restores the
        // formula.
        unsafe { std::env::set_var("SRUTHI_GATE_MIN_FREE_MB", "999999") };
        assert!(is_tight(Some(64_000), 834, true));
        unsafe { std::env::remove_var("SRUTHI_GATE_MIN_FREE_MB") };
        assert!(!is_tight(Some(64_000), 834, true));
    }
}
