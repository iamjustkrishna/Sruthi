//! Spoken self-corrections, fixed by rule so they work instantly, with or
//! without the AI clean-up:
//!
//! - a value corrected right after it's said: "I have 10 no 20 dogs" →
//!   "I have 20 dogs", "at five. No wait, at six" → "at six",
//!   "Monday, sorry, I mean Tuesday" → "Tuesday";
//! - "scratch that" right after a sentence drops that sentence;
//! - a phrase restarted once: "I'll try to do to do that" → "I'll try to do that".
//!
//! Only clear-cut cases are handled: the corrected value and its
//! replacement must be the same kind of thing (two numbers, two days, two
//! names…), so "No, I don't have 20 dogs" is left alone. Fuzzier ones
//! ("we should, actually let's just…") are left to the AI.

use super::rules::{is_number_word, split_punct};

/// Cues that always signal a correction.
const STRONG_CUES: &[&[&str]] = &[
    &["no", "wait"],
    &["no", "no"],
    &["no", "sorry"],
    &["sorry", "i", "mean"],
    &["no", "i", "mean"],
    &["i", "mean"],
    &["i", "meant"],
    &["or", "rather"],
    &["make", "that"],
    &["scratch", "that"],
    &["correction"],
];

/// Cues that are also everyday words: only trusted between two numbers,
/// days or months in the same sentence ("10 no 20").
const WEAK_CUES: &[&[&str]] = &[&["no"], &["sorry"], &["wait"], &["oops"]];

/// After a finished sentence, these drop it: "Meet Monday. Scratch that."
const DROP_CUES: &[&[&str]] = &[&["scratch", "that"], &["delete", "that"], &["strike", "that"], &["erase", "that"]];

/// Words that may come before the value and get repeated with the
/// correction ("at five, no wait, at six").
const PREPOSITIONS: &[&str] = &["at", "on", "in", "by", "for", "to", "from", "until", "till", "before", "after", "around", "about"];

const DAYS: &[&str] = &[
    "monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday", "today", "tomorrow", "yesterday", "tonight",
];
const MONTHS: &[&str] = &[
    "january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december",
];

/// How far back (in words) the corrected value may be from the cue.
const MAX_GAP: usize = 3;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Number,
    Day,
    Month,
    Name,
}

fn word(token: &str) -> String {
    split_punct(token).1.to_lowercase()
}

fn ends_sentence(token: &str) -> bool {
    split_punct(token).2.contains(['.', '!', '?'])
}

fn kind(token: &str, sentence_start: bool) -> Option<Kind> {
    let (_, w, _) = split_punct(token);
    let lower = w.to_lowercase();
    let numeric = !w.is_empty()
        && w.starts_with(|c: char| c.is_ascii_digit())
        && w.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ',' | ':' | '%'));
    if numeric || is_number_word(&lower) {
        Some(Kind::Number)
    } else if DAYS.contains(&lower.as_str()) {
        Some(Kind::Day)
    } else if MONTHS.contains(&lower.as_str()) && lower != "may" {
        Some(Kind::Month)
    } else if !sentence_start && w.len() > 1 && w.starts_with(|c: char| c.is_uppercase()) && w != "I" {
        Some(Kind::Name)
    } else {
        None
    }
}

fn starts_sentence(tokens: &[String], i: usize) -> bool {
    i == 0 || ends_sentence(&tokens[i - 1])
}

/// Length of the cue starting at `i`, longest first.
fn cue_at(tokens: &[String], i: usize, cues: &[&[&str]]) -> Option<usize> {
    cues.iter()
        .filter(|cue| i + cue.len() <= tokens.len() && cue.iter().enumerate().all(|(k, c)| word(&tokens[i + k]) == *c))
        .map(|cue| cue.len())
        .max()
}

/// Uppercases the first letter of a token.
fn capitalize(token: &str) -> String {
    let mut chars = token.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// "I have 10 no 20 dogs" → "I have 20 dogs".
pub fn values(text: &str) -> String {
    text.split('\n').map(values_in_line).collect::<Vec<_>>().join("\n")
}

fn values_in_line(line: &str) -> String {
    let mut tokens: Vec<String> = line.split(' ').filter(|t| !t.is_empty()).map(String::from).collect();
    let mut i = 1;
    while i < tokens.len() {
        match correct_at(&tokens, i) {
            Some((start, cue_end, tail)) => {
                let capital = starts_sentence(&tokens, start);
                let mut rest: Vec<String> = tokens.split_off(cue_end);
                // Words between the old value and the cue ("10 dogs, no,
                // 20") belong after the new one when it doesn't repeat them.
                if !tail.is_empty() {
                    let y = rest.iter().position(|t| kind(t, false).is_some()).unwrap_or(0);
                    let repeated = rest.get(y + 1).is_some_and(|t| word(t) == word(&tail[0]));
                    if !repeated {
                        let (lead, w, trail) = split_punct(&rest[y]);
                        let (bare, trail) = (format!("{lead}{w}"), trail.to_string());
                        rest[y] = bare;
                        let mut moved: Vec<String> = tail.iter().map(|t| split_punct(t).1.to_string()).collect();
                        if let Some(last) = moved.last_mut() {
                            last.push_str(&trail);
                        }
                        rest.splice(y + 1..y + 1, moved);
                    }
                }
                tokens.truncate(start);
                if capital {
                    if let Some(first) = rest.first_mut() {
                        *first = capitalize(first);
                    }
                }
                tokens.extend(rest);
                i = start.max(1);
            }
            None => i += 1,
        }
    }
    tokens.join(" ")
}

/// If a correction cue starts at `i`, returns where the corrected part
/// starts, where the cue ends, and the words between the old value and the
/// cue.
fn correct_at(tokens: &[String], i: usize) -> Option<(usize, usize, Vec<String>)> {
    let (len, strong) = match (cue_at(tokens, i, STRONG_CUES), cue_at(tokens, i, WEAK_CUES)) {
        (Some(len), _) => (len, true),
        (None, Some(len)) => (len, false),
        _ => return None,
    };
    let cue_end = i + len;
    // The new value, maybe after a repeated preposition.
    let mut y = cue_end;
    let new_prep = tokens.get(y).is_some_and(|t| PREPOSITIONS.contains(&word(t).as_str()));
    if new_prep {
        y += 1;
    }
    let y_kind = kind(tokens.get(y)?, false)?;
    if y_kind == Kind::Name && !strong {
        return None;
    }
    // The old value: the nearest same-kind word just before the cue.
    let j = (i.saturating_sub(MAX_GAP)..i).rev().find(|&j| kind(&tokens[j], starts_sentence(tokens, j)) == Some(y_kind))?;
    let tail: Vec<String> = tokens[j + 1..i].to_vec();
    if y_kind == Kind::Name && !tail.is_empty() {
        return None;
    }
    // A question isn't corrected by its answer ("Is it 5? No, 6.").
    if tokens[j..i].iter().any(|t| split_punct(t).2.contains('?')) {
        return None;
    }
    // Nothing may end a sentence between the old value and the cue, except
    // right before the cue ("at five. No wait, at six")...
    if tokens[j..i - 1].iter().any(|t| ends_sentence(t)) {
        return None;
    }
    // ...and after a full stop, an everyday cue only counts when the new
    // value ends the thought: "Meet at 5. Wait, 6 people are coming" is
    // not a correction.
    if !strong && ends_sentence(&tokens[i - 1]) {
        let after = tokens.get(y + 1);
        let ends = split_punct(&tokens[y]).2.contains(['.', ',', '!', '?', ';', ':']);
        if !(ends || after.is_none()) {
            return None;
        }
    }
    let had_prep = j > 0 && PREPOSITIONS.contains(&word(&tokens[j - 1]).as_str());
    let start = if new_prep && had_prep { j - 1 } else { j };
    Some((start, cue_end, tail))
}

/// "Let's meet Monday. Scratch that." → "" (the sentence before the cue
/// goes). Only after a finished sentence; mid-sentence ones are handled as
/// value corrections or left alone.
pub fn scratch_that(text: &str) -> String {
    text.split('\n').map(scratch_in_line).collect::<Vec<_>>().join("\n")
}

fn scratch_in_line(line: &str) -> String {
    let mut tokens: Vec<String> = line.split(' ').filter(|t| !t.is_empty()).map(String::from).collect();
    let mut i = 1;
    while i < tokens.len() {
        match cue_at(&tokens, i, DROP_CUES) {
            Some(len) if ends_sentence(&tokens[i - 1]) => {
                // Start of the sentence that ends at i - 1.
                let start = (0..i - 1).rev().find(|&k| ends_sentence(&tokens[k])).map(|k| k + 1).unwrap_or(0);
                tokens.drain(start..i + len);
                if let Some(first) = tokens.get_mut(start) {
                    *first = capitalize(first);
                }
                i = start.max(1);
            }
            _ => i += 1,
        }
    }
    tokens.join(" ")
}

/// Longest phrase checked for a restart.
const MAX_RESTART_WORDS: usize = 4;

/// A restart may swap its first word for one of these ("I went to the, we
/// went to the store")...
const SWAPPABLE: &[&str] = &[
    "i", "we", "you", "he", "she", "they", "it", "the", "a", "an", "this", "that", "my", "our", "your", "his", "her", "their",
];
/// ...when the abandoned attempt stops mid-phrase, on one of these. "I like
/// it, you like it" ends on a full thought and is left alone.
const DANGLING: &[&str] = &[
    "the", "a", "an", "to", "of", "in", "on", "at", "for", "with", "from", "and", "or", "but", "is", "are", "was", "were", "my", "our",
    "your", "his", "her", "their", "this", "that", "will", "would", "can", "should",
];

/// "I want to, I want to go" → "I want to go": a phrase of 2-4 words said
/// twice in a row. Phrases with numbers or single letters are kept ("my
/// PIN is 1 2 1 2", spelled codes), and so are sentences repeated after a
/// full stop.
pub fn restarts(text: &str) -> String {
    text.split('\n').map(restarts_in_line).collect::<Vec<_>>().join("\n")
}

fn restarts_in_line(line: &str) -> String {
    let mut tokens: Vec<String> = line.split(' ').filter(|t| !t.is_empty()).map(String::from).collect();
    let mut i = 0;
    while i < tokens.len() {
        let found = (2..=MAX_RESTART_WORDS).rev().find(|&n| {
            if i + 2 * n > tokens.len() {
                return false;
            }
            let first: Vec<String> = tokens[i..i + n].iter().map(|t| word(t)).collect();
            let second: Vec<String> = tokens[i + n..i + 2 * n].iter().map(|t| word(t)).collect();
            let same = first == second
                || (n >= 3
                    && first[1..] == second[1..]
                    && SWAPPABLE.contains(&first[0].as_str())
                    && SWAPPABLE.contains(&second[0].as_str())
                    && DANGLING.contains(&first[n - 1].as_str()));
            same && first.iter().all(|w| (w.chars().count() > 1 || w == "i" || w == "a") && !is_number_word(w) && !w.starts_with(|c: char| c.is_ascii_digit()))
                && !tokens[i..i + n].iter().any(|t| ends_sentence(t))
        });
        match found {
            Some(n) => {
                // Keep the capital of a sentence start ("I want to, I want to go").
                let capital = tokens[i].starts_with(|c: char| c.is_uppercase());
                tokens.drain(i..i + n);
                if capital {
                    tokens[i] = capitalize(&tokens[i]);
                }
            }
            None => i += 1,
        }
    }
    tokens.join(" ")
}

#[cfg(test)]
mod tests {
    use super::{restarts, scratch_that, values};

    /// Prints every change this step makes to the saved dictations, to spot
    /// false corrections: `cargo test --lib saved_dictations -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn saved_dictations() {
        let path = crate::recordings::folder().join("history.json");
        let history: Vec<serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let mut changed = 0;
        for entry in &history {
            let heard = entry["heard"].as_str().unwrap_or_default();
            let out = scratch_that(&values(&restarts(heard)));
            if out != heard {
                changed += 1;
                println!("IN : {heard}
OUT: {out}
");
            }
        }
        println!("{changed} of {} dictations changed", history.len());
    }

    #[test]
    fn numbers_corrected_on_the_spot() {
        assert_eq!(values("I have 10 no 20 dogs"), "I have 20 dogs");
        assert_eq!(values("I have 10, no, 20 dogs."), "I have 20 dogs.");
        assert_eq!(values("I have 10 dogs no 20"), "I have 20 dogs");
        assert_eq!(values("I have 10 dogs, no, 20 dogs."), "I have 20 dogs.");
        assert_eq!(values("It costs 500 I mean 600 rupees"), "It costs 600 rupees");
        assert_eq!(values("We should go at five. No wait, at six"), "We should go at six");
        assert_eq!(values("We should go at 5. No wait, at 6 because traffic is bad"), "We should go at 6 because traffic is bad");
        assert_eq!(values("Meet at 5. No, 6."), "Meet at 6.");
        assert_eq!(values("Call me at 3:30 PM sorry 4:30 PM"), "Call me at 4:30 PM");
        // Several in a row: the last one wins.
        assert_eq!(values("Give me 5 no 6 no 7 apples"), "Give me 7 apples");
    }

    #[test]
    fn days_months_and_names() {
        assert_eq!(values("We can meet on Monday. Sorry, I mean Tuesday at 10 AM"), "We can meet on Tuesday at 10 AM");
        assert_eq!(values("Let's do it tomorrow no wait today"), "Let's do it today");
        assert_eq!(values("The launch is in March no April"), "The launch is in April");
        assert_eq!(values("Send it to Raghav, I mean Jagjeet, by Friday."), "Send it to Jagjeet, by Friday.");
        // A name needs a strong cue: a bare "no" isn't enough.
        assert_eq!(values("I told Raghav no Jagjeet would come"), "I told Raghav no Jagjeet would come");
    }

    #[test]
    fn everyday_words_stay() {
        for text in [
            "I have no dogs.",
            "No, I don't have 20 dogs.",
            "Is it 5? No, 6.",
            "Meet at 5. Wait, 6 people are coming.",
            "There were 10 of us. No one else came.",
            "I mean it, 20 is too many.",
            "Sorry I'm late.",
            "We met on Monday. Sorry about that.",
            "Call me maybe.",
        ] {
            assert_eq!(values(text), text, "{text}");
        }
    }

    #[test]
    fn scratch_that_drops_the_sentence() {
        assert_eq!(scratch_that("Let's meet Monday. Scratch that. Let's meet Tuesday."), "Let's meet Tuesday.");
        assert_eq!(scratch_that("Hi Raj. The report is late. Delete that."), "Hi Raj.");
        // Mid-sentence it's a value correction, not a sentence drop.
        assert_eq!(scratch_that("Meet Monday, scratch that, Tuesday"), "Meet Monday, scratch that, Tuesday");
        assert_eq!(values("Meet Monday, scratch that, Tuesday"), "Meet Tuesday");
    }

    #[test]
    fn restarted_phrases() {
        // Real Parakeet output.
        assert_eq!(restarts("And I'll try to do to do that."), "And I'll try to do that.");
        assert_eq!(restarts("I want to, I want to go home"), "I want to go home");
        assert_eq!(restarts("we should we should leave"), "we should leave");
        // A false start with a different first word.
        assert_eq!(restarts("I went to the, we went to the store."), "We went to the store.");
        assert_eq!(restarts("so my car is our car is red"), "so our car is red");
        assert_eq!(restarts("I like it, you like it, everyone likes it."), "I like it, you like it, everyone likes it.");
        for text in ["My PIN is 1 2 1 2", "It was very very good", "I did it. I did it.", "that that is", "A B A B"] {
            assert_eq!(restarts(text), text, "{text}");
        }
    }
}
