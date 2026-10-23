//! Direct Windows (WASAPI) microphone capture in **raw mode**. Raw mode
//! asks Windows to skip the driver's audio effects (noise suppression,
//! noise gates, "enhancements"). On this laptop those effects turned all
//! quiet sound, whispers included, into exact silence before any app saw
//! it. Chromium-based apps capture this way and then do their own, gentler
//! processing; we do the same (see `enhance.rs`).

#![cfg(windows)]

use std::time::Duration;

use anyhow::{Result, anyhow};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMOPTIONS_RAW,
    AudioCategory_Speech, AudioClientProperties, DEVICE_STATE_ACTIVE, IAudioCaptureClient,
    IAudioClient, IAudioClient2, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, eCapture,
    eConsole,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, STGM_READ,
};
use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
};
use windows::core::Interface;

/// How often captured audio is collected.
const POLL: Duration = Duration::from_millis(10);

fn enumerator() -> Result<IMMDeviceEnumerator> {
    // SAFETY: COM may already be initialized on this thread; that's fine.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        Ok(CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?)
    }
}

fn friendly_name(device: &IMMDevice) -> String {
    // SAFETY: read-only property lookup on a valid device.
    unsafe {
        device
            .OpenPropertyStore(STGM_READ)
            .and_then(|s| s.GetValue(&PKEY_Device_FriendlyName))
            .map(|v| v.to_string())
            .unwrap_or_default()
    }
}

/// Name of Windows' default recording device.
pub fn default_device_name() -> Option<String> {
    let enumerator = enumerator().ok()?;
    // SAFETY: plain COM call on a valid enumerator.
    let device = unsafe {
        enumerator
            .GetDefaultAudioEndpoint(eCapture, eConsole)
            .ok()?
    };
    Some(friendly_name(&device))
}

/// Names of all active recording devices.
pub fn device_names() -> Vec<String> {
    let Ok(enumerator) = enumerator() else {
        return Vec::new();
    };
    // SAFETY: plain COM enumeration.
    unsafe {
        let Ok(all) = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) else {
            return Vec::new();
        };
        (0..all.GetCount().unwrap_or(0))
            .filter_map(|i| all.Item(i).ok())
            .map(|d| friendly_name(&d))
            .collect()
    }
}

/// The device called `name`, or Windows' default when `name` is None.
fn find_device(name: Option<&str>) -> Result<(IMMDevice, String)> {
    let enumerator = enumerator()?;
    // SAFETY: plain COM calls on a valid enumerator.
    unsafe {
        match name {
            None => {
                let device = enumerator.GetDefaultAudioEndpoint(eCapture, eConsole)?;
                let name = friendly_name(&device);
                Ok((device, name))
            }
            Some(want) => {
                let all = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;
                for i in 0..all.GetCount()? {
                    let device = all.Item(i)?;
                    let name = friendly_name(&device);
                    if name == want {
                        return Ok((device, name));
                    }
                }
                Err(anyhow!("Microphone \"{want}\" isn't connected"))
            }
        }
    }
}

/// Captures mono f32 audio from `device` (None: Windows' default) until
/// `keep_going` returns false. `ready(rate, device_name)` is called once
/// audio is flowing; `sink` receives every chunk.
pub fn capture(
    device: Option<&str>,
    ready: &mut dyn FnMut(u32, &str),
    sink: &mut dyn FnMut(&[f32]),
    keep_going: &mut dyn FnMut() -> bool,
) -> Result<()> {
    let (device, name) = find_device(device)?;
    // SAFETY: standard WASAPI shared-mode capture. The packet buffer is only
    // read between GetBuffer and ReleaseBuffer; the mix format is freed.
    unsafe {
        let client2: IAudioClient2 = device.Activate(CLSCTX_ALL, None)?;
        let props = AudioClientProperties {
            cbSize: std::mem::size_of::<AudioClientProperties>() as u32,
            bIsOffload: false.into(),
            eCategory: AudioCategory_Speech,
            Options: AUDCLNT_STREAMOPTIONS_RAW,
        };
        if client2.SetClientProperties(&props).is_err() {
            log::info!("\"{name}\" doesn't offer raw capture; using its processed audio");
        }
        let client: IAudioClient = client2.cast()?;
        let format = client.GetMixFormat()?;
        let (rate, channels, bits) = (
            (*format).nSamplesPerSec,
            (*format).nChannels as usize,
            (*format).wBitsPerSample,
        );
        // Room for 100 ms of audio, and a high thread priority: the speech
        // model transcribes in the background while the key is held, and
        // the microphone must not lose audio when the CPU is busy.
        let init = client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 1_000_000, 0, format, None);
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
        CoTaskMemFree(Some(format as *const _));
        init?;
        if bits != 32 && bits != 16 {
            return Err(anyhow!("Unsupported microphone format ({bits}-bit)"));
        }
        let capture: IAudioCaptureClient = client.GetService()?;
        client.Start()?;
        ready(rate, &name);

        let mut mono: Vec<f32> = Vec::with_capacity(rate as usize / 10);
        let result = (|| -> Result<()> {
            while keep_going() {
                std::thread::sleep(POLL);
                while capture.GetNextPacketSize()? > 0 {
                    let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
                    capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                    let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                    mono.clear();
                    for f in 0..frames as usize {
                        let mut sum = 0.0f32;
                        if !silent {
                            for c in 0..channels {
                                let i = f * channels + c;
                                sum += if bits == 32 {
                                    *(data as *const f32).add(i)
                                } else {
                                    *(data as *const i16).add(i) as f32 / 32768.0
                                };
                            }
                        }
                        mono.push(sum / channels as f32);
                    }
                    capture.ReleaseBuffer(frames)?;
                    sink(&mono);
                }
            }
            Ok(())
        })();
        let _ = client.Stop();
        result
    }
}

/// Records `secs` seconds from `device` and returns (rms, peak). Used to
/// tell a working microphone (always some room noise in raw mode) from a
/// dead one (exact silence).
pub fn probe(device: &str, secs: f32) -> Result<(f32, f32)> {
    let deadline = std::time::Instant::now() + Duration::from_secs_f32(secs);
    let mut samples: Vec<f32> = Vec::new();
    capture(
        Some(device),
        &mut |_, _| {},
        &mut |chunk| samples.extend_from_slice(chunk),
        &mut || std::time::Instant::now() < deadline,
    )?;
    let rms = (samples.iter().map(|v| v * v).sum::<f32>() / samples.len().max(1) as f32).sqrt();
    let peak = samples.iter().fold(0f32, |m, v| m.max(v.abs()));
    Ok((rms, peak))
}

#[cfg(test)]
mod tests {
    /// Probes each microphone in raw mode:
    /// `cargo test --lib probe_all -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn probe_all() {
        println!("default: {:?}", super::default_device_name());
        for name in super::device_names() {
            match super::probe(&name, 1.0) {
                Ok((rms, peak)) => println!("{name}: rms={rms:.6} peak={peak:.4}"),
                Err(e) => println!("{name}: {e}"),
            }
        }
    }
}
