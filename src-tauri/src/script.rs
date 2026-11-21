// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Sruthi contributors

//! Script output: Latin (default) or native Devanagari. The pass runs at the
//! very end of the pipeline, after corrections and snippets.
//!
//! Native→Latin is a deterministic, local romanization — no model, no
//! network. Latin→Native goes through the local qwen model (see `apply`).

use serde::{Deserialize, Serialize};

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
    matches!(
        c,
        'ा' | 'ि' | 'ी' | 'ु' | 'ू' | 'ृ' | 'ो' | 'ौ' | 'े' | 'ै'
    )
}

/// The romanized base of a Devanagari consonant (without its inherent vowel).
fn consonant(c: char) -> Option<&'static str> {
    Some(match c {
        'क' => "k", 'ख' => "kh", 'ग' => "g", 'घ' => "gh", 'ङ' => "ng",
        'च' => "ch", 'छ' => "chh", 'ज' => "j", 'झ' => "jh", 'ञ' => "ny",
        'ट' => "t", 'ठ' => "th", 'ड' => "d", 'ढ' => "dh", 'ण' => "n",
        'त' => "t", 'थ' => "th", 'द' => "d", 'ध' => "dh", 'न' => "n",
        'प' => "p", 'फ' => "ph", 'ब' => "b", 'भ' => "bh", 'म' => "m",
        'य' => "y", 'र' => "r", 'ल' => "l", 'व' => "v", 'श' => "sh",
        'ष' => "sh", 'स' => "s", 'ह' => "h",
        _ => return None,
    })
}

/// Independent vowels at the start of a syllable.
fn independent_vowel(c: char) -> Option<&'static str> {
    Some(match c {
        'अ' | 'आ' => "a",
        'इ' | 'ई' => "i",
        'उ' | 'ऊ' => "u",
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
        'ु' | 'ू' => "u",
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
            _ => {
                out.push(c);
                suppress_inherent = false;
            }
        }
    }
    out
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
        assert_eq!(serde_json::to_string(&Script::Native).unwrap(), "\"native\"");
    }
}
