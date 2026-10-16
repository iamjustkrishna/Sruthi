//! Microphone capture. cpal streams are not `Send`, so each recording owns a
//! dedicated thread that holds the stream until it is told to stop.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use anyhow::{anyhow, bail, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

/// Sample rate every speech model in this app expects.
pub const TARGET_RATE: u32 = 16_000;

/// A live microphone capture. The audio stays readable while recording
/// (for live transcription) and after `stop()`.
pub struct Recording {
    stop_tx: mpsc::Sender<()>,
    level: Arc<AtomicU32>,
    /// Mono samples at the device's own rate.
    buffer: Arc<Mutex<Vec<f32>>>,
    rate: u32,
    handle: Mutex<Option<JoinHandle<Result<()>>>>,
}

impl Recording {
    /// Starts capturing mono audio from the default input device.
    pub fn start() -> Result<Self> {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<u32>();
        let level = Arc::new(AtomicU32::new(0));
        let buffer = Arc::new(Mutex::new(Vec::<f32>::new()));
        let (level_cb, buffer_cb) = (level.clone(), buffer.clone());

        let handle = thread::spawn(move || -> Result<()> {
            let device = cpal::default_host()
                .default_input_device()
                .ok_or_else(|| anyhow!("No microphone found"))?;
            let supported = device.default_input_config()?;
            let rate = supported.sample_rate();
            let channels = supported.channels() as usize;
            let config = supported.config();
            buffer_cb.lock().unwrap().reserve(rate as usize * 30);

            let stream = match supported.sample_format() {
                SampleFormat::F32 => build::<f32>(&device, config, channels, buffer_cb, level_cb)?,
                SampleFormat::I16 => build::<i16>(&device, config, channels, buffer_cb, level_cb)?,
                SampleFormat::U16 => build::<u16>(&device, config, channels, buffer_cb, level_cb)?,
                SampleFormat::I32 => build::<i32>(&device, config, channels, buffer_cb, level_cb)?,
                other => bail!("Unsupported microphone sample format: {other:?}"),
            };
            stream.play()?;
            let _ = ready_tx.send(rate);

            // Blocks until stop() is called (or the Recording is dropped).
            let _ = stop_rx.recv();
            drop(stream);
            Ok(())
        });

        match ready_rx.recv() {
            Ok(rate) => Ok(Self { stop_tx, level, buffer, rate, handle: Mutex::new(Some(handle)) }),
            // The thread exited before the stream started: surface its error.
            Err(_) => match handle.join() {
                Ok(Err(e)) => Err(e),
                _ => Err(anyhow!("Microphone thread stopped unexpectedly")),
            },
        }
    }

    /// Live input loudness (RMS of the latest buffer, 0.0–1.0) for meters.
    pub fn level(&self) -> Arc<AtomicU32> {
        self.level.clone()
    }

    /// Number of samples captured so far, at the device rate.
    pub fn len(&self) -> usize {
        self.buffer.lock().unwrap().len()
    }

    /// Samples `from..to` (device-rate indices), converted to 16 kHz.
    pub fn slice_16k(&self, from: usize, to: usize) -> Vec<f32> {
        let buf = self.buffer.lock().unwrap();
        let to = to.min(buf.len());
        let from = from.min(to);
        resample(&buf[from..to], self.rate, TARGET_RATE)
    }

    /// Stops capturing. The captured audio stays readable.
    pub fn stop(&self) -> Result<()> {
        let _ = self.stop_tx.send(());
        if let Some(handle) = self.handle.lock().unwrap().take() {
            handle.join().map_err(|_| anyhow!("Microphone thread panicked"))??;
        }
        Ok(())
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    buffer: Arc<Mutex<Vec<f32>>>,
    level: Arc<AtomicU32>,
) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let mut buf = buffer.lock().unwrap();
            let start = buf.len();
            // Downmix each frame to mono by averaging its channels.
            for frame in data.chunks(channels) {
                let sum: f32 = frame.iter().map(|s| f32::from_sample_(*s)).sum();
                buf.push(sum / channels as f32);
            }
            let chunk = &buf[start..];
            if !chunk.is_empty() {
                let rms = (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32).sqrt();
                level.store(rms.to_bits(), Ordering::Relaxed);
            }
        },
        |err| log::error!("Microphone stream error: {err}"),
        None,
    )?;
    Ok(stream)
}

/// Converts `input` from `from` Hz to `to` Hz. When downsampling, a box filter
/// the width of the ratio runs first so high frequencies don't alias into the
/// speech band; linear interpolation then picks the output samples.
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from as f64 / to as f64;

    let filtered: Vec<f32> = if ratio > 1.0 {
        let width = ratio.round().max(1.0) as usize;
        let mut out = Vec::with_capacity(input.len());
        let mut acc = 0.0f32;
        for (i, &s) in input.iter().enumerate() {
            acc += s;
            if i >= width {
                acc -= input[i - width];
            }
            out.push(acc / width.min(i + 1) as f32);
        }
        out
    } else {
        input.to_vec()
    };

    let out_len = (input.len() as f64 / ratio).floor() as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let idx = pos.floor() as usize;
            let frac = (pos - idx as f64) as f32;
            let a = filtered[idx];
            let b = *filtered.get(idx + 1).unwrap_or(&a);
            a + (b - a) * frac
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_48k_to_16k_keeps_duration_and_level() {
        let input = vec![0.5f32; 48_000];
        let out = resample(&input, 48_000, TARGET_RATE);
        assert_eq!(out.len(), 16_000);
        assert!(out[10..].iter().all(|s| (s - 0.5).abs() < 1e-5));
    }

    #[test]
    fn resample_same_rate_is_identity() {
        let input = vec![0.1, -0.2, 0.3];
        assert_eq!(resample(&input, 16_000, 16_000), input);
    }

    #[test]
    fn resample_44_1k_to_16k_has_expected_length() {
        let out = resample(&vec![0.0; 44_100], 44_100, TARGET_RATE);
        assert_eq!(out.len(), 16_000);
    }
}
