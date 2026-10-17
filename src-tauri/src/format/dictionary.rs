//! Personal dictionary: words the user adds (names, product terms, rare
//! words). After transcription, any stretch of 1–4 words that *sounds like*
//! a dictionary entry is replaced with the entry's exact spelling, so
//! "Babanas War" becomes "Bhubaneswar" and "Cuba Ernets" becomes
//! "Kubernetes".

/// Longest stretch of transcript words matched against one entry.
const MAX_SPAN_WORDS: usize = 4;

/// Replaces sound-alikes of dictionary entries with the entries.
pub fn apply(text: &str, dictionary: &[String]) -> String {
    let entries: Vec<(&str, String)> = dictionary
        .iter()
        .map(|e| e.trim())
        .filter(|e| !e.is_empty())
        .map(|e| (e, sound_key(e)))
        .filter(|(_, key)| key.len() >= 2)
        .collect();
    if entries.is_empty() {
        return text.to_string();
    }

    let spelled = join_spelled(text, dictionary);
    let words: Vec<&str> = spelled.split(' ').collect();

    // Every span of 1–4 words that matches some entry: (start, len, entry, score).
    let mut candidates: Vec<(usize, usize, &str, f32)> = Vec::new();
    for i in 0..words.len() {
        for n in 1..=MAX_SPAN_WORDS.min(words.len() - i) {
            let span = &words[i..i + n];
            // Don't match across sentence breaks.
            if span[..n - 1].iter().any(|w| w.ends_with(['.', '?', '!', ',', ';', ':'])) {
                break;
            }
            let joined: String = span.iter().map(|w| letters(w)).collect();
            if joined.is_empty() {
                continue;
            }
            let key = sound_key(&joined);
            for (entry, entry_key) in &entries {
                let entry_letters = letters(entry);
                // Spelled the same but cased/split differently ("srutam
                // dictate", "anti-gravity") beats any sound-alike.
                let score = if joined.eq_ignore_ascii_case(&entry_letters) {
                    2.0
                } else if entry_key.len() <= SHORT_KEY && !close_spelling(&joined, &entry_letters) {
                    0.0
                } else {
                    match_score(&key, entry_key)
                };
                if score >= threshold(entry_key) {
                    candidates.push((i, n, entry, score));
                }
            }
        }
    }

    // Take the best matches first, never overlapping; prefer shorter spans
    // on equal scores so surrounding words aren't swallowed.
    candidates.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
    let mut taken = vec![false; words.len()];
    let mut chosen: Vec<(usize, usize, &str)> = Vec::new();
    for (i, n, entry, _) in candidates {
        if taken[i..i + n].iter().any(|t| *t) {
            continue;
        }
        taken[i..i + n].iter_mut().for_each(|t| *t = true);
        chosen.push((i, n, entry));
    }
    chosen.sort_by_key(|c| c.0);

    let mut out: Vec<String> = Vec::with_capacity(words.len());
    let mut i = 0;
    let mut next = chosen.into_iter().peekable();
    while i < words.len() {
        match next.peek() {
            Some(&(start, n, entry)) if start == i => {
                let lead = leading_punct(words[i]);
                let trail = trailing_punct(words[i + n - 1]);
                out.push(format!("{lead}{entry}{trail}"));
                i += n;
                next.next();
            }
            _ => {
                out.push(words[i].to_string());
                i += 1;
            }
        }
    }
    out.join(" ")
}

/// Sound keys this short are too coarse on their own ("Tauri", "torii" and
/// "theory" all sound alike), so the spelling must be close too.
const SHORT_KEY: usize = 4;

/// True when `heard` (one or a few words) sounds roughly like `entry`: the
/// AI clean-up may swap a misheard name for its dictionary spelling.
pub fn sounds_like(heard: &str, entry: &str) -> bool {
    let (heard, entry) = (sound_key(heard), sound_key(entry));
    !entry.is_empty() && match_score(&heard, &entry) >= 0.65
}

/// Spelled within one letter of each other (two for 5+ letter words),
/// case-insensitive.
fn close_spelling(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_lowercase(), b.to_lowercase());
    let longest = a.chars().count().max(b.chars().count());
    let allowed = if longest >= 5 { 2.0 } else { 1.0 };
    let edits = (1.0 - match_score_raw(&a, &b)) * longest as f32;
    edits <= allowed + 0.01
}

/// Dictations longer than this never count as near misses: in sentences,
/// short names "sound close" to ordinary phrases ("Hansi" ~ "is a"), and
/// every false alarm costs seconds of Whisper.
const NEAR_MISS_MAX_WORDS: usize = 4;
/// Entries need at least this many sounds to be near-missed reliably.
const NEAR_MISS_MIN_KEY: usize = 5;

/// True when a very short dictation sounds *close to* a dictionary entry
/// without matching it ("Shruton" for "Srutam"): a sign the speech model
/// misheard a dictionary word, worth a second opinion. Text the
/// dictionary can already fix doesn't count.
pub fn has_near_miss(text: &str, dictionary: &[String]) -> bool {
    let words: Vec<&str> = text.split(' ').filter(|w| !w.is_empty()).collect();
    if words.is_empty() || words.len() > NEAR_MISS_MAX_WORDS {
        return false;
    }
    let entries: Vec<String> =
        dictionary.iter().map(|e| sound_key(e)).filter(|k| k.len() >= NEAR_MISS_MIN_KEY).collect();
    if entries.is_empty() || apply(text, dictionary) != text {
        return false;
    }
    (0..words.len()).any(|i| {
        (1..=3.min(words.len() - i)).any(|n| {
            let joined: String = words[i..i + n].iter().map(|w| letters(w)).collect();
            let key = sound_key(&joined);
            entries.iter().any(|entry| {
                let length = key.len() as f32 / entry.len() as f32;
                let score = match_score(&key, entry);
                (0.6..=1.67).contains(&length) && score >= 0.65 && score < threshold(entry)
            })
        })
    })
}

/// True when `text` contains one of the dictionary words (after fixing
/// sound-alikes), i.e. a second opinion actually found a name.
pub fn mentions_entry(text: &str, dictionary: &[String]) -> bool {
    let fixed = apply(text, dictionary).to_lowercase();
    dictionary.iter().any(|e| !e.trim().is_empty() && fixed.contains(&e.trim().to_lowercase()))
}

/// Letters spelled out one by one ("S-R U T A M", "S R U T A M") that
/// spell a dictionary entry become that entry.
fn join_spelled(text: &str, dictionary: &[String]) -> String {
    // A token like "S", "u." or "S-R" made only of single letters.
    let letters_of = |token: &str| -> Option<String> {
        let core = token.trim_matches(|c: char| !c.is_alphanumeric());
        let parts: Vec<&str> = core.split('-').collect();
        (!core.is_empty() && parts.iter().all(|p| p.chars().count() == 1 && p.chars().all(|c| c.is_alphabetic())))
            .then(|| parts.concat())
    };
    let tokens: Vec<&str> = text.split(' ').collect();
    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        let mut j = i;
        let mut spelled = String::new();
        while let Some(l) = tokens.get(j).and_then(|t| letters_of(t)) {
            spelled.push_str(&l);
            j += 1;
        }
        let entry = (spelled.chars().count() >= 3)
            .then(|| dictionary.iter().find(|e| close_spelling(&letters(e), &spelled)))
            .flatten();
        match entry {
            Some(entry) => {
                out.push(format!("{}{}", entry.trim(), trailing_punct(tokens[j - 1])));
                i = j;
            }
            None => {
                out.push(tokens[i].to_string());
                i += 1;
            }
        }
    }
    out.join(" ")
}

/// How similar two sound keys must be. Short words must sound exactly the
/// same, so common words like "story" are never turned into "Tauri".
fn threshold(entry_key: &str) -> f32 {
    match entry_key.len() {
        0..=4 => 1.0,
        5..=7 => 0.86,
        _ => 0.78,
    }
}

fn letters(word: &str) -> String {
    word.chars().filter(|c| c.is_alphanumeric()).collect()
}

fn leading_punct(word: &str) -> &str {
    let end = word.find(|c: char| c.is_alphanumeric()).unwrap_or(word.len());
    &word[..end]
}

fn trailing_punct(word: &str) -> &str {
    let start = word.rfind(|c: char| c.is_alphanumeric()).map(|i| i + 1).unwrap_or(0);
    let start = word[start..].char_indices().next().map(|_| start).unwrap_or(word.len());
    &word[start..]
}

/// A rough "how it sounds" key: letters that sound alike are merged
/// (c/k/q, s/z, v/w, ph/f, bh/b …) and each vowel run becomes one "a".
///
/// Tuned for Indian-English pronunciation too: "sh" and "s" are one sound,
/// and an "r" right after a starting "s"/"sh" is dropped, because "shr"
/// clusters get heard without the r. With "th" as "t", the real outputs
/// "Shoot them", "Shootham", "Shrutem", "Shutam" and "Sruitam" all share
/// the key of "Srutam".
fn sound_key(text: &str) -> String {
    let s: String = text.to_lowercase().chars().filter(|c| c.is_ascii_alphabetic()).collect();
    let b = s.as_bytes();
    let mut out = String::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        let next = b.get(i + 1).map(|&n| n as char);
        let (sound, used) = match (c, next) {
            ('p', Some('h')) => ('f', 2),
            ('c', Some('h')) => ('C', 2), // "ch"
            ('s', Some('h')) => ('s', 2), // "sh" sounds like "s"
            ('t', Some('h')) => ('t', 2),
            ('g', Some('h')) => ('g', 2),
            ('c', Some('k')) => ('k', 2),
            ('d', Some('g')) => ('j', 2),
            ('w', Some('r')) => ('r', 2),
            ('k', Some('n')) if i == 0 => ('n', 2),
            ('q', Some('u')) => ('k', 2),
            // Aspirated consonants common in Indian names: bh, dh, kh, jh.
            (x @ ('b' | 'd' | 'k' | 'j'), Some('h')) => (x, 2),
            ('c', Some('e' | 'i' | 'y')) => ('s', 1),
            ('c' | 'q', _) => ('k', 1),
            ('x', _) => ('k', 1), // "ks"; the s is close enough
            ('z', _) => ('s', 1),
            // t and d are often swapped in Indian-English speech ("Shudam").
            ('d', _) => ('t', 1),
            ('w', _) => ('v', 1),
            ('h', _) => ('\0', 1),
            ('a' | 'e' | 'i' | 'o' | 'u' | 'y', _) => ('a', 1),
            (x, _) => (x, 1),
        };
        // "sr"/"shr" at the start: the r is often not heard.
        let swallowed_r = sound == 'r' && out == "s";
        if sound != '\0' && !swallowed_r && !out.ends_with(sound) {
            out.push(sound);
        }
        i += used;
    }
    out
}

/// Similarity of two sound keys, 0.0–1.0 (1 - edit distance / length).
fn match_score(a: &str, b: &str) -> f32 {
    let (la, lb) = (a.chars().count() as f32, b.chars().count() as f32);
    // Lengths too different can't be the same word.
    if la < lb * 0.6 || lb < la * 0.6 {
        return 0.0;
    }
    match_score_raw(a, b)
}

/// 1 - edit distance / longer length.
fn match_score_raw(a: &str, b: &str) -> f32 {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let longest = a.len().max(b.len());
    if longest == 0 {
        return 0.0;
    }
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let tmp = row[j + 1];
            row[j + 1] = if ca == cb { prev } else { 1 + prev.min(row[j]).min(row[j + 1]) };
            prev = tmp;
        }
    }
    1.0 - row[b.len()] as f32 / longest as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict() -> Vec<String> {
        ["supercalifragilisticexpialidocious", "Tauri", "Kubernetes", "Bhubaneswar", "Antigravity", "Srutam Dictate"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn fixes_real_parakeet_mishearings() {
        let d = dict();
        // Every input below is real Parakeet output on synthetic speech.
        assert_eq!(
            apply("The word of the day is Supercalifragalistus expialidos.", &d),
            "The word of the day is supercalifragilisticexpialidocious."
        );
        assert_eq!(
            apply("The word of the day is Supercalifragalistus expalacious.", &d),
            "The word of the day is supercalifragilisticexpialidocious."
        );
        assert_eq!(
            apply("I am building Srutam dictate with Tori and Parakeet.", &d),
            "I am building Srutam Dictate with Tauri and Parakeet."
        );
        assert_eq!(apply("with Taurai and Parakeet.", &d), "with Tauri and Parakeet.");
        assert_eq!(
            apply("We deployed the service on Cuba Ernets yesterday.", &d),
            "We deployed the service on Kubernetes yesterday."
        );
        assert_eq!(
            apply("Ask Priyanka about the Bhuban Eshbar office.", &d),
            "Ask Priyanka about the Bhubaneswar office."
        );
        assert_eq!(
            apply("Ask Priyanka about the Babanas War Office.", &d),
            "Ask Priyanka about the Bhubaneswar Office."
        );
        assert_eq!(apply("Open the anti-gravity IDE", &d), "Open the Antigravity IDE");
    }

    #[test]
    fn leaves_ordinary_words_alone() {
        let d = dict();
        for text in [
            "Tell me a story about the tour.",
            "The torii gate and the theory of everything.",
            "Cuba is an island and Ernest is my friend.",
            "We need gravity and an antidote.",
        ] {
            assert_eq!(apply(text, &d), text);
        }
    }

    #[test]
    fn fixes_real_mishearings_of_srutam() {
        let d = vec!["Srutam".to_string()];
        // Every input is what Parakeet actually produced from the user's voice.
        for (heard, want) in [
            ("Shoot them.", "Srutam."),
            ("Shoot um", "Srutam"),
            ("Shrutem", "Srutam"),
            ("Shoot them is gonna be the best app people has ever seen.", "Srutam is gonna be the best app people has ever seen."),
            ("Return by Shutam dictate.", "Return by Srutam dictate."),
            ("Written by Sruitam Dictate.", "Written by Srutam Dictate."),
            ("Return by Sruotam Dictate.", "Return by Srutam Dictate."),
            ("Shootham dictate itself", "Srutam dictate itself"),
            ("Shudam.", "Srutam."),
            ("Shrutam.", "Srutam."),
        ] {
            assert_eq!(apply(heard, &d), want, "heard: {heard}");
        }
    }

    #[test]
    fn spelled_letters_become_the_word() {
        let d = vec!["Srutam".to_string()];
        assert_eq!(apply("type S-R U T A M to speak", &d), "type Srutam to speak");
        assert_eq!(apply("it is S R U T A M.", &d), "it is Srutam.");
        // Letters that spell nothing in the dictionary stay as they are.
        assert_eq!(apply("press A B C now", &d), "press A B C now");
    }

    #[test]
    fn near_misses_are_spotted() {
        let d = vec!["Srutam".to_string()];
        // Real Parakeet outputs for "Srutam" that sound-matching can't fix.
        assert!(has_near_miss("Shruton", &d));
        // Too far off to tell from the real word, or a whole sentence (not
        // worth seconds of Whisper; the dictionary and AI clean-up handle it).
        assert!(!has_near_miss("Shop.", &d));
        assert!(!has_near_miss("Shoot up. It's gonna be the best.", &d));
        // Already matched, or nothing like it.
        assert!(!has_near_miss("Shoot them.", &d));
        assert!(!has_near_miss("The meeting is at five tomorrow.", &d));
        assert!(!has_near_miss("I went to the shop and bought some stamps today.", &d));
    }

    #[test]
    fn sentences_never_trigger_near_misses() {
        // The user's real dictionary and dictations that wrongly sent
        // everything to Whisper (8 s each) before this rule.
        let d: Vec<String> = ["Ajay", "Hansi", "Jagjeet", "Jeena", "Mahendra", "Raghav", "Renu", "Srutam"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        for text in [
            "There is a very thin line between what is possible and what's not possible, but that line is now faded a lot.",
            "I want you to note down two points. Number one point.",
            "Do you know who is uh Raj?",
            "Right.",
        ] {
            assert!(!has_near_miss(text, &d), "{text}");
        }
        assert!(has_near_miss("Shruton", &d));
    }

    #[test]
    fn similar_ordinary_words_stay() {
        let d = vec!["Srutam".to_string()];
        for text in ["Show them the way.", "What is shooting?", "Shoot.", "The stem is green.", "Satan and Sudan."] {
            assert_eq!(apply(text, &d), text);
        }
    }

    #[test]
    fn empty_dictionary_changes_nothing() {
        assert_eq!(apply("anything at all", &[]), "anything at all");
    }
}

#[cfg(test)]
mod near_miss_debug {
    /// Prints which spans count as near misses in real sentences:
    /// `cargo test --lib near_miss_debug -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn show_triggers() {
        let dict: Vec<String> = std::env::var("SRUTAM_TEST_DICT").unwrap_or_default().split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        for text in [
            "There is a very thin line between what is possible and what's not possible, but that line is now faded a lot.",
            "I want you to note down two points. Number one point. We have to go to the party. Number two, we have to go in the car.",
            "Do you know who is uh Raj?",
            "Right.",
        ] {
            let words: Vec<&str> = text.split(' ').collect();
            let mut hits = Vec::new();
            for i in 0..words.len() {
                for n in 1..=3.min(words.len() - i) {
                    let span = words[i..i + n].join(" ");
                    if super::has_near_miss(&span, &dict) {
                        hits.push(span);
                    }
                }
            }
            println!("{} -> near_miss={} spans={:?}", text, super::has_near_miss(text, &dict), hits);
        }
    }
}
