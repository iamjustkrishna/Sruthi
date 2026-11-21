// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Sruthi contributors

//! IndicConformer CTC backend. Mel features in, Devanagari text out.
//!
//! The public INT8 ONNX export takes 80-bin mel features at 16 kHz and emits
//! CTC log-probs over 5633 tokens. v1 implements the CTC head only; the
//! RNNT branch of the original model is not used.

use std::fs;
use std::path::Path;

use anyhow::{Context, anyhow, bail};
use ort::session::Session;
use ort::value::Tensor;
use transcribe_rs::ModelCapabilities;
use transcribe_rs::SpeechModel;
use transcribe_rs::TranscribeError;
use transcribe_rs::TranscribeOptions;
use transcribe_rs::TranscriptionResult;

const SAMPLE_RATE: usize = 16_000;
const FFT: usize = 512;
const HOP: usize = 160;
const MELS: usize = 80;
const BLANK_TOKEN: &str = "<blk>";
/// The public 600M multilingual export has 5633 CTC tokens. A partial
/// download must fail loudly rather than decode with a shifted blank id.
const VOCAB_SIZE: usize = 5633;

pub struct IndicModel {
    session: Session,
    tokens: Vec<String>,
    blank: usize,
    mel: Mel,
}

impl IndicModel {
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        let tokens_path = dir.join("tokens.txt");
        let text = fs::read_to_string(&tokens_path)
            .with_context(|| format!("Couldn't read {}", tokens_path.display()))?;
        let (tokens, blank) = parse_tokens(&text)?;
        let threads = crate::thermal::safe_threads().max(1) as usize;
        let model_path = dir.join("model.int8.onnx");
        let session = Session::builder()
            .map_err(|e| anyhow!("{e}"))?
            .with_intra_threads(threads)
            .map_err(|e| anyhow!("{e}"))?
            .commit_from_file(&model_path)
            .map_err(|e| anyhow!("{e}"))?;
        Ok(Self {
            session,
            tokens,
            blank,
            mel: Mel::new(),
        })
    }
}

impl SpeechModel for IndicModel {
    fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities {
            name: "Indic Conformer",
            engine_id: "indic-conformer",
            sample_rate: SAMPLE_RATE as u32,
            languages: &["hi", "as", "bn", "brx", "gu", "kn", "ks", "mr"],
            supports_timestamps: false,
            supports_translation: false,
            supports_streaming: false,
        }
    }

    fn transcribe_raw(
        &mut self,
        samples: &[f32],
        _options: &TranscribeOptions,
    ) -> Result<TranscriptionResult, TranscribeError> {
        if crate::thermal::stop_requested() {
            return Err(TranscribeError::Inference(
                "Stopped: this PC was getting too hot".into(),
            ));
        }
        let (features, frames) = mel_features(samples, &self.mel);
        if frames == 0 {
            return Ok(TranscriptionResult {
                text: String::new(),
                segments: None,
            });
        }
        let signal = Tensor::from_array((
            vec![1i64, MELS as i64, frames as i64],
            features.into_boxed_slice(),
        ))?;
        let lengths = Tensor::from_array((vec![1i64], vec![frames as i64].into_boxed_slice()))?;
        let outputs = self.session.run(ort::inputs![
            "processed_signal" => signal,
            "processed_signal_length" => lengths
        ])?;
        if crate::thermal::stop_requested() {
            return Err(TranscribeError::Inference(
                "Stopped: this PC was getting too hot".into(),
            ));
        }
        let (shape, data) = outputs["log_probs"].try_extract_tensor::<f32>()?;
        if shape.len() != 3 {
            return Err(TranscribeError::Inference(format!(
                "Unexpected log_probs shape {shape:?}"
            )));
        }
        let frames_out = shape[1].max(0) as usize;
        let vocab = shape[2].max(0) as usize;
        if vocab == 0 || data.len() < frames_out.saturating_mul(vocab) {
            return Err(TranscribeError::Inference("Short log_probs output".into()));
        }
        let mut ids = Vec::with_capacity(frames_out);
        for frame in 0..frames_out {
            let base = frame * vocab;
            let mut best = 0usize;
            let mut best_value = f32::NEG_INFINITY;
            for (id, &value) in data[base..base + vocab].iter().enumerate() {
                if value > best_value {
                    best_value = value;
                    best = id;
                }
            }
            ids.push(best);
        }
        let text = decode(&collapse(&ids, self.blank), &self.tokens);
        Ok(TranscriptionResult {
            text,
            segments: None,
        })
    }
}

/// Greedy CTC: drop blanks, then merge repeated neighbours.
fn collapse(ids: &[usize], blank: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut previous = usize::MAX;
    for &id in ids {
        if id == blank {
            previous = usize::MAX;
            continue;
        }
        if id != previous {
            out.push(id);
            previous = id;
        }
    }
    out
}

/// Reads the token file into an id-indexed table and finds the CTC blank.
/// The export is fixed at 5633 tokens; a short or garbled file is an error,
/// not a reason to paste `<unk>` into someone's document.
fn parse_tokens(text: &str) -> anyhow::Result<(Vec<String>, usize)> {
    let mut seen = vec![false; VOCAB_SIZE];
    let mut tokens = vec![String::new(); VOCAB_SIZE];
    for line in text.lines() {
        let Some((token, id)) = line.trim_end().rsplit_once(' ') else {
            continue;
        };
        let Ok(id) = id.trim().parse::<usize>() else {
            continue;
        };
        if id >= VOCAB_SIZE {
            bail!("tokens.txt has an id outside the model's vocabulary");
        }
        seen[id] = true;
        tokens[id] = token.to_owned();
    }
    if seen.iter().any(|ok| !ok) {
        bail!("tokens.txt is incomplete");
    }
    let blank = tokens
        .iter()
        .position(|t| t == BLANK_TOKEN)
        .ok_or_else(|| anyhow!("tokens.txt has no {BLANK_TOKEN} token"))?;
    Ok((tokens, blank))
}

/// Token ids to text. SentencePiece marks word starts with `▁` (U+2581);
/// turn those into spaces. Unknown tokens are dropped, never pasted. Marks
/// within a word are concatenated so Devanagari combinations stay intact.
fn decode(ids: &[usize], tokens: &[String]) -> String {
    let mut out = String::new();
    for &id in ids {
        let Some(token) = tokens.get(id) else {
            continue;
        };
        if token == "<unk>" || token == BLANK_TOKEN {
            continue;
        }
        for part in token.split('▁') {
            if part.is_empty() {
                out.push(' ');
            } else {
                out.push_str(part);
            }
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

struct Mel {
    /// For each mel band, the FFT bins and their triangular weights.
    filters: Vec<Vec<(usize, f32)>>,
}

impl Mel {
    fn new() -> Self {
        let bins = FFT / 2 + 1;
        let low = hz_to_mel(0.0);
        let high = hz_to_mel(SAMPLE_RATE as f32 / 2.0);
        let points: Vec<f32> = (0..MELS + 2)
            .map(|i| mel_to_hz(low + (high - low) * i as f32 / (MELS + 1) as f32))
            .map(|hz| hz * FFT as f32 / SAMPLE_RATE as f32)
            .collect();
        let mut filters = vec![Vec::new(); MELS];
        for band in 0..MELS {
            let (left, center, right) = (points[band], points[band + 1], points[band + 2]);
            let start = left.floor() as usize;
            let end = (right.ceil() as usize).min(bins - 1);
            for fft_bin in start..=end {
                let f = fft_bin as f32;
                let weight = if f < center && center > left {
                    (f - left) / (center - left)
                } else if f <= right && right > center {
                    (right - f) / (right - center)
                } else {
                    0.0
                };
                if weight > 0.0 {
                    filters[band].push((fft_bin, weight));
                }
            }
        }
        Self { filters }
    }
}

fn hz_to_mel(hz: f32) -> f32 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

fn mel_to_hz(mel: f32) -> f32 {
    700.0 * (10.0f32.powf(mel / 2595.0) - 1.0)
}

/// 80-bin log-mel features, laid out as [mel][time] for the ONNX model.
fn mel_features(samples: &[f32], bank: &Mel) -> (Vec<f32>, usize) {
    let frames = samples.len().saturating_sub(FFT) / HOP + 1;
    if samples.len() < FFT || frames == 0 {
        return (Vec::new(), 0);
    }
    let mut window = vec![0.0f32; FFT];
    for (i, value) in window.iter_mut().enumerate() {
        *value = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT - 1) as f32).cos();
    }
    let mut out = vec![0.0f32; MELS * frames];
    let mut real = vec![0.0f32; FFT];
    let mut imag = vec![0.0f32; FFT];
    for frame in 0..frames {
        let start = frame * HOP;
        for i in 0..FFT {
            real[i] = samples[start + i] * window[i];
            imag[i] = 0.0;
        }
        fft(&mut real, &mut imag);
        for (band, weights) in bank.filters.iter().enumerate() {
            let mut energy = 0.0f32;
            for &(bin, weight) in weights {
                energy += weight * (real[bin] * real[bin] + imag[bin] * imag[bin]);
            }
            out[band * frames + frame] = (energy + 1e-10).ln();
        }
    }
    (out, frames)
}

/// Iterative in-place radix-2 FFT.
fn fft(real: &mut [f32], imag: &mut [f32]) {
    let n = real.len();
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            real.swap(i, j);
            imag.swap(i, j);
        }
    }
    let mut length = 2;
    while length <= n {
        let angle = -2.0 * std::f32::consts::PI / length as f32;
        let (step_re, step_im) = (angle.cos(), angle.sin());
        for block in (0..n).step_by(length) {
            let mut twiddle_re = 1.0f32;
            let mut twiddle_im = 0.0f32;
            for k in 0..length / 2 {
                let a = block + k;
                let b = a + length / 2;
                let value_re = real[b] * twiddle_re - imag[b] * twiddle_im;
                let value_im = real[b] * twiddle_im + imag[b] * twiddle_re;
                real[b] = real[a] - value_re;
                imag[b] = imag[a] - value_im;
                real[a] += value_re;
                imag[a] += value_im;
                let next = twiddle_re * step_re - twiddle_im * step_im;
                twiddle_im = twiddle_re * step_im + twiddle_im * step_re;
                twiddle_re = next;
            }
        }
        length *= 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctc_collapse_merges_repeats_and_drops_blank() {
        assert_eq!(collapse(&[0, 3, 3, 0, 4, 0], 0), vec![3, 4]);
    }

    #[test]
    fn tokens_decode_devanagari() {
        let tokens = vec!["<blk>".to_string(), "म".into(), "ै".into()];
        assert_eq!(decode(&[1, 2], &tokens), "मै");
    }

    #[test]
    fn sentencepiece_marker_becomes_a_space() {
        let tokens = vec!["<blk>".into(), "▁मै".into(), "▁gaya".into()];
        assert_eq!(decode(&[1, 2], &tokens), "मै gaya");
    }

    #[test]
    fn unknown_tokens_are_never_pasted() {
        let tokens = vec!["<blk>".into(), "<unk>".into(), "म".into()];
        assert_eq!(decode(&[1, 2], &tokens), "म");
    }

    #[test]
    fn truncated_token_file_is_rejected() {
        assert!(parse_tokens("<blk> 5632\n").is_err());
    }

    #[test]
    fn token_file_without_blank_is_rejected() {
        assert!(parse_tokens("म 0\n").is_err());
    }

    #[test]
    fn mel_features_have_the_expected_layout() {
        let bank = Mel::new();
        let samples = vec![0.0f32; SAMPLE_RATE];
        let (features, frames) = mel_features(&samples, &bank);
        assert_eq!(frames, (SAMPLE_RATE - FFT) / HOP + 1);
        assert_eq!(features.len(), MELS * frames);
    }
}
