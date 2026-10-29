// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Sruthi contributors

//! Snippets: a short spoken phrase that types a longer block of text
//! ("email sign-off" -> "Thanks and regards,\nKrishna").
//!
//! Expansion runs last, on the formatted text, so the block arrives exactly
//! as saved (line breaks and capitals untouched) and formatting never gets a
//! chance to rewrite it. The phrase is matched word by word, ignoring case
//! and the punctuation formatting added around it.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Snippet {
    /// What you say.
    pub trigger: String,
    /// What gets typed.
    pub text: String,
    /// Drop the full stop or comma that formatting put after the phrase.
    #[serde(default = "default_exact")]
    pub exact: bool,
}

fn default_exact() -> bool {
    true
}

/// A run of letters and digits in `text`: byte range plus lowercased word.
struct Word {
    start: usize,
    end: usize,
    key: String,
}

fn words(text: &str) -> Vec<Word> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        match (c.is_alphanumeric(), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                out.push(Word {
                    start: s,
                    end: i,
                    key: text[s..i].to_lowercase(),
                });
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push(Word {
            start: s,
            end: text.len(),
            key: text[s..].to_lowercase(),
        });
    }
    out
}

/// Replaces every spoken trigger in `text` with its snippet. Longer triggers
/// win over shorter ones that start the same way.
pub fn apply(text: &str, snippets: &[Snippet]) -> String {
    let mut triggers: Vec<(Vec<String>, &Snippet)> = snippets
        .iter()
        .map(|s| (words(&s.trigger).into_iter().map(|w| w.key).collect(), s))
        .filter(|(keys, s): &(Vec<String>, &Snippet)| !keys.is_empty() && !s.text.is_empty())
        .collect();
    if triggers.is_empty() {
        return text.to_string();
    }
    triggers.sort_by_key(|(keys, _)| std::cmp::Reverse(keys.len()));

    let spoken = words(text);
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i < spoken.len() {
        let hit = triggers.iter().find(|(keys, _)| {
            i + keys.len() <= spoken.len()
                && keys
                    .iter()
                    .zip(&spoken[i..])
                    .all(|(want, got)| *want == got.key)
        });
        let Some((keys, snippet)) = hit else {
            i += 1;
            continue;
        };
        let start = spoken[i].start;
        let mut end = spoken[i + keys.len() - 1].end;
        if snippet.exact {
            // The full stop (or comma) formatting put after the phrase.
            let rest = &text[end..];
            let punctuation = rest
                .chars()
                .take_while(|c| matches!(c, '.' | ',' | ';' | ':' | '!' | '?'))
                .map(char::len_utf8)
                .sum::<usize>();
            end += punctuation;
        }
        out.push_str(&text[copied..start]);
        out.push_str(&snippet.text);
        copied = end;
        i += keys.len();
    }
    out.push_str(&text[copied..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snip(trigger: &str, text: &str, exact: bool) -> Snippet {
        Snippet {
            trigger: trigger.into(),
            text: text.into(),
            exact,
        }
    }

    #[test]
    fn expands_the_phrase_ignoring_case_and_punctuation() {
        let s = [snip("email sign-off", "Thanks,\nKrishna", true)];
        assert_eq!(apply("Email sign off.", &s), "Thanks,\nKrishna");
        assert_eq!(
            apply("Cheers, email sign-off", &s),
            "Cheers, Thanks,\nKrishna"
        );
    }

    #[test]
    fn exact_swallows_trailing_punctuation_and_inline_keeps_it() {
        let exact = [snip("my address", "12 Main Street", true)];
        let inline = [snip("my address", "12 Main Street", false)];
        let said = "Send it to my address.";
        assert_eq!(apply(said, &exact), "Send it to 12 Main Street");
        assert_eq!(apply(said, &inline), "Send it to 12 Main Street.");
    }

    #[test]
    fn longer_trigger_wins_and_replaced_text_is_not_rescanned() {
        let s = [
            snip("my address", "A", false),
            snip("my address at work", "B my address", false),
        ];
        assert_eq!(
            apply("use my address at work today", &s),
            "use B my address today"
        );
    }

    #[test]
    fn unrelated_text_and_empty_lists_pass_through() {
        let s = [snip("standup", "Yesterday:\nToday:", true)];
        assert_eq!(
            apply("Nothing to expand here.", &s),
            "Nothing to expand here."
        );
        assert_eq!(apply("My address.", &[]), "My address.");
        assert_eq!(apply("standups are long", &s), "standups are long");
    }

    #[test]
    fn a_trigger_must_match_whole_words() {
        let s = [snip("sign off", "X", false)];
        assert_eq!(apply("design offer", &s), "design offer");
    }
}
