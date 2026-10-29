// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Sruthi contributors

//! Reads Windows' own volume and mute state for each microphone, so the app
//! can warn when the one it records from is muted or turned down to
//! nothing. (A muted mic doesn't fail: it just delivers silence.)

#[derive(Clone, Debug, serde::Serialize)]
pub struct MicInfo {
    pub name: String,
    /// Windows input volume, 0.0–1.0.
    pub volume: f32,
    pub muted: bool,
    /// Windows' default recording device (the one the app uses).
    pub is_default: bool,
}

#[cfg(windows)]
pub fn microphones() -> Vec<MicInfo> {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{
        DEVICE_STATE_ACTIVE, IMMDeviceEnumerator, MMDeviceEnumerator, eCapture, eConsole,
    };
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
        STGM_READ,
    };

    // SAFETY: COM is initialized for this call and every interface is
    // dropped before CoUninitialize.
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = (|| -> windows::core::Result<Vec<MicInfo>> {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let default_id = enumerator
                .GetDefaultAudioEndpoint(eCapture, eConsole)
                .and_then(|d| d.GetId())
                .ok()
                .and_then(|id| id.to_string().ok());
            let devices = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;
            let mut mics = Vec::new();
            for i in 0..devices.GetCount()? {
                let device = devices.Item(i)?;
                let id = device.GetId().ok().and_then(|id| id.to_string().ok());
                let name = device
                    .OpenPropertyStore(STGM_READ)
                    .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName))
                    .map(|value| value.to_string())
                    .unwrap_or_else(|_| "Unknown microphone".into());
                let volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;
                mics.push(MicInfo {
                    name,
                    volume: volume.GetMasterVolumeLevelScalar().unwrap_or(1.0),
                    muted: volume.GetMute().map(|m| m.as_bool()).unwrap_or(false),
                    is_default: id.is_some() && id == default_id,
                });
            }
            Ok(mics)
        })();
        if initialized {
            CoUninitialize();
        }
        result.unwrap_or_default()
    }
}

#[cfg(not(windows))]
pub fn microphones() -> Vec<MicInfo> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    /// Prints every microphone's Windows volume and mute state:
    /// `cargo test --lib list_microphones -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn list_microphones() {
        for m in super::microphones() {
            println!(
                "{}{}: volume {:.0}%{}",
                m.name,
                if m.is_default { " (default)" } else { "" },
                m.volume * 100.0,
                if m.muted { ", MUTED" } else { "" }
            );
        }
    }
}
