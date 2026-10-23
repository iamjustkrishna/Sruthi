//! SRUTHI PATCH: hotword boosting (contextual biasing) for greedy TDT
//! decoding. The user's dictionary words are split into vocabulary pieces
//! and stored in a prefix tree. While decoding, pieces that start or
//! continue one of those words get a small logit bonus, so rare words and
//! names come out spelled right instead of as common-word look-alikes.

use std::collections::HashMap;

/// Bonus for the first piece of a hotword. Kept small: it applies at
/// every word start.
pub const START_BOOST: f32 = 1.0;
/// Bonus for continuing a hotword that has already started.
pub const CONTINUE_BOOST: f32 = 2.5;

pub struct Hotwords {
    /// Prefix tree over token ids; node 0 is the root.
    children: Vec<HashMap<i32, usize>>,
}

impl Hotwords {
    pub fn new(words: &[String], vocab: &[String]) -> Self {
        let index: HashMap<&str, i32> = vocab
            .iter()
            .enumerate()
            .filter(|(_, piece)| !piece.is_empty() && !piece.starts_with('<'))
            .map(|(id, piece)| (piece.as_str(), id as i32))
            .collect();
        let longest_piece = index.keys().map(|p| p.chars().count()).max().unwrap_or(1);

        let mut hotwords = Self { children: vec![HashMap::new()] };
        for word in words {
            let words: Vec<&str> = word.split_whitespace().collect();
            if words.is_empty() {
                continue;
            }
            // The model may write the word as typed, lowercase or capitalized.
            let typed = words.join(" ");
            let lower = typed.to_lowercase();
            let capitalized = words.iter().map(|w| capitalize(w)).collect::<Vec<_>>().join(" ");
            let mut variants = vec![typed, lower, capitalized];
            variants.dedup();
            for variant in variants {
                if let Some(pieces) = tokenize(&format!(" {variant}"), &index, longest_piece) {
                    hotwords.insert(&pieces);
                }
            }
        }
        hotwords
    }

    pub fn is_empty(&self) -> bool {
        self.children[0].is_empty()
    }

    fn insert(&mut self, pieces: &[i32]) {
        let mut node = 0;
        for &piece in pieces {
            node = match self.children[node].get(&piece) {
                Some(&next) => next,
                None => {
                    self.children.push(HashMap::new());
                    let next = self.children.len() - 1;
                    self.children[node].insert(piece, next);
                    next
                }
            };
        }
    }

    /// Bonus per token given the hotwords currently in progress
    /// (`active` nodes). `allow_starts` is false when starting a new word
    /// would mean overriding a confident "nothing here" (blank).
    pub fn boosts(&self, active: &[usize], allow_starts: bool) -> HashMap<i32, f32> {
        let mut boosts: HashMap<i32, f32> = HashMap::new();
        if allow_starts {
            for &token in self.children[0].keys() {
                boosts.insert(token, START_BOOST);
            }
        }
        for &node in active {
            for &token in self.children[node].keys() {
                boosts.insert(token, CONTINUE_BOOST);
            }
        }
        boosts
    }

    /// Hotwords in progress after emitting `token`.
    pub fn advance(&self, active: &[usize], token: i32) -> Vec<usize> {
        std::iter::once(&0)
            .chain(active)
            .filter_map(|&node| self.children[node].get(&token).copied())
            .filter(|&next| !self.children[next].is_empty())
            .collect()
    }
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Splits `text` into vocabulary pieces by greedy longest match, close to
/// how SentencePiece segments it. None if some part has no piece.
fn tokenize(text: &str, index: &HashMap<&str, i32>, longest_piece: usize) -> Option<Vec<i32>> {
    let chars: Vec<char> = text.chars().collect();
    let mut pieces = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let found = (1..=longest_piece.min(chars.len() - i)).rev().find_map(|len| {
            let piece: String = chars[i..i + len].iter().collect();
            index.get(piece.as_str()).map(|&id| (id, len))
        });
        let (id, len) = found?;
        pieces.push(id);
        i += len;
    }
    Some(pieces)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab() -> Vec<String> {
        // id: piece (" " marks a word start, like "▁" in SentencePiece)
        ["<blk>", " T", "au", "ri", " t", "or", "i", " K", "uber", "net", "es", " the"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn boosts_starts_then_continuations() {
        let h = Hotwords::new(&["Tauri".to_string()], &vocab());
        assert!(!h.is_empty());
        // " T" (id 1) starts "Tauri"; " t" (id 4) starts lowercase "tauri".
        let start = h.boosts(&[], true);
        assert_eq!(start.get(&1), Some(&START_BOOST));
        assert!(h.boosts(&[], false).is_empty());

        let active = h.advance(&[], 1);
        assert_eq!(active.len(), 1);
        assert_eq!(h.boosts(&active, false).get(&2), Some(&CONTINUE_BOOST)); // "au"
        let active = h.advance(&active, 2);
        assert_eq!(h.boosts(&active, false).get(&3), Some(&CONTINUE_BOOST)); // "ri"
        // Word complete: nothing left in progress.
        assert!(h.advance(&active, 3).is_empty());
    }

    #[test]
    fn unrelated_tokens_end_a_match() {
        let h = Hotwords::new(&["Tauri".to_string()], &vocab());
        let active = h.advance(&[], 1);
        assert!(h.advance(&active, 11).is_empty()); // " the"
    }

    #[test]
    fn words_without_pieces_are_skipped() {
        let h = Hotwords::new(&["Zzz".to_string()], &vocab());
        assert!(h.is_empty());
    }
}
