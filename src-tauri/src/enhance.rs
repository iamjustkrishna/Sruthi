//! Gets audio ready for the speech model: lifts quiet speech (whispers, a
//! far-away mic) to a normal speaking level, removes low rumble, and spots
//! recordings that hold no speech at all. Models hallucinate on silence and
//! on very quiet audio, so both matter.

use crate::audio::TARGET_RATE;

/// 20 ms frames.
const FRAME: usize = TARGET_RATE as usize / 50;
/// Normal speaking level (RMS, about -22 dBFS).
const TARGET_RMS: f32 = 0.08;
/// At most +38 dB: real whispers on this laptop peaked around 0.001-0.005.
const MAX_GAIN: f32 = 80.0;
/// A frame this many times louder than the noise floor counts as speech.
const SPEECH_OVER_NOISE: f32 = 2.0;
/// Frames quieter than this never count as speech (about -74 dBFS). Kept
/// tiny on purpose: an earlier 0.0015 threw away real whispers that were
/// thousands of times louder than the (gated, near-zero) background.
const MIN_SPEECH_RMS: f32 = 0.0002;
/// Less speech than this (in frames, 120 ms) means nothing was said.
const MIN_SPEECH_FRAMES: usize = 6;
/// Rumble and DC below this frequency are removed.
const HIGH_PASS_HZ: f32 = 80.0;

/// Returns the cleaned-up audio, or `None` when the recording is silence.
pub fn prepare(samples: &[f32]) -> Option<Vec<f32>> {
    let mut audio = high_pass(samples, HIGH_PASS_HZ);
    let frames: Vec<f32> = audio.chunks(FRAME).map(rms).collect();
    if frames.is_empty() {
        return None;
    }

    let mut sorted = frames.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let noise_floor = sorted[sorted.len() / 10].max(1e-5);
    let threshold = (noise_floor * SPEECH_OVER_NOISE).max(MIN_SPEECH_RMS);
    let speech: Vec<f32> = frames.iter().copied().filter(|&r| r > threshold).collect();
    if speech.len() < MIN_SPEECH_FRAMES {
        return None;
    }

    // Level of the speech itself (power mean), ignoring the pauses.
    let level = (speech.iter().map(|r| r * r).sum::<f32>() / speech.len() as f32).sqrt();
    let gain = (TARGET_RMS / level).clamp(1.0, MAX_GAIN);
    if gain > 1.01 {
        log::info!("Quiet speech: boosting by {:.0} dB", 20.0 * gain.log10());
        for s in &mut audio {
            *s = soft_clip(*s * gain);
        }
    }
    Some(audio)
}

fn rms(frame: &[f32]) -> f32 {
    (frame.iter().map(|s| s * s).sum::<f32>() / frame.len().max(1) as f32).sqrt()
}

/// One-pole high-pass filter.
fn high_pass(input: &[f32], cutoff_hz: f32) -> Vec<f32> {
    let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff_hz);
    let dt = 1.0 / TARGET_RATE as f32;
    let a = rc / (rc + dt);
    let mut out = Vec::with_capacity(input.len());
    let (mut prev_in, mut prev_out) = (0.0f32, 0.0f32);
    for &x in input {
        let y = a * (prev_out + x - prev_in);
        out.push(y);
        prev_in = x;
        prev_out = y;
    }
    out
}

/// Keeps boosted peaks from clipping harshly: linear up to 0.9, then a
/// smooth curve that never passes 1.0.
fn soft_clip(x: f32) -> f32 {
    const KNEE: f32 = 0.9;
    let m = x.abs();
    if m <= KNEE {
        x
    } else {
        x.signum() * (KNEE + (1.0 - KNEE) * ((m - KNEE) / (1.0 - KNEE)).tanh())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 220 Hz tone at `level` RMS for `secs`, with quiet noise around it.
    fn speechlike(level: f32, secs: f32) -> Vec<f32> {
        let n = (secs * TARGET_RATE as f32) as usize;
        let mut seed = 12345u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            ((seed >> 16) as f32 / 32_768.0 - 1.0) * 0.0005
        };
        let pad = TARGET_RATE as usize / 2;
        let amp = level * std::f32::consts::SQRT_2;
        let mut out: Vec<f32> = (0..pad).map(|_| noise()).collect();
        out.extend((0..n).map(|i| amp * (2.0 * std::f32::consts::PI * 220.0 * i as f32 / TARGET_RATE as f32).sin() + noise()));
        out.extend((0..pad).map(|_| noise()));
        out
    }

    fn speech_level(audio: &[f32]) -> f32 {
        let mut frames: Vec<f32> = audio.chunks(FRAME).map(rms).collect();
        frames.sort_by(|a, b| b.partial_cmp(a).unwrap());
        frames[frames.len() / 4]
    }

    #[test]
    fn whisper_is_lifted_to_speaking_level() {
        let out = prepare(&speechlike(0.004, 1.0)).expect("speech detected");
        let level = speech_level(&out);
        assert!((0.05..=0.11).contains(&level), "level {level}");
    }

    /// Real whispers measured 0.001-0.005 RMS over a background of ~0
    /// (the mic gates silence); they must count as speech.
    #[test]
    fn whisper_over_silent_background_is_kept() {
        let mut audio = vec![0.0f32; TARGET_RATE as usize];
        let whisper: Vec<f32> = (0..TARGET_RATE as usize)
            .map(|i| 0.0015 * std::f32::consts::SQRT_2 * (2.0 * std::f32::consts::PI * 300.0 * i as f32 / TARGET_RATE as f32).sin())
            .collect();
        audio.extend(whisper);
        audio.extend(vec![0.0f32; TARGET_RATE as usize]);
        assert!(prepare(&audio).is_some());
    }

    #[test]
    fn normal_speech_is_left_alone() {
        let input = speechlike(0.1, 1.0);
        let out = prepare(&input).unwrap();
        assert!((speech_level(&out) - speech_level(&input)).abs() < 0.01);
    }

    #[test]
    fn silence_is_rejected() {
        assert!(prepare(&speechlike(0.0, 1.0)).is_none());
        assert!(prepare(&[]).is_none());
    }

    #[test]
    fn soft_clip_never_exceeds_one() {
        assert!(soft_clip(5.0) <= 1.0 && soft_clip(-5.0) >= -1.0);
        assert_eq!(soft_clip(0.5), 0.5);
    }
}
