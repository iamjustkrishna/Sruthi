//! Microphone capture, two ways:
//!
//! - [`Mic`] keeps the microphone open while the app runs and remembers the
//!   last half second, so recording starts instantly and even includes a
//!   word spoken as the key went down. Windows shows its "microphone in
//!   use" icon the whole time; audio is only kept while the key is held.
//! - [`Recording::start`] opens the microphone for one recording only. On
//!   this machine that took 150–200 ms, enough to lose the first syllable.
//!
//! cpal streams are not `Send`, so every stream lives on its own thread.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

/// Sample rate every speech model in this app expects.
pub const TARGET_RATE: u32 = 16_000;

/// Audio kept from just before the key press.
const PRE_ROLL_SECS: f32 = 0.5;
/// How often the always-ready mic checks it still works and is the default.
const HEALTH_CHECK: Duration = Duration::from_secs(2);

/// Receives mono samples from a stream callback.
type Sink = Arc<dyn Fn(&[f32]) + Send + Sync>;

/// Identifies a device across calls (to notice the default mic changing).
fn device_key(device: &cpal::Device) -> String {
    device.id().map(|id| format!("{id:?}")).unwrap_or_else(|_| device.to_string())
}

fn default_device_key() -> Option<String> {
    cpal::default_host().default_input_device().map(|d| device_key(&d))
}

/// Opens the default microphone on the calling thread. Returns the playing
/// stream, its sample rate and the device key.
fn open_default(sink: Sink, level: Arc<AtomicU32>, failed: Arc<AtomicBool>) -> Result<(cpal::Stream, u32, String)> {
    let device = cpal::default_host()
        .default_input_device()
        .ok_or_else(|| anyhow!("No microphone found"))?;
    let key = device_key(&device);
    let supported = device.default_input_config()?;
    let rate = supported.sample_rate();
    let channels = supported.channels() as usize;
    let config = supported.config();
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, config, channels, sink, level, failed)?,
        SampleFormat::I16 => build::<i16>(&device, config, channels, sink, level, failed)?,
        SampleFormat::U16 => build::<u16>(&device, config, channels, sink, level, failed)?,
        SampleFormat::I32 => build::<i32>(&device, config, channels, sink, level, failed)?,
        other => bail!("Unsupported microphone sample format: {other:?}"),
    };
    stream.play()?;
    Ok((stream, rate, key))
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
    /// The last `PRE_ROLL_SECS` of audio while nobody is recording.
    pre_roll: VecDeque<f32>,
    /// Where samples go while a recording is running.
    target: Option<Arc<Mutex<Vec<f32>>>>,
}

impl Mic {
    /// Opens the default microphone and keeps it ready. Fails if no
    /// microphone can be opened right now.
    pub fn start() -> Result<Arc<Self>> {
        let mic = Arc::new(Self {
            state: Mutex::new(MicState::default()),
            level: Arc::new(AtomicU32::new(0)),
            stop_tx: Mutex::new(None),
            thread: Mutex::new(None),
        });
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let weak = Arc::downgrade(&mic);
        let handle = thread::spawn(move || keep_open(weak, stop_rx, ready_tx));
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
        Recording { level: self.level.clone(), buffer, rate: state.rate, source: Source::Shared(self.clone()) }
    }

    /// Closes the microphone.
    pub fn stop(&self) {
        drop(self.stop_tx.lock().unwrap().take());
        if let Some(handle) = self.thread.lock().unwrap().take() {
            let _ = handle.join();
        }
    }

    fn push(&self, samples: &[f32]) {
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

    fn reset(&self, rate: u32) {
        let mut state = self.state.lock().unwrap();
        state.rate = rate;
        state.pre_roll.clear();
    }
}

/// The always-ready mic's thread: keeps a stream open, reopening it when
/// it fails or when the default microphone changes (between recordings).
fn keep_open(mic: Weak<Mic>, stop_rx: mpsc::Receiver<()>, ready_tx: mpsc::Sender<Result<(), String>>) {
    let mut ready_tx = Some(ready_tx);
    loop {
        let Some(level) = mic.upgrade().map(|m| m.level.clone()) else { return };
        let sink: Sink = {
            let mic = mic.clone();
            Arc::new(move |samples: &[f32]| {
                if let Some(m) = mic.upgrade() {
                    m.push(samples);
                }
            })
        };
        let failed = Arc::new(AtomicBool::new(false));
        match open_default(sink, level, failed.clone()) {
            Ok((stream, rate, device)) => {
                if let Some(m) = mic.upgrade() {
                    m.reset(rate);
                }
                if let Some(tx) = ready_tx.take() {
                    let _ = tx.send(Ok(()));
                }
                log::info!("Microphone ready ({rate} Hz)");
                loop {
                    match stop_rx.recv_timeout(HEALTH_CHECK) {
                        Err(RecvTimeoutError::Timeout) => {}
                        _ => return, // Stopped: dropping the stream closes the mic.
                    }
                    if failed.load(Ordering::Relaxed) {
                        log::warn!("Microphone stopped working; reopening");
                        break;
                    }
                    let busy = mic.upgrade().is_some_and(|m| m.recording());
                    if !busy && default_device_key().is_some_and(|key| key != device) {
                        log::info!("Default microphone changed; switching");
                        break;
                    }
                }
                drop(stream);
            }
            Err(e) => {
                if let Some(tx) = ready_tx.take() {
                    let _ = tx.send(Err(e.to_string()));
                    return;
                }
                log::warn!("Couldn't reopen the microphone ({e}); retrying");
                if !matches!(stop_rx.recv_timeout(HEALTH_CHECK), Err(RecvTimeoutError::Timeout)) {
                    return;
                }
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
    source: Source,
}

enum Source {
    /// Opened just for this recording.
    Own { stop_tx: mpsc::Sender<()>, thread: Mutex<Option<JoinHandle<()>>> },
    /// Taken from the always-ready mic.
    Shared(Arc<Mic>),
}

impl Recording {
    /// Opens the default microphone just for this recording.
    pub fn start() -> Result<Self> {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();
        let level = Arc::new(AtomicU32::new(0));
        let buffer = Arc::new(Mutex::new(Vec::<f32>::new()));
        let (level_cb, buffer_cb) = (level.clone(), buffer.clone());

        let thread = thread::spawn(move || {
            let sink: Sink = Arc::new(move |samples: &[f32]| buffer_cb.lock().unwrap().extend_from_slice(samples));
            match open_default(sink, level_cb, Arc::new(AtomicBool::new(false))) {
                Ok((stream, rate, _)) => {
                    let _ = ready_tx.send(Ok(rate));
                    // Blocks until stop() is called (or the Recording is dropped).
                    let _ = stop_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e.to_string()));
                }
            }
        });

        match ready_rx.recv() {
            Ok(Ok(rate)) => Ok(Self {
                level,
                buffer,
                rate,
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

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    sink: Sink,
    level: Arc<AtomicU32>,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let mut mono: Vec<f32> = Vec::new();
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            // Downmix each frame to mono by averaging its channels.
            mono.clear();
            mono.extend(
                data.chunks(channels)
                    .map(|frame| frame.iter().map(|s| f32::from_sample_(*s)).sum::<f32>() / channels as f32),
            );
            if !mono.is_empty() {
                let rms = (mono.iter().map(|s| s * s).sum::<f32>() / mono.len() as f32).sqrt();
                level.store(rms.to_bits(), Ordering::Relaxed);
            }
            sink(&mono);
        },
        move |err| {
            log::error!("Microphone stream error: {err}");
            failed.store(true, Ordering::Relaxed);
        },
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

    fn test_mic(rate: u32) -> Arc<Mic> {
        let mic = Arc::new(Mic {
            state: Mutex::new(MicState::default()),
            level: Arc::new(AtomicU32::new(0)),
            stop_tx: Mutex::new(None),
            thread: Mutex::new(None),
        });
        mic.reset(rate);
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
    /// Opens the real microphone and prints how long it takes before audio
    /// arrives, on demand vs always-ready:
    /// `cargo test --lib mic_start -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn mic_start_latency() {
        use super::{Mic, Recording};
        use std::time::{Duration, Instant};
        for attempt in 1..=3 {
            let pressed = Instant::now();
            let rec = Recording::start().expect("microphone");
            let opened = pressed.elapsed();
            while rec.len() == 0 && pressed.elapsed() < Duration::from_secs(3) {
                std::thread::sleep(Duration::from_millis(1));
            }
            println!("on demand, attempt {attempt}: stream open {} ms, first audio {} ms", opened.as_millis(), pressed.elapsed().as_millis());
            rec.stop().unwrap();
        }

        let mic = Mic::start().expect("microphone");
        std::thread::sleep(Duration::from_millis(800)); // let the pre-roll fill
        let pressed = Instant::now();
        let rec = mic.record();
        println!(
            "always-ready: recording started in {} us, already holding {} ms of audio from before the press",
            pressed.elapsed().as_micros(),
            rec.len() as u64 * 1000 / rec.rate as u64
        );
        rec.stop().unwrap();
        mic.stop();
    }
}
