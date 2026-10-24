//! The system-memory gate: how much RAM is free, and whether loading a
//! speech model into it (or card staging) would squeeze an already-busy PC.

#[cfg(windows)]
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

/// Free RAM in MiB, or `None` when Windows doesn't answer (non-Windows
/// counts as "no reading"; the gate then never blocks).
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

/// The smallest free-RAM reading that counts as comfortable for loading a
/// `model_mb` model: on the card only staging and slack need system RAM;
/// on the CPU the model itself lives there.
pub fn needed_mb(model_mb: u32, will_use_gpu: bool) -> u64 {
    if will_use_gpu {
        u64::from(model_mb) + 1024
    } else {
        u64::from(model_mb) * 3 / 2 + 1024
    }
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
    free < needed_mb(model_mb, will_use_gpu)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn free_ram_is_reported() {
        assert!(free_ram_mb().is_some_and(|m| m > 0));
    }

    #[test]
    fn gpu_needs_less_headroom_than_cpu() {
        // On the card the model lands in VRAM; the CPU path must hold the
        // whole model in system RAM plus working space.
        assert!(needed_mb(834, true) < needed_mb(834, false));
    }

    #[test]
    fn tight_by_threshold_without_override() {
        assert!(!is_tight(Some(64_000), 834, true));
        assert!(is_tight(Some(1_000), 834, false)); // CPU needs 2_275
        assert!(!is_tight(Some(3_000), 834, false));
        assert!(is_tight(Some(1_000), 834, true)); // GPU needs 1_858
        // No reading means never blocked; the load fails on its own if so.
        assert!(!is_tight(None, 834, false));

        // The override forces the threshold, and removing it restores the
        // formula. One test on purpose: env vars are process-wide and
        // these assertions share it (tests run in parallel otherwise).
        unsafe { std::env::set_var("SRUTHI_GATE_MIN_FREE_MB", "999999") };
        assert!(is_tight(Some(64_000), 834, true));
        unsafe { std::env::remove_var("SRUTHI_GATE_MIN_FREE_MB") };
        assert!(!is_tight(Some(64_000), 834, true));
    }
}
