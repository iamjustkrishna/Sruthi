// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Sruthi contributors

//! Picks the graphics card for Whisper and the AI clean-up.
//!
//! Laptops often have two: the processor's built-in graphics (which shares
//! system RAM) and a dedicated card. whisper.cpp and llama.cpp list the
//! built-in one first, so Whisper ran there (~1.7x slower) and Qwen got
//! split across both. Hiding every GPU but the dedicated one fixes that.

use std::process::Command;
use std::sync::OnceLock;

use ash::vk;

/// Vulkan's index of the dedicated graphics card, if there is one.
pub fn dedicated() -> Option<usize> {
    info().map(|(index, _, _)| index)
}

/// The dedicated card's name ("AMD Radeon RX 6500M"), for the main window.
pub fn device_name() -> Option<String> {
    info().map(|(_, name, _)| name)
}

/// The dedicated card's own memory in MiB (what a model has to fit in).
pub fn vram_mb() -> Option<u64> {
    info().map(|(_, _, vram)| vram)
}

/// The dedicated card, found once and cached: Vulkan device index, name and
/// graphics memory in MiB.
fn info() -> Option<(usize, String, u64)> {
    static INFO: OnceLock<Option<(usize, String, u64)>> = OnceLock::new();
    let found = INFO.get_or_init(|| {
        let found = find_dedicated();
        match &found {
            Some((index, name, vram)) => {
                log::info!(
                    "Graphics card for Whisper and the AI: {name}, {vram} MB (Vulkan device {index})"
                )
            }
            None => log::info!(
                "No dedicated graphics card found; whisper.cpp and llama.cpp pick their own"
            ),
        }
        found
    });
    found.clone()
}

/// Lets a whisper.cpp or llama.cpp process see only the dedicated card
/// (unless the user already chose one with GGML_VK_VISIBLE_DEVICES).
pub fn use_dedicated(cmd: &mut Command) {
    if std::env::var_os("GGML_VK_VISIBLE_DEVICES").is_some() {
        return;
    }
    if let Some(index) = dedicated() {
        cmd.env("GGML_VK_VISIBLE_DEVICES", index.to_string());
    }
}

fn find_dedicated() -> Option<(usize, String, u64)> {
    // SAFETY: plain Vulkan queries; the instance is destroyed before
    // returning and nothing created from it outlives it.
    unsafe {
        let entry = ash::Entry::load().ok()?;
        let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
        let instance = entry
            .create_instance(
                &vk::InstanceCreateInfo::default().application_info(&app),
                None,
            )
            .ok()?;
        // Same order whisper.cpp and llama.cpp number them in.
        let found = instance
            .enumerate_physical_devices()
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .find_map(|(index, device)| {
                let props = instance.get_physical_device_properties(device);
                (props.device_type == vk::PhysicalDeviceType::DISCRETE_GPU).then(|| {
                    let memory = instance.get_physical_device_memory_properties(device);
                    let vram: u64 = memory.memory_heaps[..memory.memory_heap_count as usize]
                        .iter()
                        .filter(|heap| heap.flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL))
                        .map(|heap| heap.size >> 20)
                        .sum();
                    (
                        index,
                        props
                            .device_name_as_c_str()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        vram,
                    )
                })
            });
        instance.destroy_instance(None);
        found
    }
}
