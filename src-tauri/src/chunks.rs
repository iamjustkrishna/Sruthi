//! Splits long dictations into pieces at pauses, so the speech model can
//! work through them one piece at a time, most of it while the key is still
//! held (see `dictation.rs`). Parakeet's time grows faster than the audio
//! (17 s took 1.6 s, 80 s took 11-14 s), so pieces also cost less in total.
//!
//! Pieces end in the middle of a pause, so no word is cut in two and each
//! piece starts and ends with a little silence.

use crate::audio::TARGET_RATE;

/// Pieces are at least this long: short dictations stay one piece, and
/// each piece has enough context for the model.
pub const MIN_PIECE_SECS: f32 = 12.0;
/// Longest piece. Past this, the piece ends at the quietest moment found.
pub const MAX_PIECE_SECS: f32 = 24.0;
/// A pause this long is the end of a sentence or a breath between phrases,
/// a safe place to split.
const GOOD_PAUSE_SECS: f32 = 0.5;
/// 20 ms frames.
const FRAME: usize = TARGET_RATE as usize / 50;
/// A frame counts as quiet below this share of the way from the noise
/// floor to the speech level (-26 dB under speech in a silent room).
const QUIET_SHARE: f32 = 0.05;
/// ...and always when it's less than twice the noise floor (`enhance.rs`
/// counts frames above that as speech).
const QUIET_OVER_NOISE: f32 = 2.0;

/// Where the next piece of `audio` ends (a sample index), or `None` when it
/// should wait for more audio. `audio` is 16 kHz and starts where the last
/// piece ended; `ended` says the recording is complete, in which case
/// `None` means the rest is the last piece.
pub fn next_cut(audio: &[f32], ended: bool) -> Option<usize> {
    let (min, max) = (frames_in(MIN_PIECE_SECS), frames_in(MAX_PIECE_SECS));
    let good = frames_in(GOOD_PAUSE_SECS);
    let levels: Vec<f32> = audio.chunks(FRAME).map(level).collect();
    if (ended && levels.len() <= max) || levels.len() < min + good {
        return None;
    }
    let runs = quiet_runs(&levels);
    let mid = |&(start, end): &(usize, usize)| (start + end) / 2;
    // A pause still going on at the end of the audio may run longer; wait
    // for it to finish.
    let finished = |&&(_, end): &&(usize, usize)| ended || end < levels.len();
    let in_range = |run: &&(usize, usize)| (min..=max).contains(&mid(run));
    let longest = |runs: &mut dyn Iterator<Item = &(usize, usize)>| {
        runs.max_by_key(|(start, end)| end - start).map(mid)
    };

    if let Some(cut) = longest(
        &mut runs
            .iter()
            .filter(finished)
            .filter(in_range)
            .filter(|(s, e)| e - s >= good),
    ) {
        return Some(cut * FRAME);
    }
    if !ended && levels.len() < max {
        return None;
    }
    // No clear pause in time: the longest quiet stretch, else the quietest
    // moment (100 ms average).
    if let Some(cut) = longest(&mut runs.iter().filter(in_range)) {
        return Some(cut * FRAME);
    }
    let smoothed = |i: usize| {
        levels[i.saturating_sub(2)..(i + 3).min(levels.len())]
            .iter()
            .sum::<f32>()
    };
    let quietest =
        (min..=max.min(levels.len() - 1)).min_by(|&a, &b| smoothed(a).total_cmp(&smoothed(b)))?;
    Some(quietest * FRAME)
}

/// Words that are only capitalized at the start of a sentence.
const SENTENCE_WORDS: &[&str] = &[
    "a", "about", "all", "also", "an", "and", "any", "are", "as", "at", "be", "because", "but",
    "by", "can", "could", "did", "do", "does", "for", "from", "had", "has", "have", "he", "her",
    "here", "his", "how", "if", "in", "is", "it", "its", "just", "like", "my", "no", "not", "of",
    "on", "or", "our", "she", "should", "so", "some", "that", "the", "their", "then", "there",
    "these", "they", "this", "those", "to", "was", "we", "were", "what", "when", "where", "which",
    "who", "with", "would", "you", "your",
];

/// Joins the pieces' texts. The model writes each piece like a whole
/// dictation, with a period at the end and a capital at the start, even
/// where a pause split a sentence. Where the next piece plainly carries on
/// the sentence, that's undone: "safer than the collateral. is the extent"
/// and "it looks like The probability".
pub fn join(texts: &[String]) -> String {
    let mut out = String::new();
    for text in texts.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
        if out.is_empty() {
            out.push_str(text);
            continue;
        }
        let first = text.split_whitespace().next().unwrap_or_default();
        let bare = first.trim_end_matches(|c: char| !c.is_alphanumeric());
        let last = out.rsplit(' ').next().unwrap_or_default();
        if text.starts_with(char::is_lowercase)
            && last.ends_with('.')
            && last.matches('.').count() == 1
        {
            out.pop();
            out = out.trim_end().to_string();
            out.push(' ');
            out.push_str(text);
        } else if out.ends_with(char::is_alphanumeric)
            && SENTENCE_WORDS.contains(&bare.to_lowercase().as_str())
            && bare[1..].chars().all(char::is_lowercase)
        {
            out.push(' ');
            out.extend(text.chars().next().map(|c| c.to_ascii_lowercase()));
            out.push_str(&text[1..]);
        } else {
            out.push(' ');
            out.push_str(text);
        }
    }
    out
}

fn frames_in(secs: f32) -> usize {
    (secs * TARGET_RATE as f32) as usize / FRAME
}

/// Loudness of a frame, ignoring any DC offset from the microphone.
fn level(frame: &[f32]) -> f32 {
    let mean = frame.iter().sum::<f32>() / frame.len() as f32;
    (frame.iter().map(|s| (s - mean) * (s - mean)).sum::<f32>() / frame.len() as f32).sqrt()
}

/// Stretches of quiet frames, as (first, past-the-end) frame indices.
fn quiet_runs(levels: &[f32]) -> Vec<(usize, usize)> {
    let mut sorted = levels.to_vec();
    sorted.sort_by(f32::total_cmp);
    // Fast talkers pause less than a tenth of the time, so the floor is
    // taken lower than `enhance.rs` does.
    let noise = sorted[sorted.len() / 20];
    let speech = sorted[sorted.len() * 9 / 10];
    let quiet = (noise * QUIET_OVER_NOISE).max(noise + (speech - noise) * QUIET_SHARE);

    let mut runs = Vec::new();
    let mut start = None;
    for (i, &l) in levels.iter().enumerate() {
        match (l <= quiet, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                runs.push((s, i));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        runs.push((s, levels.len()));
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: usize = TARGET_RATE as usize;

    /// "Speech" (a 200 Hz tone with a slow wobble) and silences, in seconds,
    /// alternating, starting with speech. Quiet hiss throughout.
    fn audio(parts: &[f32]) -> Vec<f32> {
        let mut seed = 99u32;
        let mut out = Vec::new();
        for (n, &secs) in parts.iter().enumerate() {
            for i in 0..(secs * RATE as f32) as usize {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let hiss = ((seed >> 16) as f32 / 32_768.0 - 1.0) * 0.001;
                let t = i as f32 / RATE as f32;
                let voice = if n % 2 == 0 {
                    0.1 * (6.28 * 200.0 * t).sin() * (0.6 + 0.4 * (6.28 * 3.0 * t).sin())
                } else {
                    0.0
                };
                out.push(voice + hiss);
            }
        }
        out
    }

    fn secs(samples: usize) -> f32 {
        samples as f32 / RATE as f32
    }

    #[test]
    fn short_dictations_are_one_piece() {
        let a = audio(&[5.0, 0.8, 5.0]);
        assert_eq!(next_cut(&a, false), None);
        assert_eq!(next_cut(&a, true), None);
        // Even past the minimum, a finished recording up to the maximum stays whole.
        let a = audio(&[10.0, 0.8, 10.0]);
        assert_eq!(next_cut(&a, true), None);
    }

    #[test]
    fn splits_in_the_middle_of_the_first_pause_past_the_minimum() {
        // Pauses at 8-8.6 s (too early) and 14-14.8 s.
        let a = audio(&[8.0, 0.6, 5.4, 0.8, 4.0]);
        let cut = secs(next_cut(&a, false).expect("cut"));
        assert!((14.3..=14.5).contains(&cut), "{cut}");
    }

    #[test]
    fn waits_for_a_pause_to_finish() {
        // Recording so far ends 0.6 s into a pause.
        let a = audio(&[14.0, 0.6]);
        assert_eq!(next_cut(&a, false), None);
        // Once the speaker carries on, the pause can be used.
        let a = audio(&[14.0, 0.7, 0.5]);
        let cut = secs(next_cut(&a, false).expect("cut"));
        assert!((14.2..=14.5).contains(&cut), "{cut}");
    }

    #[test]
    fn waits_while_no_good_pause_and_under_the_maximum() {
        // Only short gaps between "words".
        let a = audio(&[13.0, 0.2, 3.0, 0.2, 3.0]);
        assert_eq!(next_cut(&a, false), None);
    }

    #[test]
    fn long_speech_without_pauses_ends_at_the_quietest_moment() {
        let a = audio(&[13.0, 0.2, 6.0, 0.15, 6.0]);
        let cut = secs(next_cut(&a, false).expect("forced cut"));
        assert!((13.0..=13.2).contains(&cut), "{cut}");
        // Not one gap at all: still ends within the limits.
        let a = audio(&[30.0]);
        let cut = secs(next_cut(&a, false).expect("forced cut"));
        assert!((MIN_PIECE_SECS..=MAX_PIECE_SECS).contains(&cut), "{cut}");
    }

    #[test]
    fn finished_long_recording_is_split() {
        let a = audio(&[13.0, 0.6, 13.0]);
        let cut = secs(next_cut(&a, true).expect("cut"));
        assert!((13.2..=13.4).contains(&cut), "{cut}");
    }

    #[test]
    fn digital_silence_counts_as_quiet() {
        // The mic gates silence to exact zeros.
        let mut a: Vec<f32> = (0..13 * RATE)
            .map(|i| 0.1 * (i as f32 * 0.08).sin())
            .collect();
        a.extend(vec![0.0; RATE]);
        a.extend((0..2 * RATE).map(|i| 0.1 * (i as f32 * 0.08).sin()));
        let cut = secs(next_cut(&a, false).expect("cut"));
        assert!((13.4..=13.6).contains(&cut), "{cut}");
    }

    #[test]
    fn join_mends_sentences_split_by_a_pause() {
        let join2 = |a: &str, b: &str| join(&[a.to_string(), b.to_string()]);
        assert_eq!(
            join2("safer than the collateral.", "is the extent."),
            "safer than the collateral is the extent."
        );
        assert_eq!(
            join2("on paper it looks like", "The probability is low."),
            "on paper it looks like the probability is low."
        );
        assert_eq!(
            join2("Because we have", "Because, you see"),
            "Because we have because, you see"
        );
        // Real sentence ends, names and "I" are kept.
        assert_eq!(
            join2("We'll go.", "The flight is late."),
            "We'll go. The flight is late."
        );
        assert_eq!(join2("thank you", "Krishna."), "thank you Krishna.");
        assert_eq!(join2("and then", "I left."), "and then I left.");
        assert_eq!(
            join2("based in the U.S.", "for now"),
            "based in the U.S. for now"
        );
        assert_eq!(join2("call", "THE office"), "call THE office");
    }

    #[test]
    fn join_skips_empty_pieces() {
        let texts = vec![
            "Hello there.".to_string(),
            " ".to_string(),
            "How are you? ".to_string(),
        ];
        assert_eq!(join(&texts), "Hello there. How are you?");
    }
}
