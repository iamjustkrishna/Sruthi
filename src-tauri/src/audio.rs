//! Microphone capture, two ways:
//!
//! - [`Mic`] keeps the microphone open while the app runs and remembers the
//!   last half second, so recording starts instantly and even includes a
//!   word spoken as the key went down. Windows shows its "microphone in
//!   use" icon the whole time; audio is only kept while the key is held.
//! - [`Recording::start`] opens the microphone for one recording only. On
//!   this machine that took 150–200 ms, enough to lose the first syllable.
//!
//! On Windows, audio comes from WASAPI in raw mode (see `wasapi.rs`), which
//! skips driver effects that turned whispers into silence. Elsewhere, cpal.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::{Arc, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};

/// Sample rate every speech model in this app expects.
pub const TARGET_RATE: u32 = 16_000;

/// Audio kept from just before the key press.
const PRE_ROLL_SECS: f32 = 0.5;
/// How often the always-ready mic checks the default microphone.
const DEVICE_CHECK: Duration = Duration::from_secs(2);
/// Wait before reopening a microphone that failed.
const RETRY_DELAY: Duration = Duration::from_secs(2);
/// Below this loudness (RMS) a whole recording is digital silence: the
/// microphone delivers no sound at all (dead, unplugged, or muted in
/// hardware). Even a quiet room reads well above it in raw mode.
pub const DEAD_MIC_RMS: f32 = 1e-5;

/// Captures mono audio from `device` (None: the system default) until
/// `keep_going` returns false.
fn capture(
    device: Option<&str>,
    ready: &mut dyn FnMut(u32, &str),
    sink: &mut dyn FnMut(&[f32]),
    keep_going: &mut dyn FnMut() -> bool,
) -> Result<()> {
    #[cfg(windows)]
    {
        crate::wasapi::capture(device, ready, sink, keep_going)
    }
    #[cfg(not(windows))]
    {
        cpal_capture::capture(device, ready, sink, keep_going)
    }
}

/// Name of the system's default microphone.
pub fn default_device_name() -> Option<String> {
    #[cfg(windows)]
    {
        crate::wasapi::default_device_name()
    }
    #[cfg(not(windows))]
    {
        cpal_capture::default_device_name()
    }
}

pub fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt()
}

// ---------------------------------------------------------------------------
// Always-ready microphone

pub struct Mic {
    state: Mutex<MicState>,
    level: Arc<AtomicU32>,
    stop_tx: Mutex<Option<mpsc::Sender<()>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Default)]
struct MicState {
    rate: u32,
    device: String,
    /// The last `PRE_ROLL_SECS` of audio while nobody is recording.
    pre_roll: VecDeque<f32>,
    /// Where samples go while a recording is running.
    target: Option<Arc<Mutex<Vec<f32>>>>,
}

impl Mic {
    /// Opens `device` (None: the system default, followed when it changes)
    /// and keeps it ready. Fails if it can't be opened right now.
    pub fn start(device: Option<String>) -> Result<Arc<Self>> {
        let mic = Arc::new(Self {
            state: Mutex::new(MicState::default()),
            level: Arc::new(AtomicU32::new(0)),
            stop_tx: Mutex::new(None),
            thread: Mutex::new(None),
        });
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let weak = Arc::downgrade(&mic);
        let handle = thread::spawn(move || keep_open(weak, device, stop_rx, ready_tx));
        *mic.stop_tx.lock().unwrap() = Some(stop_tx);
        *mic.thread.lock().unwrap() = Some(handle);
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(mic),
            Ok(Err(e)) => {
                mic.stop();
                Err(anyhow!(e))
            }
            Err(_) => Err(anyhow!("Microphone thread stopped unexpectedly")),
        }
    }

    /// Starts a recording instantly, beginning with the pre-roll.
    pub fn record(self: &Arc<Self>) -> Recording {
        let mut state = self.state.lock().unwrap();
        let mut samples: Vec<f32> = state.pre_roll.drain(..).collect();
        samples.reserve(state.rate as usize * 30);
        let buffer = Arc::new(Mutex::new(samples));
        state.target = Some(buffer.clone());
        Recording {
            level: self.level.clone(),
            buffer,
            rate: state.rate,
            device: state.device.clone(),
            source: Source::Shared(self.clone()),
        }
    }

    /// Closes the microphone.
    pub fn stop(&self) {
        drop(self.stop_tx.lock().unwrap().take());
        if let Some(handle) = self.thread.lock().unwrap().take() {
            let _ = handle.join();
        }
    }

    fn push(&self, samples: &[f32]) {
        self.level.store(rms(samples).to_bits(), Ordering::Relaxed);
        let mut state = self.state.lock().unwrap();
        if let Some(target) = &state.target {
            target.lock().unwrap().extend_from_slice(samples);
            return;
        }
        let keep = (state.rate as f32 * PRE_ROLL_SECS) as usize;
        state.pre_roll.extend(samples.iter().copied());
        let excess = state.pre_roll.len().saturating_sub(keep);
        state.pre_roll.drain(..excess);
    }

    fn detach(&self, buffer: &Arc<Mutex<Vec<f32>>>) {
        let mut state = self.state.lock().unwrap();
        if state.target.as_ref().is_some_and(|t| Arc::ptr_eq(t, buffer)) {
            state.target = None;
        }
    }

    fn recording(&self) -> bool {
        self.state.lock().unwrap().target.is_some()
    }

    fn reset(&self, rate: u32, device: &str) {
        let mut state = self.state.lock().unwrap();
        state.rate = rate;
        state.device = device.to_string();
        state.pre_roll.clear();
    }
}

/// The always-ready mic's thread: keeps capturing, reopening the device
/// after errors, and following the system default when it changes
/// (between recordings) unless a specific device was chosen.
fn keep_open(mic: Weak<Mic>, device: Option<String>, stop_rx: mpsc::Receiver<()>, ready_tx: mpsc::Sender<Result<(), String>>) {
    let mut ready_tx = Some(ready_tx);
    loop {
        let mut stopped = false;
        let opened = std::cell::RefCell::new(String::new());
        let mut last_check = Instant::now();
        let result = capture(
            device.as_deref(),
            &mut |rate, name| {
                *opened.borrow_mut() = name.to_string();
                if let Some(m) = mic.upgrade() {
                    m.reset(rate, name);
                }
                if let Some(tx) = ready_tx.take() {
                    let _ = tx.send(Ok(()));
                }
                log::info!("Microphone ready: {name} ({rate} Hz)");
            },
            &mut |samples| {
                if let Some(m) = mic.upgrade() {
                    m.push(samples);
                }
            },
            &mut || {
                if !matches!(stop_rx.try_recv(), Err(TryRecvError::Empty)) {
                    stopped = true;
                    return false;
                }
                if device.is_none() && last_check.elapsed() >= DEVICE_CHECK {
                    last_check = Instant::now();
                    let busy = mic.upgrade().is_some_and(|m| m.recording());
                    if !busy && default_device_name().is_some_and(|d| d != *opened.borrow()) {
                        log::info!("Default microphone changed; switching");
                        return false;
                    }
                }
                true
            },
        );
        if stopped {
            return;
        }
        if let Err(e) = result {
            if let Some(tx) = ready_tx.take() {
                let _ = tx.send(Err(e.to_string()));
                return;
            }
            log::warn!("Microphone stopped working ({e}); retrying");
            if !matches!(stop_rx.recv_timeout(RETRY_DELAY), Err(mpsc::RecvTimeoutError::Timeout)) {
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// One recording

/// One dictation's audio. Readable while recording and after `stop()`.
pub struct Recording {
    level: Arc<AtomicU32>,
    /// Mono samples at the device's own rate.
    buffer: Arc<Mutex<Vec<f32>>>,
    rate: u32,
    /// Which microphone it came from.
    device: String,
    source: Source,
}

enum Source {
    /// Opened just for this recording.
    Own { stop_tx: mpsc::Sender<()>, thread: Mutex<Option<JoinHandle<()>>> },
    /// Taken from the always-ready mic.
    Shared(Arc<Mic>),
}

impl Recording {
    /// Opens `device` (None: the system default) just for this recording.
    pub fn start(device: Option<String>) -> Result<Self> {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(u32, String), String>>();
        let level = Arc::new(AtomicU32::new(0));
        let buffer = Arc::new(Mutex::new(Vec::<f32>::new()));
        let (level_cb, buffer_cb) = (level.clone(), buffer.clone());

        let thread = thread::spawn(move || {
            let mut ready_tx = Some(ready_tx);
            let result = capture(
                device.as_deref(),
                &mut |rate, name| {
                    if let Some(tx) = ready_tx.take() {
                        let _ = tx.send(Ok((rate, name.to_string())));
                    }
                },
                &mut |samples| {
                    level_cb.store(rms(samples).to_bits(), Ordering::Relaxed);
                    buffer_cb.lock().unwrap().extend_from_slice(samples);
                },
                // Runs until stop() is called (or the Recording is dropped).
                &mut || matches!(stop_rx.try_recv(), Err(TryRecvError::Empty)),
            );
            if let (Err(e), Some(tx)) = (result, ready_tx.take()) {
                let _ = tx.send(Err(e.to_string()));
            }
        });

        match ready_rx.recv() {
            Ok(Ok((rate, device))) => Ok(Self {
                level,
                buffer,
                rate,
                device,
                source: Source::Own { stop_tx, thread: Mutex::new(Some(thread)) },
            }),
            Ok(Err(e)) => Err(anyhow!(e)),
            Err(_) => Err(anyhow!("Microphone thread stopped unexpectedly")),
        }
    }

    /// Live input loudness (RMS of the latest buffer, 0.0–1.0) for meters.
    pub fn level(&self) -> Arc<AtomicU32> {
        self.level.clone()
    }

    /// Which microphone this came from.
    pub fn device(&self) -> &str {
        &self.device
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

    /// How many device-rate samples `samples_16k` samples at 16 kHz span.
    pub fn device_samples(&self, samples_16k: usize) -> usize {
        (samples_16k as u64 * self.rate as u64 / TARGET_RATE as u64) as usize
    }

    /// Stops capturing. The captured audio stays readable.
    pub fn stop(&self) -> Result<()> {
        match &self.source {
            Source::Own { stop_tx, thread } => {
                let _ = stop_tx.send(());
                if let Some(handle) = thread.lock().unwrap().take() {
                    handle.join().map_err(|_| anyhow!("Microphone thread panicked"))?;
                }
            }
            Source::Shared(mic) => mic.detach(&self.buffer),
        }
        Ok(())
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

// ---------------------------------------------------------------------------
// cpal capture (macOS / Linux)

#[cfg(not(windows))]
mod cpal_capture {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use anyhow::{anyhow, bail, Result};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, SampleFormat, SizedSample};

    pub fn default_device_name() -> Option<String> {
        cpal::default_host().default_input_device().map(|d| d.to_string())
    }

    pub fn capture(
        device: Option<&str>,
        ready: &mut dyn FnMut(u32, &str),
        sink: &mut dyn FnMut(&[f32]),
        keep_going: &mut dyn FnMut() -> bool,
    ) -> Result<()> {
        let host = cpal::default_host();
        let device = match device {
            None => host.default_input_device(),
            Some(want) => host.input_devices()?.find(|d| d.to_string() == want),
        }
        .ok_or_else(|| anyhow!("No microphone found"))?;
        let supported = device.default_input_config()?;
        let rate = supported.sample_rate();
        let channels = supported.channels() as usize;
        let queue = Arc::new(Mutex::new(Vec::<f32>::new()));
        let failed = Arc::new(Mutex::new(None::<String>));
        let stream = match supported.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, supported.config(), channels, queue.clone(), failed.clone())?,
            SampleFormat::I16 => build::<i16>(&device, supported.config(), channels, queue.clone(), failed.clone())?,
            SampleFormat::U16 => build::<u16>(&device, supported.config(), channels, queue.clone(), failed.clone())?,
            SampleFormat::I32 => build::<i32>(&device, supported.config(), channels, queue.clone(), failed.clone())?,
            other => bail!("Unsupported microphone sample format: {other:?}"),
        };
        stream.play()?;
        ready(rate, &device.to_string());
        while keep_going() {
            std::thread::sleep(Duration::from_millis(10));
            if let Some(e) = failed.lock().unwrap().take() {
                bail!(e);
            }
            let chunk = std::mem::take(&mut *queue.lock().unwrap());
            if !chunk.is_empty() {
                sink(&chunk);
            }
        }
        Ok(())
    }

    fn build<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        channels: usize,
        queue: Arc<Mutex<Vec<f32>>>,
        failed: Arc<Mutex<Option<String>>>,
    ) -> Result<cpal::Stream>
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        Ok(device.build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                queue.lock().unwrap().extend(
                    data.chunks(channels)
                        .map(|frame| frame.iter().map(|s| f32::from_sample_(*s)).sum::<f32>() / channels as f32),
                );
            },
            move |err| *failed.lock().unwrap() = Some(err.to_string()),
            None,
        )?)
    }
}

// ---------------------------------------------------------------------------

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

    fn test_mic(rate: u32) -> Arc<Mic> {
        let mic = Arc::new(Mic {
            state: Mutex::new(MicState::default()),
            level: Arc::new(AtomicU32::new(0)),
            stop_tx: Mutex::new(None),
            thread: Mutex::new(None),
        });
        mic.reset(rate, "test");
        mic
    }

    #[test]
    fn pre_roll_keeps_only_the_last_half_second() {
        let mic = test_mic(1000);
        mic.push(&vec![0.1; 800]);
        mic.push(&vec![0.2; 300]);
        let rec = mic.record();
        assert_eq!(rec.len(), 500);
        // The newest audio is what survives.
        assert_eq!(rec.buffer.lock().unwrap()[499], 0.2);
        assert_eq!(rec.device(), "test");
    }

    #[test]
    fn recording_gets_new_audio_until_stopped() {
        let mic = test_mic(1000);
        mic.push(&[0.1; 100]);
        let rec = mic.record();
        mic.push(&[0.3; 50]);
        rec.stop().unwrap();
        mic.push(&[0.9; 50]); // after stop: back to the pre-roll
        assert_eq!(rec.len(), 150);
        assert!(!mic.recording());
    }
}

#[cfg(test)]
mod latency {
    /// Opens the real default microphone and prints how long recording
    /// takes to start, on demand vs always-ready:
    /// `cargo test --lib mic_start -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn mic_start_latency() {
        use super::{Mic, Recording};
        use std::time::{Duration, Instant};
        for attempt in 1..=3 {
            let pressed = Instant::now();
            let rec = Recording::start(None).expect("microphone");
            let opened = pressed.elapsed();
            while rec.len() == 0 && pressed.elapsed() < Duration::from_secs(3) {
                std::thread::sleep(Duration::from_millis(1));
            }
            println!("on demand, attempt {attempt}: open {} ms, first audio {} ms", opened.as_millis(), pressed.elapsed().as_millis());
            rec.stop().unwrap();
        }

        let mic = Mic::start(None).expect("microphone");
        std::thread::sleep(Duration::from_millis(800)); // let the pre-roll fill
        let pressed = Instant::now();
        let rec = mic.record();
        println!(
            "always-ready ({}): started in {} us, holding {} ms from before the press",
            rec.device(),
            pressed.elapsed().as_micros(),
            rec.len() as u64 * 1000 / rec.rate as u64
        );
        rec.stop().unwrap();
        mic.stop();
    }
}
