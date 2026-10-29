// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Sruthi contributors

//! Your own benchmark: every speech model, scored on the dictations you
//! corrected. The audio never left this PC, so any model can be run on it
//! again and compared with what you said you meant.

use serde::{Deserialize, Serialize};

/// Words of `text` as lowercase letters and digits, the way two spellings of
/// the same sentence are compared ("4 PM." and "4 pm" are the same words).
pub fn normalize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '\'')
        .map(|w| w.trim_matches('\'').to_lowercase())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Word errors of `heard` against `reference` (insertions, deletions and
/// substitutions) and the number of reference words.
pub fn word_errors(reference: &str, heard: &str) -> (usize, usize) {
    let (a, b) = (normalize(reference), normalize(heard));
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, wa) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, wb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = if wa == wb {
                diagonal
            } else {
                1 + diagonal.min(above).min(row[j])
            };
            diagonal = above;
        }
    }
    (row[b.len()], a.len())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Score {
    pub model_id: String,
    pub name: String,
    /// Corrected dictations this model was run on.
    pub clips: usize,
    pub words: usize,
    pub wrong: usize,
    /// Time spent transcribing all clips, and the audio they hold.
    pub total_ms: u64,
    pub audio_s: f32,
    /// When the run finished, in milliseconds since 1970.
    pub at_ms: u64,
}

fn path() -> std::path::PathBuf {
    crate::settings::app_root().join("benchmark.json")
}

pub fn load() -> Vec<Score> {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Keeps the newest score per model.
pub fn save(scores: Vec<Score>) -> anyhow::Result<()> {
    std::fs::write(path(), serde_json::to_string_pretty(&scores)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_sentences_have_no_errors_whatever_the_punctuation() {
        assert_eq!(
            word_errors("Meet at 4 PM, Friday.", "meet at 4 pm friday"),
            (0, 5)
        );
    }

    #[test]
    fn counts_substitutions_deletions_and_insertions() {
        // one substituted, one dropped, one extra
        assert_eq!(
            word_errors("send the report today", "send a report"),
            (2, 4)
        );
        assert_eq!(word_errors("hello there", "hello there again"), (1, 2));
        assert_eq!(word_errors("hello there", ""), (2, 2));
    }

    #[test]
    fn an_empty_reference_has_no_words() {
        assert_eq!(word_errors("", "anything at all"), (3, 0));
    }

    /// The same path a benchmark run takes, on your own newest recordings,
    /// with what was pasted standing in for the answer key.
    /// `cargo test --release --lib scores_real_recordings -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn scores_real_recordings() {
        let model = crate::model::find("parakeet-v3").unwrap();
        let mut loaded = model.load().unwrap();
        loaded.set_latin_only(true);
        let engine: crate::dictation::Engine =
            std::sync::Arc::new(std::sync::Mutex::new(Some(loaded)));
        let (mut wrong, mut words) = (0, 0);
        for entry in crate::recordings::load()
            .into_iter()
            .filter(|e| !e.text.is_empty())
            .take(5)
        {
            let audio = crate::recordings::read_samples(&entry.id).unwrap();
            let started = std::time::Instant::now();
            let heard = crate::dictation::transcribe_audio(&engine, &audio).unwrap();
            let (edits, n) = word_errors(&entry.text, &heard);
            println!(
                "{} ms, {edits} of {n} words differ\n  pasted: {}\n  raw:    {heard}",
                started.elapsed().as_millis(),
                entry.text
            );
            wrong += edits;
            words += n;
        }
        println!("Total: {wrong} of {words} words differ from what was pasted");
        assert!(words > 0, "no recordings to score");
    }

    #[test]
    fn apostrophes_stay_inside_words() {
        assert_eq!(
            normalize("It's Krishna's 'idea'."),
            ["it's", "krishna's", "idea"]
        );
    }
}
