// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Sruthi contributors

//! First run: what this PC can do, and which speech model fits it.
//!
//! `report` reads the hardware (nothing is sent anywhere) and `recommend` is
//! a plain function from that report and the languages the user picked to one
//! model plus a few alternatives, so the rules are easy to test and to change.

use std::process::Command;

use serde::Serialize;

use crate::{gpu, memory, process, settings};

/// A dedicated card needs about this much memory of its own to run the
/// Whisper models comfortably.
const CARD_VRAM_MB: u64 = 4096;
/// Below this much free memory, a PC wants the smallest model.
const LOW_FREE_RAM_MB: u64 = 3072;
/// With no dedicated card, this much installed memory or less counts as light.
const LIGHT_TOTAL_RAM_MB: u64 = 8192;

#[derive(Clone, Debug, Default, Serialize)]
pub struct Hardware {
    pub cpu: Option<String>,
    pub cores: usize,
    pub ram_total_mb: Option<u64>,
    pub ram_free_mb: Option<u64>,
    pub gpu: Option<String>,
    pub vram_mb: Option<u64>,
    pub disk_free_mb: Option<u64>,
    /// "windows", "macos" and so on, for wording the hardware tiles.
    pub platform: &'static str,
    /// Whether the Whisper engine runs here (Windows only for now). Without
    /// it only the Parakeet and Moonshine models are on offer.
    pub whisper_engine: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    Good,
    Slow,
    /// Nothing worth saying either way.
    Plain,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Pick {
    pub id: &'static str,
    /// One plain sentence on why this model suits this PC.
    pub why: &'static str,
    pub fit: Fit,
    /// The label that goes with `fit` ("Good fit: you have a graphics card").
    pub note: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct Plan {
    pub hardware: Hardware,
    pub primary: Pick,
    pub alternatives: Vec<Pick>,
    /// Something the user should know about their choices on this OS.
    pub notice: Option<&'static str>,
}

pub fn report() -> Hardware {
    Hardware {
        cpu: cpu_name(),
        cores: std::thread::available_parallelism().map_or(0, |n| n.get()),
        ram_total_mb: memory::total_ram_mb(),
        ram_free_mb: memory::free_ram_mb(),
        gpu: gpu::device_name(),
        vram_mb: gpu::vram_mb(),
        disk_free_mb: disk_free_mb(),
        platform: std::env::consts::OS,
        whisper_engine: cfg!(windows),
    }
}

pub fn plan(languages: &[String]) -> Plan {
    let hardware = report();
    let (primary, alternatives) = recommend(&hardware, languages);
    let wants_whisper = says(languages, "hinglish") || says(languages, "other");
    let notice = (!hardware.whisper_engine && wants_whisper).then_some(
        "The models for Hindi + English and other languages need Windows for now, so Parakeet V3 is the closest fit here.",
    );
    Plan {
        hardware,
        primary,
        alternatives,
        notice,
    }
}

/// First run means no choice was ever recorded and no model is on disk yet.
/// Anyone who already has a model installed is an existing user and is never
/// shown the setup, even if their settings file predates it.
pub fn needs_onboarding(onboarded: Option<bool>, any_model_installed: bool) -> bool {
    onboarded.is_none() && !any_model_installed
}

fn says(languages: &[String], key: &str) -> bool {
    languages.iter().any(|l| l == key)
}

/// The model to suggest and up to three others worth showing.
pub fn recommend(hw: &Hardware, languages: &[String]) -> (Pick, Vec<Pick>) {
    let has_card = hw.vram_mb.is_some_and(|v| v >= CARD_VRAM_MB);
    let low_free = hw.ram_free_mb.is_some_and(|f| f < LOW_FREE_RAM_MB);
    let light =
        low_free || (hw.gpu.is_none() && hw.ram_total_mb.is_some_and(|t| t <= LIGHT_TOTAL_RAM_MB));
    let hinglish = says(languages, "hinglish");
    let european = says(languages, "european");
    let other = says(languages, "other");
    let english = says(languages, "english");
    // Nothing picked (a skipped step) means plain English.
    let english_only = !hinglish && !european && !other;
    // Another language and nothing Parakeet or the Hinglish models cover.
    let other_only = other && !english && !european && !hinglish;

    let whisper_fit = |on_card: &'static str, off_card: &'static str| {
        if has_card {
            (Fit::Good, on_card)
        } else {
            (Fit::Slow, off_card)
        }
    };

    let pick = |id: &'static str| -> Pick {
        let (why, fit, note) = match id {
            "parakeet-v3" => (
                "Best all-round accuracy, and quick on any modern processor. Covers English and 24 European languages.",
                Fit::Plain,
                "",
            ),
            "moonshine-base" => (
                "Small and fast. A little less accurate than Parakeet, but easy on a PC that is short on memory.",
                Fit::Good,
                "Light on memory",
            ),
            "hybrid" => {
                let (fit, note) = whisper_fit(
                    "Good fit: you have a graphics card",
                    "Slower without a graphics card",
                );
                (
                    "Parakeet's speed for normal speech, with a second pass on whispers and rare words.",
                    fit,
                    note,
                )
            }
            "hinglish-apex" => {
                let (fit, note) = whisper_fit(
                    "Good fit: you have a graphics card",
                    "Slow without a graphics card",
                );
                (
                    "Built for Hindi and English mixed speech, typed in Roman letters.",
                    fit,
                    note,
                )
            }
            "hinglish-swift" => (
                "Small and quick, and types Hindi and English mixed speech in Roman letters.",
                Fit::Good,
                "Quick on any PC",
            ),
            "whisper-tiny" => (
                "The smallest multilingual model. It understands many languages but is rough; you can change it in Models.",
                Fit::Plain,
                "",
            ),
            other => unreachable!("not a first-run model: {other}"),
        };
        Pick { id, why, fit, note }
    };

    // The Whisper models (Hinglish, tiny, hybrid) need the Whisper engine.
    let whisper = hw.whisper_engine;
    let primary_id = if hinglish && whisper {
        if has_card {
            "hinglish-apex"
        } else {
            "hinglish-swift"
        }
    } else if other_only && whisper {
        "whisper-tiny"
    } else if light && english_only {
        "moonshine-base"
    } else {
        "parakeet-v3"
    };

    let mut wanted: Vec<&'static str> = Vec::new();
    if !(other_only && whisper) {
        wanted.push("parakeet-v3");
    }
    if hinglish && whisper {
        wanted.push(if primary_id == "hinglish-apex" {
            "hinglish-swift"
        } else {
            "hinglish-apex"
        });
    }
    if !light && english_only && whisper {
        wanted.push("hybrid");
    }
    if english_only {
        wanted.push("moonshine-base");
    }

    let alternatives = wanted
        .into_iter()
        .filter(|id| *id != primary_id)
        .take(3)
        .map(pick)
        .collect();
    (pick(primary_id), alternatives)
}

fn cpu_name() -> Option<String> {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("reg");
        cmd.args([
            "query",
            r"HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0",
            "/v",
            "ProcessorNameString",
        ]);
        process::hide_window(&mut cmd);
        let out = cmd.output().ok()?;
        // "    ProcessorNameString    REG_SZ    AMD Ryzen 5 5600H with Radeon Graphics"
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .find_map(|line| line.split_once("REG_SZ"))
            .map(|(_, name)| name.trim().to_string())
            .filter(|name| !name.is_empty())
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("sysctl");
        cmd.args(["-n", "machdep.cpu.brand_string"]);
        process::hide_window(&mut cmd);
        let out = cmd.output().ok()?;
        let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!name.is_empty()).then_some(name)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        None
    }
}

/// Free space on the drive that holds Sruthi's data, in MiB.
fn disk_free_mb() -> Option<u64> {
    let root = settings::app_root();
    std::fs::create_dir_all(&root).ok()?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let wide: Vec<u16> = root.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut free: u64 = 0;
        // SAFETY: `wide` is NUL-terminated and outlives the call; the three
        // out-pointers are a valid u64 or null, which the API allows.
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        (ok != 0).then_some(free >> 20)
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("df");
        cmd.arg("-k").arg(&root);
        let out = cmd.output().ok()?;
        // Second line, fourth column: available 1K blocks.
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .nth(1)?
            .split_whitespace()
            .nth(3)?
            .parse::<u64>()
            .ok()
            .map(|kb| kb >> 10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pc(ram_total: u64, ram_free: u64, vram: Option<u64>) -> Hardware {
        Hardware {
            cpu: Some("Test CPU".into()),
            cores: 8,
            ram_total_mb: Some(ram_total),
            ram_free_mb: Some(ram_free),
            gpu: vram.map(|_| "Test GPU".into()),
            vram_mb: vram,
            disk_free_mb: Some(100_000),
            platform: "windows",
            whisper_engine: true,
        }
    }

    fn mac(ram_total: u64, ram_free: u64) -> Hardware {
        Hardware {
            platform: "macos",
            whisper_engine: false,
            ..pc(ram_total, ram_free, None)
        }
    }

    fn langs(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_ordinary_pc_gets_parakeet_v3() {
        let (primary, _) = recommend(&pc(16_384, 9_000, None), &langs(&["english"]));
        assert_eq!(primary.id, "parakeet-v3");
    }

    #[test]
    fn skipping_the_language_step_means_english() {
        let (primary, _) = recommend(&pc(16_384, 9_000, None), &[]);
        assert_eq!(primary.id, "parakeet-v3");
    }

    #[test]
    fn a_pc_short_on_memory_gets_the_small_model() {
        let (primary, _) = recommend(&pc(8_192, 2_000, None), &langs(&["english"]));
        assert_eq!(primary.id, "moonshine-base");
        let (primary, _) = recommend(&pc(8_192, 5_000, None), &[]);
        assert_eq!(primary.id, "moonshine-base");
    }

    #[test]
    fn european_languages_never_get_the_english_only_small_model() {
        let (primary, _) = recommend(&pc(8_192, 2_000, None), &langs(&["english", "european"]));
        assert_eq!(primary.id, "parakeet-v3");
    }

    #[test]
    fn hinglish_follows_the_graphics_card() {
        let (with_card, _) = recommend(&pc(16_384, 9_000, Some(6_144)), &langs(&["hinglish"]));
        assert_eq!(with_card.id, "hinglish-apex");
        let (without, _) = recommend(&pc(16_384, 9_000, None), &langs(&["hinglish"]));
        assert_eq!(without.id, "hinglish-swift");
        let (small_card, _) = recommend(&pc(16_384, 9_000, Some(2_048)), &langs(&["hinglish"]));
        assert_eq!(small_card.id, "hinglish-swift");
    }

    #[test]
    fn another_language_alone_gets_whisper_tiny() {
        let (primary, alternatives) = recommend(&pc(16_384, 9_000, None), &langs(&["other"]));
        assert_eq!(primary.id, "whisper-tiny");
        assert!(alternatives.iter().all(|a| a.id != "parakeet-v3"));
    }

    #[test]
    fn hybrid_is_a_good_fit_only_with_a_card() {
        let (_, with_card) = recommend(&pc(16_384, 9_000, Some(6_144)), &[]);
        let hybrid = with_card.iter().find(|a| a.id == "hybrid").unwrap();
        assert_eq!(hybrid.fit, Fit::Good);
        let (_, without) = recommend(&pc(16_384, 9_000, None), &[]);
        let hybrid = without.iter().find(|a| a.id == "hybrid").unwrap();
        assert_eq!(hybrid.fit, Fit::Slow);
    }

    #[test]
    fn alternatives_are_distinct_short_and_real_models() {
        let cases = [
            (pc(16_384, 9_000, Some(6_144)), langs(&[])),
            (pc(8_192, 2_000, None), langs(&["english"])),
            (pc(16_384, 9_000, None), langs(&["hinglish"])),
            (pc(16_384, 9_000, None), langs(&["other"])),
            (
                pc(16_384, 9_000, None),
                langs(&["english", "european", "hinglish"]),
            ),
        ];
        for (hw, languages) in cases {
            let (primary, alternatives) = recommend(&hw, &languages);
            assert!(alternatives.len() <= 3);
            assert!(crate::model::find(primary.id).is_some());
            for (i, a) in alternatives.iter().enumerate() {
                assert_ne!(a.id, primary.id);
                assert!(
                    crate::model::find(a.id).is_some(),
                    "{} not in the catalog",
                    a.id
                );
                assert!(alternatives[..i].iter().all(|b| b.id != a.id));
            }
        }
    }

    #[test]
    fn a_mac_is_only_offered_models_it_can_run() {
        for languages in [
            langs(&[]),
            langs(&["hinglish"]),
            langs(&["other"]),
            langs(&["english", "european"]),
        ] {
            let (primary, alternatives) = recommend(&mac(16_384, 9_000), &languages);
            for pick in std::iter::once(&primary).chain(alternatives.iter()) {
                let info = crate::model::find(pick.id).unwrap();
                assert!(!info.uses_whisper(), "{} needs the Whisper engine", pick.id);
            }
            assert_eq!(primary.id, "parakeet-v3");
        }
        let (small, _) = recommend(&mac(8_192, 2_000), &langs(&["english"]));
        assert_eq!(small.id, "moonshine-base");
    }

    #[test]
    fn only_a_fresh_install_sees_the_setup() {
        assert!(needs_onboarding(None, false));
        assert!(!needs_onboarding(None, true), "an existing user");
        assert!(
            !needs_onboarding(Some(true), false),
            "already finished or skipped"
        );
    }
}
