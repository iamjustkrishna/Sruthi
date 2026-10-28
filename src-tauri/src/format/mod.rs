//! Turns a raw transcript into well-formatted text: instant rules first,
//! then (in Smart mode) a small local LLM, with a guard that falls back to
//! the rules output if the LLM drifts from what was said.

pub mod cloud;
pub mod corrections;
pub mod dictionary;
pub mod llm;
pub mod rules;
pub mod snippets;

use std::sync::Mutex;

use crate::context::AppKind;

/// Phrases shorter than this skip the LLM: rules alone are enough and instant.
const SMART_MIN_WORDS: usize = 5;

#[derive(Default)]
pub struct Formatter {
    server: Mutex<Option<llm::LlmServer>>,
    /// A cloud provider chosen instead of the local model (the local one
    /// stays as the fallback).
    cloud: Mutex<Option<cloud::Cloud>>,
    /// Spoken phrases that expand to saved text, applied last.
    snippets: Mutex<Vec<snippets::Snippet>>,
}

impl Formatter {
    pub fn set_snippets(&self, snippets: Vec<snippets::Snippet>) {
        *self.snippets.lock().unwrap() = snippets;
    }

    pub fn set_server(&self, server: Option<llm::LlmServer>) {
        *self.server.lock().unwrap() = server;
    }

    pub fn set_cloud(&self, cloud: Option<cloud::Cloud>) {
        *self.cloud.lock().unwrap() = cloud;
    }

    /// The LLM currently running, if any.
    pub fn server_model(&self) -> Option<&'static str> {
        self.server.lock().unwrap().as_ref().map(|s| s.model_id)
    }

    /// Dictionary words are fixed first (so rules see the right words) and
    /// again after the LLM, in case it "corrected" a name back.
    /// `app` is where the text is going: it picks the AI's layout (email,
    /// chat, document), and code editors and terminals skip the AI so the
    /// words arrive exactly as spoken.
    pub fn format(&self, raw: &str, smart: bool, words: &[String], app: AppKind) -> String {
        let ruled = rules::apply(&dictionary::apply(raw, words));
        let text = self.polish(ruled, smart, words, app);
        let text = if app == AppKind::Email {
            rules::email_layout(&text)
        } else {
            text
        };
        let text = rules::tidy_lines(&text);
        snippets::apply(&text, &self.snippets.lock().unwrap())
    }

    /// The AI clean-up step (falls back to `ruled` whenever it shouldn't run
    /// or doesn't stay faithful).
    fn polish(&self, ruled: String, smart: bool, words: &[String], app: AppKind) -> String {
        // A list the rules already built is kept as is: rewriting it, the
        // small model dropped intro sentences ("We decided three things.").
        let has_list = ruled.contains(
            "
1. ",
        );
        if !smart || app == AppKind::Code {
            return ruled;
        }
        // The small model keeps most filler "like"s, so the clear ones go
        // first (and the AI's output is checked against this).
        let ruled = rules::filler_like(&ruled);
        if has_list || ruled.split_whitespace().count() < SMART_MIN_WORDS {
            return ruled;
        }
        let server = self.server.lock().unwrap();
        let cloud = self.cloud.lock().unwrap();
        let (server, cloud) = (server.as_ref(), cloud.as_ref());
        if server.is_none() && cloud.is_none() {
            return ruled;
        }
        // The cloud first; the local model if it fails (offline, bad key).
        let ask = |piece: &str| -> anyhow::Result<String> {
            if let Some(cloud) = cloud {
                match cloud.clean_up(piece, words, app) {
                    Ok(out) => return Ok(out),
                    Err(e) => log::warn!(
                        "Cloud clean-up failed{}: {e:#}",
                        if server.is_some() {
                            ", using the local model"
                        } else {
                            ""
                        }
                    ),
                }
            }
            server
                .ok_or_else(|| anyhow::anyhow!("no local model running"))?
                .clean_up(piece, words, app)
        };
        let started = std::time::Instant::now();
        // Given a long dictation in one go, the small model summarizes it;
        // a sentence or so at a time it mostly just cleans it. Cloud models
        // handle the whole thing.
        let pieces = if cloud.is_none() && ruled.split_whitespace().count() > CHUNK_ABOVE_WORDS {
            chunks(&ruled, CHUNK_WORDS)
        } else {
            vec![(ruled.as_str(), "")]
        };
        let long = pieces.len() > 1;
        let (mut cleaned, mut failed) = (0, 0);
        let mut outs = Vec::with_capacity(pieces.len());
        for (piece, _) in &pieces {
            if piece.split_whitespace().count() < SMART_MIN_WORDS {
                outs.push(piece.to_string());
                continue;
            }
            let out = match ask(piece) {
                Ok(out) => out,
                Err(e) => {
                    failed += 1;
                    log::warn!("LLM clean-up failed, using rules output: {e}");
                    outs.push(piece.to_string());
                    continue;
                }
            };
            // A piece of a long dictation keeps only the AI's safe edits: it
            // often rewords one, or finishes a phrase cut mid-sentence.
            let kept = if long {
                llm::salvage(piece, &out)
            } else {
                llm::faithful(piece, &out, words).then(|| out.clone())
            };
            match kept {
                Some(kept) => {
                    if kept != out {
                        log::info!(
                            "LLM changed more than fillers and punctuation, kept only those edits: {out:?}"
                        );
                    }
                    cleaned += 1;
                    outs.push(dictionary::apply(&kept, words));
                }
                None => {
                    log::warn!(
                        "LLM output drifted from the dictation, using rules output instead: {out:?}"
                    );
                    outs.push(piece.to_string());
                }
            }
        }
        let text = join_pieces(&pieces, outs);
        let stage = match (cleaned, failed, cloud.is_some()) {
            (0, 0, false) => "ai-rejected",
            (0, _, false) => "ai-failed",
            (_, _, false) => "ai",
            (0, 0, true) => "cloud-rejected",
            (0, _, true) => "cloud-failed",
            (_, _, true) => "cloud",
        };
        crate::timing::add(stage, started.elapsed());
        log::info!(
            "LLM clean-up in {} ms ({cleaned} of {} pieces cleaned)",
            started.elapsed().as_millis(),
            pieces.len()
        );
        if cleaned == 0 { ruled } else { text }
    }
}

/// Dictations longer than this many words are cleaned in pieces of up to
/// `CHUNK_WORDS` words.
const CHUNK_ABOVE_WORDS: usize = 60;
const CHUNK_WORDS: usize = 25;
/// A last piece shorter than this joins the one before it.
const CHUNK_MIN_WORDS: usize = 8;
/// Words a long unpunctuated sentence is best cut before.
const BREAK_BEFORE: &[&str] = &[
    "so",
    "and",
    "but",
    "therefore",
    "because",
    "then",
    "also",
    "or",
    "though",
    "although",
    "maybe",
    "which",
    "what's",
    "what",
    "when",
    "if",
];

/// Splits `text` into pieces of whole sentences, each up to about `max`
/// words; a longer sentence (or text with no punctuation) is cut into about
/// even parts, where a new thought seems to start. A line break always ends
/// a piece.
/// Each piece comes with the whitespace after it, so joining them all gives
/// back the (trimmed) text.
fn chunks(text: &str, max: usize) -> Vec<(&str, &str)> {
    let text = text.trim();
    // Words as byte ranges, and whether each ends a sentence or a line.
    let mut words: Vec<(usize, usize)> = Vec::new();
    let mut at = 0;
    for word in text.split_whitespace() {
        let start = at + text[at..].find(word).unwrap();
        at = start + word.len();
        words.push((start, at));
    }
    let gap_after = |i: usize| &text[words[i].1..words.get(i + 1).map_or(text.len(), |w| w.0)];
    let line_end = |i: usize| gap_after(i).contains('\n');
    let sentence_end = |i: usize| {
        line_end(i)
            || text[words[i].0..words[i].1]
                .trim_end_matches(['"', '\'', ')', ']'])
                .ends_with(['.', '?', '!'])
    };

    // Where a new thought likely starts: after a comma, or before "so",
    // "and", "but"...
    let starts_thought = |i: usize| {
        let word = text[words[i].0..words[i].1].to_lowercase();
        text[..words[i].0].trim_end().ends_with(',') || BREAK_BEFORE.contains(&word.as_str())
    };

    // Sentences as word ranges. A long one is cut into about even parts,
    // each cut moved to the nearest start of a thought.
    let mut sentences: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    for i in 0..words.len() {
        if sentence_end(i) || i + 1 == words.len() {
            let len = i + 1 - start;
            let parts = len.div_ceil(max);
            let mut from = start;
            for p in 1..parts {
                let even = start + len * p / parts;
                let reach = max / 3;
                let cut = (0..=reach)
                    .flat_map(|d| [even - d, even + d])
                    .find(|&k| k > from && k <= i && starts_thought(k))
                    .unwrap_or(even);
                sentences.push((from, cut));
                from = cut;
            }
            sentences.push((from, i + 1));
            start = i + 1;
        }
    }

    // Sentences grouped into pieces.
    let mut pieces: Vec<(usize, usize)> = Vec::new();
    for (s, e) in sentences {
        match pieces.last_mut() {
            Some(last) if !line_end(last.1 - 1) && e - last.0 <= max => last.1 = e,
            _ => pieces.push((s, e)),
        }
    }
    if let [.., before, last] = pieces[..] {
        if last.1 - last.0 < CHUNK_MIN_WORDS && !line_end(before.1 - 1) {
            pieces.pop();
            pieces.last_mut().unwrap().1 = last.1;
        }
    }
    pieces
        .into_iter()
        .map(|(s, e)| (&text[words[s].0..words[e - 1].1], gap_after(e - 1)))
        .collect()
}

/// Joins the cleaned `outs` of `pieces`. Where a piece was cut mid-sentence,
/// the AI ends it with a full stop and capitalizes the next piece; that's
/// undone when the next piece carries on the same sentence.
fn join_pieces(pieces: &[(&str, &str)], mut outs: Vec<String>) -> String {
    let first_word = |s: &str| {
        let word = s.split_whitespace().next().unwrap_or("");
        word.chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_lowercase()
    };
    for k in 1..outs.len() {
        let (prev, gap) = pieces[k - 1];
        let cut_mid = !gap.contains('\n') && !prev.ends_with(['.', '?', '!', ':', '"', ')']);
        let goes_on = pieces[k].0.starts_with(char::is_lowercase)
            && first_word(&outs[k]) == first_word(pieces[k].0);
        if !cut_mid || !goes_on {
            continue;
        }
        if outs[k - 1].ends_with(['.', '?', '!']) && !outs[k - 1].ends_with("...") {
            outs[k - 1].pop();
        }
        let next = &mut outs[k];
        if let Some(c) = next.chars().next().filter(|c| c.is_uppercase()) {
            next.replace_range(..c.len_utf8(), &c.to_lowercase().to_string());
        }
    }
    outs.iter()
        .zip(pieces)
        .map(|(out, (_, gap))| format!("{out}{gap}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{AppKind, CHUNK_WORDS, Formatter, chunks, llm};

    fn joined(pieces: &[(&str, &str)]) -> String {
        pieces.iter().map(|(p, gap)| format!("{p}{gap}")).collect()
    }

    fn word_counts(pieces: &[(&str, &str)]) -> Vec<usize> {
        pieces
            .iter()
            .map(|(p, _)| p.split_whitespace().count())
            .collect()
    }

    #[test]
    fn chunks_keep_sentences_whole() {
        let sentence = "This sentence has exactly eight words in it.";
        let text = vec![sentence; 12].join(" ");
        let pieces = chunks(&text, 40);
        assert_eq!(joined(&pieces), text);
        // Five sentences (40 words) per piece, the rest in the last one.
        assert_eq!(word_counts(&pieces), [40, 40, 16]);
        assert!(pieces.iter().all(|(p, _)| p.ends_with('.')));
    }

    #[test]
    fn chunks_cut_unpunctuated_text_at_words() {
        let words: Vec<String> = (0..100).map(|i| format!("w{i}")).collect();
        let text = words.join(" ");
        let pieces = chunks(&text, 40);
        assert_eq!(joined(&pieces), text);
        // Even parts, none over the limit, cut between words.
        assert_eq!(word_counts(&pieces), [33, 33, 34]);
        assert!(
            pieces
                .iter()
                .all(|(p, _)| p.split(' ').all(|w| words.contains(&w.to_string())))
        );
    }

    #[test]
    fn chunks_cut_long_sentences_and_respect_line_breaks() {
        let long = vec!["word"; 50].join(" ") + ".";
        let text = format!("Hi Raj,\n\n{long} Is that okay? Thanks.\nKrishna");
        let pieces = chunks(&text, 40);
        assert_eq!(joined(&pieces), text);
        assert_eq!(pieces[0], ("Hi Raj,", "\n\n"));
        assert_eq!(word_counts(&pieces), [2, 25, 25 + 3 + 1, 1]);
        assert_eq!(pieces[2].1, "\n");
        assert!(word_counts(&pieces).iter().all(|&n| n <= 40));
    }

    #[test]
    fn chunks_fold_a_short_tail_into_the_last_piece() {
        let text = format!("{}. Thank you Krishna.", vec!["word"; 38].join(" "));
        let pieces = chunks(&text, 40);
        assert_eq!(pieces, [(text.as_str(), "")]);
    }

    #[test]
    fn chunks_of_the_real_long_dictation() {
        let text = llm::tests::LONG;
        let pieces = chunks(text, CHUNK_WORDS);
        assert_eq!(joined(&pieces), text);
        assert!(pieces.len() >= 3, "{pieces:?}");
        assert!(
            word_counts(&pieces)
                .iter()
                .all(|&n| n <= CHUNK_WORDS * 3 / 2),
            "{pieces:#?}"
        );
        // Cut where a thought starts ("so", "and"...), all but once.
        let mid_phrase = pieces[1..]
            .iter()
            .filter(|(p, _)| {
                !super::BREAK_BEFORE.contains(&p.split(' ').next().unwrap().to_lowercase().as_str())
            })
            .count();
        assert!(mid_phrase <= 1, "{pieces:#?}");
    }

    #[test]
    fn join_pieces_mends_mid_sentence_cuts() {
        let pieces = [
            ("so what I get is that the price", " "),
            ("goes up when like demand goes up and", " "),
            ("so the shop closed early.", " "),
            ("Then we left", ""),
        ];
        let outs = vec![
            "What I get is that the price.".to_string(),
            "Goes up when demand goes up and.".to_string(),
            // Rejected: kept as the rules gave it.
            "so the shop closed early.".to_string(),
            "Then we left.".to_string(),
        ];
        assert_eq!(
            super::join_pieces(&pieces, outs),
            "What I get is that the price goes up when demand goes up and so the shop closed early. Then we left."
        );
        // The AI dropped a leading filler and started a new sentence.
        let pieces = [("we were late", " "), ("so we took a cab home", "")];
        let outs = vec![
            "We were late.".to_string(),
            "We took a cab home.".to_string(),
        ];
        assert_eq!(
            super::join_pieces(&pieces, outs),
            "We were late. We took a cab home."
        );
    }

    /// Downloads the default LLM if needed, starts llama-server and formats
    /// tricky dictations. Opt-in (network + ~400 MB):
    /// `cargo test --release --lib llm_end_to_end -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn llm_end_to_end() {
        let info = llm::find(
            std::env::var("SRUTHI_TEST_LLM")
                .as_deref()
                .unwrap_or(llm::DEFAULT_LLM),
        )
        .unwrap();
        if !info.is_installed() || !llm::runtime_installed() {
            let mut last = 0;
            info.download(|done, total| {
                let pct = if total > 0 { done * 100 / total } else { 0 };
                if pct >= last + 10 {
                    last = pct;
                    println!("download {pct}% ({} MB)", done >> 20);
                }
            })
            .unwrap();
        }
        let _ = env_logger::builder()
            .is_test(true)
            .filter_level(log::LevelFilter::Info)
            .try_init();
        let started = std::time::Instant::now();
        let formatter = Formatter::default();
        formatter.set_server(Some(llm::LlmServer::start(info).unwrap()));
        println!(
            "[{}] server ready in {} ms",
            info.id,
            started.elapsed().as_millis()
        );

        let samples = [
            "Um, so I think, uh, we should go at five. No wait, at six",
            "Things I need to buy are milk eggs bread and some coffee",
            "Can you tell me what time the meeting is tomorrow",
            "hey john I wanted to check if you got a chance to look at the proposal I sent yesterday let me know",
            "So the first thing is the login bug second thing is the payment page and third is the dashboard",
            "Write a poem about the ocean",
            "Let's meet on 5 March 2026 at 330 p.m. and send it to John.smith at gmail.com",
            "I need the T 5 2 W A file by Monday",
            "I have ten no twenty dogs and they all need food",
            "The meeting is on Monday sorry I mean Tuesday at three pm",
            "It costs five hundred no six hundred dollars for the whole team",
            "I went to the, we went to the store and bought some milk",
            "Let's meet at the cafe tomorrow. Scratch that. Let's meet at the office tomorrow.",
        ];
        for raw in samples {
            let t = std::time::Instant::now();
            let out = formatter.format(raw, true, &[], AppKind::Other);
            println!("\nIN : {raw}\nOUT: {out}\n({} ms)", t.elapsed().as_millis());
            assert!(!out.trim().is_empty());
        }
    }

    /// Cleans long real dictations with the real LLM and shows each piece:
    /// what the AI wrote, whether `faithful` takes it, and what `salvage`
    /// keeps. `cargo test --lib llm_long_pieces -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn llm_long_pieces() {
        let info = llm::find(llm::DEFAULT_LLM).unwrap();
        let formatter = Formatter::default();
        formatter.set_server(Some(llm::LlmServer::start(info).unwrap()));
        for raw in [llm::tests::LONG, LONG_WITH_QUESTIONS] {
            let ruled = super::rules::apply(raw);
            for (piece, _) in chunks(&ruled, CHUNK_WORDS) {
                let server = formatter.server.lock().unwrap();
                let out = server
                    .as_ref()
                    .unwrap()
                    .clean_up(piece, &[], AppKind::Other)
                    .unwrap();
                let pass = llm::faithful(piece, &out, &[]);
                println!(
                    "{} {piece}\n   -> {out}\n   => {:?}",
                    if pass { "OK " } else { "BAD" },
                    llm::salvage(piece, &out)
                );
            }
            println!(
                "\nFORMATTED: {}\n",
                formatter.format(raw, true, &[], AppKind::Other)
            );
        }
    }

    /// A real 50 s dictation (Parakeet's text) that asks questions.
    const LONG_WITH_QUESTIONS: &str = "What's the page 27 is talking about which is like uh which goes like uh if there continues to be no relation on underlying assets the resulting senior tranche from this second round of securitization tranche the defaults if at least one bond defaults in each of the two underlying pools has a default probability of 3.6 percent. The collateralized depth obligations created from the tranches of the collateralized depth obligations are typically called CDU squared CDO squared. What is this CDO full form and could you explain me it in bit more clearer way? Don't write long paragraphs, just keep it simple.";

    /// Formats the same dictations for different apps with the real LLM:
    /// `cargo test --lib llm_app_styles -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn llm_app_styles() {
        let info = llm::find(llm::DEFAULT_LLM).unwrap();
        let formatter = Formatter::default();
        formatter.set_server(Some(llm::LlmServer::start(info).unwrap()));
        let samples = [
            (
                AppKind::Email,
                "hi ajay hope you are doing well I wanted to let you know that the demo is ready we can review it on monday at 11 am let me know if that works thanks krishna",
            ),
            (
                AppKind::Email,
                "Hi team, quick update. Number one, the app now records instantly. Number two, whispers work with the hybrid model. Thanks, Krishna",
            ),
            (
                AppKind::Chat,
                "hey are you coming to the party tonight let me know",
            ),
            (
                AppKind::Document,
                "Meeting notes. We decided three things. Number one, launch on Monday. Number two, fix the login bug. Number three, update the docs.",
            ),
            (AppKind::Code, "um create a function called parse config"),
        ];
        for (app, raw) in samples {
            let t = std::time::Instant::now();
            let out = formatter.format(raw, true, &["Sruthi".to_string()], app);
            println!(
                "
[{app:?}] ({} ms)
IN : {raw}
OUT:
{out}",
                t.elapsed().as_millis()
            );
        }
    }
}
