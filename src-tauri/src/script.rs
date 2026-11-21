// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Sruthi contributors

//! Script output: Latin (default) or native Devanagari. The pass runs at the
//! very end of the pipeline, after corrections and snippets.
//!
//! Native→Latin is a deterministic, local romanization — no model, no
//! network. Latin→Native goes through the local qwen model (see `apply`).

use serde::{Deserialize, Serialize};

use crate::format::llm::LlmServer;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Script {
    Latin,
    Native,
}

impl Default for Script {
    fn default() -> Self {
        Script::Latin
    }
}

impl Script {
    pub fn label(self) -> &'static str {
        match self {
            Script::Latin => "latin",
            Script::Native => "native",
        }
    }
}

/// Whether a Devanagari character is a dependent vowel sign. A consonant
/// before one of these drops its inherent "a" sound.
fn is_dependent_vowel(c: char) -> bool {
    matches!(c, 'ा' | 'ि' | 'ी' | 'ु' | 'ू' | 'ृ' | 'ो' | 'ौ' | 'े' | 'ै')
}

/// The romanized base of a Devanagari consonant (without its inherent vowel).
fn consonant(c: char) -> Option<&'static str> {
    Some(match c {
        'क' => "k",
        'ख' => "kh",
        'ग' => "g",
        'घ' => "gh",
        'ङ' => "ng",
        'च' => "ch",
        'छ' => "chh",
        'ज' => "j",
        'झ' => "jh",
        'ञ' => "ny",
        'ट' => "t",
        'ठ' => "th",
        'ड' => "d",
        'ढ' => "dh",
        'ण' => "n",
        'त' => "t",
        'थ' => "th",
        'द' => "d",
        'ध' => "dh",
        'न' => "n",
        'प' => "p",
        'फ' => "ph",
        'ब' => "b",
        'भ' => "bh",
        'म' => "m",
        'य' => "y",
        'र' => "r",
        'ल' => "l",
        'व' => "v",
        'श' => "sh",
        'ष' => "sh",
        'स' => "s",
        'ह' => "h",
        _ => return None,
    })
}

/// Independent vowels at the start of a syllable.
fn independent_vowel(c: char) -> Option<&'static str> {
    Some(match c {
        'अ' | 'आ' => "a",
        'इ' | 'ई' => "i",
        'उ' => "u",
        'ऊ' | 'ऑ' => "oo",
        'ऋ' => "ri",
        'ए' => "e",
        'ऐ' => "ai",
        'ओ' => "o",
        'औ' => "au",
        _ => return None,
    })
}

fn dependent_vowel(c: char) -> Option<&'static str> {
    Some(match c {
        'ा' => "a",
        'ि' | 'ी' => "i",
        'ु' => "u",
        'ू' => "oo",
        'ृ' => "ri",
        'ो' => "o",
        'ौ' => "au",
        'े' => "e",
        'ै' => "ai",
        _ => return None,
    })
}

/// Romanizes Devanagari deterministically. Every other character (English
/// words, digits, punctuation, whitespace) passes through unchanged.
pub fn to_latin(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut chars = text.chars().peekable();
    let mut suppress_inherent = false;
    while let Some(c) = chars.next() {
        if let Some(base) = consonant(c) {
            out.push_str(base);
            let next_is_mark = chars
                .peek()
                .is_some_and(|n| is_dependent_vowel(*n) || *n == '्' || *n == 'ं' || *n == 'ः');
            if !suppress_inherent && !next_is_mark {
                out.push('a');
            }
            suppress_inherent = false;
            continue;
        }
        if let Some(v) = independent_vowel(c) {
            out.push_str(v);
            suppress_inherent = false;
            continue;
        }
        if let Some(v) = dependent_vowel(c) {
            out.push_str(v);
            suppress_inherent = false;
            continue;
        }
        match c {
            // Virama: drop it, and suppress the next consonant's inherent "a".
            '्' => suppress_inherent = true,
            'ं' => {
                out.push('n');
                suppress_inherent = false;
            }
            'ः' => {
                out.push('h');
                suppress_inherent = false;
            }
            '़' => {} // nukta: keep the consonant it modifies unchanged
            'ँ' => {
                // Candrabindu, common in Hindi (हूँ -> hoon).
                out.push('n');
                suppress_inherent = false;
            }
            '।' | '॥' => {
                out.push('.');
                suppress_inherent = false;
            }
            c if ('\u{0900}'..='\u{097F}').contains(&c) => {
                // Any Devanagari mark we have no romanization for must be
                // dropped, never leaked into "Latin" output.
                suppress_inherent = false;
            }
            _ => {
                out.push(c);
                suppress_inherent = false;
            }
        }
    }
    out
}

/// Checks a qwen conversion reply. Empty, implausibly long, or unchanged-
/// Latin replies mean the conversion failed: keep the original text.
pub fn validate_native(reply: &str, original: &str) -> String {
    let trimmed = reply.trim();
    let original_len = original.chars().count();
    if trimmed.is_empty() || trimmed.chars().count() > original_len.saturating_mul(3) {
        return original.to_owned();
    }
    let has_devanagari = original
        .chars()
        .any(|c| ('\u{0900}'..='\u{097F}').contains(&c));
    if trimmed == original && !has_devanagari {
        return original.to_owned();
    }
    trimmed.to_owned()
}

/// True when `text` still needs a Latin→native pass. Text that already has
/// Devanagari is already in the target script; re-converting it would let
/// qwen drift saved phrases a little more on every toggle.
pub fn needs_native_conversion(text: &str) -> bool {
    !text.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c))
}

/// Converts Latin text to native script via the local qwen model. Falls back
/// to the original text when the model is missing, stopped, or misbehaves.
pub fn to_native(text: &str, llm: Option<&LlmServer>) -> String {
    if !needs_native_conversion(text) {
        return text.to_owned();
    }
    let Some(llm) = llm else {
        return text.to_owned();
    };
    match llm.to_native(text) {
        Ok(reply) => validate_native(&reply, text),
        Err(e) => {
            log::warn!("Native script conversion failed: {e}");
            text.to_owned()
        }
    }
}

/// The single entry point for the end-of-pipeline script pass.
pub fn apply(text: &str, script: Script, llm: Option<&LlmServer>) -> String {
    match script {
        Script::Latin => to_latin(text),
        Script::Native => to_native(text, llm),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devanagari_to_latin_keeps_english_and_digits() {
        assert_eq!(to_latin("मैं 2 office गया"), "main 2 office gaya");
    }

    #[test]
    fn to_latin_is_idempotent_for_latin_runs() {
        assert_eq!(to_latin("main gaya"), "main gaya");
    }

    #[test]
    fn script_defaults_to_latin_and_serializes_lowercase() {
        assert_eq!(Script::default(), Script::Latin);
        assert_eq!(
            serde_json::to_string(&Script::Native).unwrap(),
            "\"native\""
        );
    }

    #[test]
    fn apply_latin_is_a_no_op() {
        assert_eq!(apply("main gaya", Script::Latin, None), "main gaya");
    }

    #[test]
    fn native_without_qwen_falls_back_to_input() {
        assert_eq!(apply("main gaya", Script::Native, None), "main gaya");
    }

    #[test]
    fn native_rejects_an_empty_model_reply() {
        assert_eq!(validate_native("", "main gaya"), "main gaya");
        assert_eq!(validate_native("   ", "main gaya"), "main gaya");
        assert_eq!(validate_native("मैं गया", "main gaya"), "मैं गया");
    }

    #[test]
    fn to_latin_leaves_no_devanagari_characters() {
        let out = to_latin("मैं office ja रहा हूँ और email भेजूँगा।");
        assert!(
            !out.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c)),
            "still Devanagari: {out}"
        );
        assert!(out.contains("hoon"), "candrabindu not converted: {out}");
        assert!(out.ends_with('.'), "danda not converted: {out}");
    }

    #[test]
    fn native_skips_text_already_in_devanagari() {
        assert!(!needs_native_conversion("मैं गया"));
        assert!(needs_native_conversion("main gaya"));
        assert!(needs_native_conversion("office 2 main"));
    }
}
