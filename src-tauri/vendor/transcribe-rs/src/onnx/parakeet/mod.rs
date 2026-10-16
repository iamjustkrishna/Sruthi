use ndarray::{Array, Array1, Array2, Array3, ArrayD, ArrayViewD, IxDyn};
use once_cell::sync::Lazy;
use ort::inputs;
use ort::session::Session;
use ort::value::TensorRef;
use regex::Regex;
use std::path::Path;

mod hotwords;

use super::session;
use super::Quantization;
use crate::decode::tokens::load_vocab;
use crate::{
    ModelCapabilities, SpeechModel, TranscribeError, TranscribeOptions, TranscriptionResult,
    TranscriptionSegment,
};

/// Timestamp granularity for Parakeet output.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum TimestampGranularity {
    #[default]
    Token,
    Word,
    Segment,
}

/// Per-model inference parameters for Parakeet.
#[derive(Debug, Clone, Default)]
pub struct ParakeetParams {
    /// Language hint (currently unused, Parakeet is English-only).
    pub language: Option<String>,
    /// Timestamp granularity for output segments.
    pub timestamp_granularity: Option<TimestampGranularity>,
}

const CAPABILITIES: ModelCapabilities = ModelCapabilities {
    name: "Parakeet",
    engine_id: "parakeet",
    sample_rate: 16000,
    languages: &["en"],
    supports_timestamps: true,
    supports_translation: false,
    supports_streaming: false,
};

type DecoderState = (Array3<f32>, Array3<f32>);

const SUBSAMPLING_FACTOR: usize = 8;
const WINDOW_SIZE: f32 = 0.01;
const MAX_TOKENS_PER_STEP: usize = 10;

/// SRUTAM PATCH: a letter from a script other than Latin (ASCII, Latin-1
/// and Latin Extended are allowed).
fn is_non_latin_letter(c: char) -> bool {
    c.is_alphabetic() && !c.is_ascii() && !('\u{00C0}'..='\u{024F}').contains(&c)
}

/// SRUTAM PATCH: duration logits closer than this to the best count as a tie.
const DURATION_TIE_MARGIN: f32 = 1.0;

/// SRUTAM PATCH: the smallest forward TDT duration whose logit is within
/// `DURATION_TIE_MARGIN` of the best one (0 when there are no durations).
/// Duration 0 ("stay on this frame") is only used when it clearly wins:
/// picking it on a near-tie lets the decoder loop on quiet audio.
fn smallest_close_duration(logits: &[f32]) -> usize {
    let Some(best_idx) = logits
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
    else {
        return 0;
    };
    if best_idx == 0 {
        return 0;
    }
    let best = logits[best_idx];
    (1..logits.len()).find(|&i| logits[i] >= best - DURATION_TIE_MARGIN).unwrap_or(best_idx)
}

static DECODE_SPACE_RE: Lazy<Result<Regex, regex::Error>> =
    Lazy::new(|| Regex::new(r"\A\s|\s\B|(\s)\b"));

// Timestamp types for hierarchical segmentation

#[derive(Debug, Clone, PartialEq)]
struct Token {
    text: String,
    t_start: f32,
    t_end: f32,
    is_blank: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct Word {
    text: String,
    t_start: f32,
    t_end: f32,
    tokens: Vec<Token>,
}

#[derive(Debug, Clone, PartialEq)]
struct Segment {
    text: String,
    t_start: f32,
    t_end: f32,
}

struct TimestampedResult {
    text: String,
    timestamps: Vec<f32>,
    tokens: Vec<String>,
}

pub struct ParakeetModel {
    encoder: Session,
    decoder_joint: Session,
    preprocessor: Session,
    vocab: Vec<String>,
    blank_idx: i32,
    vocab_size: usize,
    /// SRUTAM PATCH: the user's dictionary, boosted while decoding.
    hotwords: Option<hotwords::Hotwords>,
    /// SRUTAM PATCH: per token, whether it contains letters outside the
    /// Latin alphabet (empty = no restriction).
    non_latin: Vec<bool>,
}

impl ParakeetModel {
    pub fn load(model_dir: &Path, quantization: &Quantization) -> Result<Self, TranscribeError> {
        let encoder_path = session::resolve_model_path(model_dir, "encoder-model", quantization);
        let decoder_path =
            session::resolve_model_path(model_dir, "decoder_joint-model", quantization);
        let preprocessor_path = model_dir.join("nemo128.onnx");

        let encoder = session::create_session(&encoder_path)?;
        let decoder_joint = session::create_session(&decoder_path)?;
        let preprocessor = session::create_session(&preprocessor_path)?;

        let vocab_path = model_dir.join("vocab.txt");
        let (vocab, blank_idx) = load_vocab(&vocab_path)?;
        let blank_idx = blank_idx.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Missing <blk> token in vocabulary",
            )
        })?;
        let vocab_size = vocab.len();

        log::info!(
            "Loaded vocabulary with {} tokens, blank_idx={}",
            vocab_size,
            blank_idx
        );

        Ok(Self {
            encoder,
            decoder_joint,
            preprocessor,
            vocab,
            blank_idx,
            vocab_size,
            hotwords: None,
            non_latin: Vec::new(),
        })
    }

    /// Default leading silence in milliseconds.
    const DEFAULT_LEADING_SILENCE_MS: u32 = 250;

    /// Transcribe with model-specific parameters.
    ///
    /// Applies leading silence padding (default 250 ms) and adjusts
    /// timestamps, matching the behaviour of [`SpeechModel::transcribe`].
    pub fn transcribe_with(
        &mut self,
        samples: &[f32],
        params: &ParakeetParams,
    ) -> Result<TranscriptionResult, TranscribeError> {
        let granularity = params.timestamp_granularity.clone().unwrap_or_default();
        let lead_ms = Self::DEFAULT_LEADING_SILENCE_MS;
        let padded = crate::audio::prepend_silence(samples, lead_ms);
        let mut result = self.infer(&padded, &granularity)?;
        result.offset_timestamps(-(lead_ms as f32 / 1000.0));
        Ok(result)
    }

    fn infer(
        &mut self,
        samples: &[f32],
        granularity: &TimestampGranularity,
    ) -> Result<TranscriptionResult, TranscribeError> {
        let timestamped_result = self.transcribe_samples_internal(samples.to_vec())?;
        let segments = convert_timestamps(&timestamped_result, granularity);

        Ok(TranscriptionResult {
            text: timestamped_result.text,
            segments: Some(segments),
        })
    }

    fn preprocess(
        &mut self,
        waveforms: &ArrayViewD<f32>,
        waveforms_lens: &ArrayViewD<i64>,
    ) -> Result<(ArrayD<f32>, ArrayD<i64>), TranscribeError> {
        let t_waveforms = TensorRef::from_array_view(waveforms.view())?;
        let t_waveforms_lens = TensorRef::from_array_view(waveforms_lens.view())?;
        let inputs = inputs![
            "waveforms" => t_waveforms,
            "waveforms_lens" => t_waveforms_lens,
        ];
        let outputs = self.preprocessor.run(inputs)?;

        let features = outputs
            .get("features")
            .ok_or_else(|| TranscribeError::Inference("Missing output: features".to_string()))?
            .try_extract_array()?;
        let features_lens = outputs
            .get("features_lens")
            .ok_or_else(|| TranscribeError::Inference("Missing output: features_lens".to_string()))?
            .try_extract_array()?;

        Ok((features.to_owned(), features_lens.to_owned()))
    }

    fn encode(
        &mut self,
        audio_signal: &ArrayViewD<f32>,
        length: &ArrayViewD<i64>,
    ) -> Result<(ArrayD<f32>, ArrayD<i64>), TranscribeError> {
        let t_audio_signal = TensorRef::from_array_view(audio_signal.view())?;
        let t_length = TensorRef::from_array_view(length.view())?;
        let inputs = inputs![
            "audio_signal" => t_audio_signal,
            "length" => t_length,
        ];
        let outputs = self.encoder.run(inputs)?;

        let encoder_output = outputs
            .get("outputs")
            .ok_or_else(|| TranscribeError::Inference("Missing output: outputs".to_string()))?
            .try_extract_array()?;
        let encoded_lengths = outputs
            .get("encoded_lengths")
            .ok_or_else(|| {
                TranscribeError::Inference("Missing output: encoded_lengths".to_string())
            })?
            .try_extract_array()?;

        let encoder_output = encoder_output.permuted_axes(IxDyn(&[0, 2, 1]));

        Ok((encoder_output.to_owned(), encoded_lengths.to_owned()))
    }

    fn create_decoder_state(&self) -> Result<DecoderState, TranscribeError> {
        let inputs = self.decoder_joint.inputs();

        let state1_shape = inputs
            .iter()
            .find(|input| input.name() == "input_states_1")
            .ok_or_else(|| TranscribeError::Inference("Missing input: input_states_1".to_string()))?
            .dtype()
            .tensor_shape()
            .ok_or_else(|| {
                TranscribeError::Inference(
                    "Failed to get tensor shape for input_states_1".to_string(),
                )
            })?;

        let state2_shape = inputs
            .iter()
            .find(|input| input.name() == "input_states_2")
            .ok_or_else(|| TranscribeError::Inference("Missing input: input_states_2".to_string()))?
            .dtype()
            .tensor_shape()
            .ok_or_else(|| {
                TranscribeError::Inference(
                    "Failed to get tensor shape for input_states_2".to_string(),
                )
            })?;

        let state1 = Array::zeros((state1_shape[0] as usize, 1, state1_shape[2] as usize));

        let state2 = Array::zeros((state2_shape[0] as usize, 1, state2_shape[2] as usize));

        Ok((state1, state2))
    }

    fn decode_step(
        &mut self,
        prev_tokens: &[i32],
        prev_state: &DecoderState,
        encoder_out: &ArrayViewD<f32>,
    ) -> Result<(ArrayD<f32>, DecoderState), TranscribeError> {
        let target_token = prev_tokens.last().copied().unwrap_or(self.blank_idx);

        let encoder_outputs = encoder_out
            .to_owned()
            .insert_axis(ndarray::Axis(0))
            .insert_axis(ndarray::Axis(2));
        let targets = Array2::from_shape_vec((1, 1), vec![target_token])?;
        let target_length = Array1::from_vec(vec![1]);

        let t_encoder_outputs = TensorRef::from_array_view(encoder_outputs.view())?;
        let t_targets = TensorRef::from_array_view(targets.view())?;
        let t_target_length = TensorRef::from_array_view(target_length.view())?;
        let t_input_states_1 = TensorRef::from_array_view(prev_state.0.view())?;
        let t_input_states_2 = TensorRef::from_array_view(prev_state.1.view())?;
        let inputs = inputs![
            "encoder_outputs" => t_encoder_outputs,
            "targets" => t_targets,
            "target_length" => t_target_length,
            "input_states_1" => t_input_states_1,
            "input_states_2" => t_input_states_2,
        ];

        let outputs = self.decoder_joint.run(inputs)?;

        let logits = outputs
            .get("outputs")
            .ok_or_else(|| TranscribeError::Inference("Missing output: outputs".to_string()))?
            .try_extract_array()?;
        let state1 = outputs
            .get("output_states_1")
            .ok_or_else(|| {
                TranscribeError::Inference("Missing output: output_states_1".to_string())
            })?
            .try_extract_array()?;
        let state2 = outputs
            .get("output_states_2")
            .ok_or_else(|| {
                TranscribeError::Inference("Missing output: output_states_2".to_string())
            })?
            .try_extract_array()?;

        let logits = logits.remove_axis(ndarray::Axis(0));

        let state1_3d = state1.to_owned().into_dimensionality::<ndarray::Ix3>()?;
        let state2_3d = state2.to_owned().into_dimensionality::<ndarray::Ix3>()?;

        Ok((logits.to_owned(), (state1_3d, state2_3d)))
    }

    fn recognize_batch(
        &mut self,
        waveforms: &ArrayViewD<f32>,
        waveforms_len: &ArrayViewD<i64>,
    ) -> Result<Vec<TimestampedResult>, TranscribeError> {
        let (features, features_lens) = self.preprocess(waveforms, waveforms_len)?;
        let (encoder_out, encoder_out_lens) =
            self.encode(&features.view(), &features_lens.view())?;

        let mut results = Vec::new();
        for (encodings, &encodings_len) in encoder_out.outer_iter().zip(encoder_out_lens.iter()) {
            let (tokens, timestamps) =
                self.decode_sequence(&encodings.view(), encodings_len as usize)?;
            let result = self.decode_tokens(tokens, timestamps);
            results.push(result);
        }

        Ok(results)
    }

    fn decode_sequence(
        &mut self,
        encodings: &ArrayViewD<f32>,
        encodings_len: usize,
    ) -> Result<(Vec<i32>, Vec<usize>), TranscribeError> {
        let mut prev_state = self.create_decoder_state()?;
        let mut tokens = Vec::new();
        let mut timestamps = Vec::new();

        let mut t = 0;
        let mut emitted_tokens = 0;
        // SRUTAM PATCH: hotwords currently in progress (prefix-tree nodes).
        let mut active_hotwords: Vec<usize> = Vec::new();

        while t < encodings_len {
            let encoder_step = encodings.slice(ndarray::s![t, ..]);
            let encoder_step_dyn = encoder_step.to_owned().into_dyn();
            let (probs, new_state) =
                self.decode_step(&tokens, &prev_state, &encoder_step_dyn.view())?;

            let vocab_logits_slice = probs
                .as_slice()
                .ok_or_else(|| TranscribeError::Inference("Logits not contiguous".to_string()))?;

            // TDT joint output = token logits followed by duration logits.
            let (vocab_logits, duration_logits) = if probs.len() > self.vocab_size {
                vocab_logits_slice.split_at(self.vocab_size)
            } else {
                (vocab_logits_slice, &[][..])
            };

            let argmax = |logits: &[f32]| {
                logits
                    .iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(idx, _)| idx)
            };
            // SRUTAM PATCH: with Latin-only on, tokens in other scripts
            // can never win.
            let mut token = if self.non_latin.is_empty() {
                argmax(vocab_logits)
            } else {
                vocab_logits
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !self.non_latin.get(*i).copied().unwrap_or(false))
                    .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(i, _)| i)
            }
            .map(|i| i as i32)
            .unwrap_or(self.blank_idx);
            // SRUTAM PATCH: hotword boosting. Pieces that start or continue
            // a dictionary word get a bonus; a new word may not override a
            // confident blank, so boosting can't invent words in silence.
            if let Some(hot) = self.hotwords.as_ref().filter(|h| !h.is_empty()) {
                let mut best = vocab_logits.get(token as usize).copied().unwrap_or(f32::NEG_INFINITY);
                for (candidate, bonus) in hot.boosts(&active_hotwords, token != self.blank_idx) {
                    if let Some(&logit) = vocab_logits.get(candidate as usize) {
                        if logit + bonus > best {
                            best = logit + bonus;
                            token = candidate;
                        }
                    }
                }
            }
            // SRUTAM PATCH: TDT models also predict how many encoder frames
            // to skip (durations 0..=4). The upstream loop ignored this and
            // stayed on the same frame until a blank, so repeated letters
            // or digits ("A A", "7 7") were emitted up to 10 times each.
            // Same rule as onnx-asr / NeMo greedy TDT decoding.
            //
            // When two skips score almost the same, take the smaller one: a
            // too-big skip can jump over a repeated character ("11th" came
            // out as "1th"), while a smaller one costs nothing because the
            // decoder state already holds the token just emitted.
            let step = smallest_close_duration(duration_logits);
            if std::env::var_os("SRUTAM_TDT_DEBUG").is_some() {
                let blank_logit = vocab_logits.get(self.blank_idx as usize).copied().unwrap_or(0.0);
                let tok_logit = vocab_logits.get(token as usize).copied().unwrap_or(0.0);
                eprintln!(
                    "t={t:>3} tok={:>8?} tok_logit={tok_logit:.1} blank={blank_logit:.1} step={step} durs={:?}",
                    self.vocab.get(token as usize),
                    duration_logits.iter().map(|v| (v * 10.0).round() / 10.0).collect::<Vec<_>>()
                );
            }

            if token != self.blank_idx {
                prev_state = new_state;
                tokens.push(token);
                timestamps.push(t);
                emitted_tokens += 1;
                if let Some(hot) = &self.hotwords {
                    active_hotwords = hot.advance(&active_hotwords, token);
                }
            }

            if step > 0 {
                t += step;
                emitted_tokens = 0;
            } else if token == self.blank_idx || emitted_tokens == MAX_TOKENS_PER_STEP {
                t += 1;
                emitted_tokens = 0;
            }
        }

        Ok((tokens, timestamps))
    }

    fn decode_tokens(&self, ids: Vec<i32>, timestamps: Vec<usize>) -> TimestampedResult {
        let tokens: Vec<String> = ids
            .iter()
            .filter_map(|&id| {
                let idx = id as usize;
                if idx < self.vocab.len() {
                    Some(self.vocab[idx].clone())
                } else {
                    None
                }
            })
            .collect();

        let text = match &*DECODE_SPACE_RE {
            Ok(regex) => regex
                .replace_all(&tokens.join(""), |caps: &regex::Captures| {
                    if caps.get(1).is_some() {
                        " "
                    } else {
                        ""
                    }
                })
                .to_string(),
            Err(_) => tokens.join(""),
        };

        let float_timestamps: Vec<f32> = timestamps
            .iter()
            .map(|&t| WINDOW_SIZE * SUBSAMPLING_FACTOR as f32 * t as f32)
            .collect();

        TimestampedResult {
            text,
            timestamps: float_timestamps,
            tokens,
        }
    }

    fn transcribe_samples_internal(
        &mut self,
        samples: Vec<f32>,
    ) -> Result<TimestampedResult, TranscribeError> {
        let batch_size = 1;
        let samples_len = samples.len();

        let waveforms = Array2::from_shape_vec((batch_size, samples_len), samples)?.into_dyn();
        let waveforms_lens = Array1::from_vec(vec![samples_len as i64]).into_dyn();

        let results = self.recognize_batch(&waveforms.view(), &waveforms_lens.view())?;

        results.into_iter().next().ok_or_else(|| {
            TranscribeError::Inference("No transcription result returned".to_string())
        })
    }
}

impl SpeechModel for ParakeetModel {
    fn capabilities(&self) -> ModelCapabilities {
        CAPABILITIES
    }

    fn set_latin_only(&mut self, on: bool) {
        self.non_latin = if on { self.vocab.iter().map(|t| t.chars().any(is_non_latin_letter)).collect() } else { Vec::new() };
    }

    fn set_hotwords(&mut self, words: &[String]) {
        self.hotwords = if words.is_empty() { None } else { Some(hotwords::Hotwords::new(words, &self.vocab)) };
    }

    fn default_leading_silence_ms(&self) -> u32 {
        Self::DEFAULT_LEADING_SILENCE_MS
    }

    fn transcribe_raw(
        &mut self,
        samples: &[f32],
        _options: &TranscribeOptions,
    ) -> Result<TranscriptionResult, TranscribeError> {
        self.infer(samples, &TimestampGranularity::default())
    }
}

// ---- Timestamp conversion ----

fn convert_timestamps(
    timestamped_result: &TimestampedResult,
    granularity: &TimestampGranularity,
) -> Vec<TranscriptionSegment> {
    match granularity {
        TimestampGranularity::Token => convert_to_raw_token_segments(timestamped_result),
        TimestampGranularity::Word => convert_to_hierarchical_word_segments(timestamped_result),
        TimestampGranularity::Segment => {
            convert_to_hierarchical_segment_segments(timestamped_result)
        }
    }
}

fn convert_to_raw_token_segments(
    timestamped_result: &TimestampedResult,
) -> Vec<TranscriptionSegment> {
    let mut segments = Vec::new();

    for (i, (token, &timestamp)) in timestamped_result
        .tokens
        .iter()
        .zip(timestamped_result.timestamps.iter())
        .enumerate()
    {
        let end_timestamp = timestamped_result
            .timestamps
            .get(i + 1)
            .copied()
            .unwrap_or(timestamp + 0.05);

        segments.push(TranscriptionSegment {
            start: timestamp,
            end: end_timestamp,
            text: token.clone(),
        });
    }

    segments
}

fn convert_to_hierarchical_word_segments(
    timestamped_result: &TimestampedResult,
) -> Vec<TranscriptionSegment> {
    if timestamped_result.tokens.is_empty() || timestamped_result.timestamps.is_empty() {
        return Vec::new();
    }

    let tokens = create_tokens_from_timestamped_result(timestamped_result);
    let words = group_tokens_into_words(&tokens);

    words
        .iter()
        .filter(|w| !w.text.trim().is_empty())
        .map(|w| TranscriptionSegment {
            start: w.t_start,
            end: w.t_end,
            text: w.text.clone(),
        })
        .collect()
}

fn convert_to_hierarchical_segment_segments(
    timestamped_result: &TimestampedResult,
) -> Vec<TranscriptionSegment> {
    if timestamped_result.tokens.is_empty() || timestamped_result.timestamps.is_empty() {
        return Vec::new();
    }

    let tokens = create_tokens_from_timestamped_result(timestamped_result);
    let words = group_tokens_into_words(&tokens);
    let segments = group_words_into_segments(&words);

    segments
        .iter()
        .filter(|s| !s.text.trim().is_empty())
        .map(|s| TranscriptionSegment {
            start: s.t_start,
            end: s.t_end,
            text: s.text.clone(),
        })
        .collect()
}

fn create_tokens_from_timestamped_result(timestamped_result: &TimestampedResult) -> Vec<Token> {
    timestamped_result
        .tokens
        .iter()
        .zip(timestamped_result.timestamps.iter())
        .enumerate()
        .map(|(i, (token_text, &timestamp))| {
            let t_end = timestamped_result
                .timestamps
                .get(i + 1)
                .copied()
                .unwrap_or(timestamp + 0.05);

            Token {
                text: token_text.clone(),
                t_start: timestamp,
                t_end,
                is_blank: token_text.trim().is_empty(),
            }
        })
        .collect()
}

fn group_tokens_into_words(tokens: &[Token]) -> Vec<Word> {
    let mut words = Vec::new();
    let mut current_word_tokens = Vec::new();

    for token in tokens {
        if token.is_blank {
            continue;
        }

        let starts_new_word = token.text.starts_with(' ')
            || token.text.starts_with("▁")
            || (current_word_tokens.is_empty() && !token.text.trim().is_empty());

        if starts_new_word && !current_word_tokens.is_empty() {
            let word = create_word_from_tokens(&current_word_tokens);
            if !word.text.is_empty() {
                words.push(word);
            }
            current_word_tokens.clear();
        }

        current_word_tokens.push(token.clone());
    }

    if !current_word_tokens.is_empty() {
        let word = create_word_from_tokens(&current_word_tokens);
        if !word.text.is_empty() {
            words.push(word);
        }
    }

    words
}

fn create_word_from_tokens(tokens: &[Token]) -> Word {
    if tokens.is_empty() {
        return Word {
            text: String::new(),
            t_start: 0.0,
            t_end: 0.0,
            tokens: Vec::new(),
        };
    }

    let t_start = tokens.first().unwrap().t_start;
    let t_end = tokens.last().unwrap().t_end;

    let text = tokens
        .iter()
        .map(|t| {
            if t.text.starts_with("▁") {
                t.text.strip_prefix("▁").unwrap_or(&t.text)
            } else if t.text.starts_with(' ') {
                t.text.strip_prefix(' ').unwrap_or(&t.text)
            } else {
                &t.text
            }
        })
        .collect::<String>()
        .trim()
        .to_string();

    Word {
        text,
        t_start,
        t_end,
        tokens: tokens.to_vec(),
    }
}

fn group_words_into_segments(words: &[Word]) -> Vec<Segment> {
    if words.is_empty() {
        return Vec::new();
    }

    let segment_separators = ['.', '?', '!'];
    let mut segments = Vec::new();
    let mut current_words: Vec<&Word> = Vec::new();

    for (i, word) in words.iter().enumerate() {
        current_words.push(word);

        let ends_segment =
            word.text.chars().any(|c| segment_separators.contains(&c)) || i == words.len() - 1;

        if ends_segment && !current_words.is_empty() {
            let text = current_words
                .iter()
                .map(|w| w.text.as_str())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");

            if !text.is_empty() {
                segments.push(Segment {
                    text,
                    t_start: current_words.first().unwrap().t_start,
                    t_end: current_words.last().unwrap().t_end,
                });
            }
            current_words.clear();
        }
    }

    if segments.is_empty() && !words.is_empty() {
        let text = words
            .iter()
            .map(|w| w.text.as_str())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if !text.is_empty() {
            segments.push(Segment {
                text,
                t_start: words.first().unwrap().t_start,
                t_end: words.last().unwrap().t_end,
            });
        }
    }

    segments
}

#[cfg(test)]
mod srutam_patch_tests {
    use super::smallest_close_duration;

    #[test]
    fn near_ties_take_the_smaller_skip() {
        // Real logits after the first "1" in "11th": 1 vs 2 frames nearly tied.
        assert_eq!(smallest_close_duration(&[2.3, 8.3, 8.5, 1.3, -2.1]), 1);
        // Clear winner stays as is.
        assert_eq!(smallest_close_duration(&[-3.8, 7.0, 8.3, -3.8, -4.3]), 2);
        assert_eq!(smallest_close_duration(&[-5.8, 3.4, 8.4, 11.6, 15.9]), 4);
        assert_eq!(smallest_close_duration(&[]), 0);
        // Latin-only decoding blocks Cyrillic/Greek, keeps accented Latin.
        assert!(super::is_non_latin_letter('Ш') && super::is_non_latin_letter('λ'));
        assert!(!super::is_non_latin_letter('é') && !super::is_non_latin_letter('a') && !super::is_non_latin_letter('5'));
        // A near-tie with "stay" moves forward instead of looping.
        assert_eq!(smallest_close_duration(&[8.2, 8.5, 3.0, 1.0, 0.0]), 1);
        // "Stay" still wins when it's the clear best.
        assert_eq!(smallest_close_duration(&[9.0, 5.0, 3.0, 1.0, 0.0]), 0);
    }
}
