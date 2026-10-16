//! Turns a raw transcript into well-formatted text: instant rules first,
//! then (in Smart mode) a small local LLM, with a guard that falls back to
//! the rules output if the LLM drifts from what was said.

pub mod llm;
pub mod rules;

use std::sync::Mutex;

/// Phrases shorter than this skip the LLM: rules alone are enough and instant.
const SMART_MIN_WORDS: usize = 5;

#[derive(Default)]
pub struct Formatter {
    server: Mutex<Option<llm::LlmServer>>,
}

impl Formatter {
    pub fn set_server(&self, server: Option<llm::LlmServer>) {
        *self.server.lock().unwrap() = server;
    }

    /// The LLM currently running, if any.
    pub fn server_model(&self) -> Option<&'static str> {
        self.server.lock().unwrap().as_ref().map(|s| s.model_id)
    }

    pub fn format(&self, raw: &str, smart: bool) -> String {
        let ruled = rules::apply(raw);
        if !smart || ruled.split_whitespace().count() < SMART_MIN_WORDS {
            return ruled;
        }
        let server = self.server.lock().unwrap();
        let Some(server) = server.as_ref() else { return ruled };
        let started = std::time::Instant::now();
        match server.clean_up(&ruled) {
            Ok(out) if llm::faithful(&ruled, &out) => {
                log::info!("LLM clean-up in {} ms", started.elapsed().as_millis());
                out
            }
            Ok(out) => {
                log::warn!("LLM output drifted from the dictation, using rules output instead: {out:?}");
                ruled
            }
            Err(e) => {
                log::warn!("LLM clean-up failed, using rules output: {e}");
                ruled
            }
        }
    }
}

/// Words that usually continue a sentence. A phrase that starts with one
/// was most likely split mid-sentence at a pause.
const CONTINUATIONS: &[&str] = &[
    "and", "but", "or", "so", "because", "which", "that", "to", "with", "of", "for", "then", "than", "if", "when",
    "while", "as", "in", "on", "at", "by", "from", "the", "a", "an", "is", "are", "was", "were",
];

/// Joins phrases transcribed separately (split at pauses), undoing the full
/// stop the model put at a pause that was really mid-sentence.
pub fn join_phrases(parts: &[String]) -> String {
    let mut out = String::new();
    for part in parts.iter().map(|p| p.trim_matches(' ')).filter(|p| !p.trim().is_empty()) {
        if out.is_empty() {
            out.push_str(part);
            continue;
        }
        let first_word: String = part.chars().take_while(|c| c.is_alphabetic()).collect::<String>().to_lowercase();
        let continues = CONTINUATIONS.contains(&first_word.as_str()) && out.ends_with('.') && !out.ends_with("..");
        if continues {
            out.pop();
            out.push(' ');
            let mut chars = part.chars();
            if let Some(c) = chars.next() {
                out.extend(c.to_lowercase());
                out.push_str(chars.as_str());
            }
        } else {
            if !out.ends_with('\n') {
                out.push(' ');
            }
            out.push_str(part);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{join_phrases, llm, Formatter};

    /// Downloads the default LLM if needed, starts llama-server and formats
    /// tricky dictations. Opt-in (network + ~400 MB):
    /// `cargo test --release --lib llm_end_to_end -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn llm_end_to_end() {
        let info = llm::find(std::env::var("SRUTAM_TEST_LLM").as_deref().unwrap_or(llm::DEFAULT_LLM)).unwrap();
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
        let _ = env_logger::builder().is_test(true).filter_level(log::LevelFilter::Info).try_init();
        let started = std::time::Instant::now();
        let formatter = Formatter::default();
        formatter.set_server(Some(llm::LlmServer::start(info).unwrap()));
        println!("[{}] server ready in {} ms", info.id, started.elapsed().as_millis());

        let samples = [
            "Um, so I think, uh, we should go at five. No wait, at six",
            "Things I need to buy are milk eggs bread and some coffee",
            "Can you tell me what time the meeting is tomorrow",
            "hey john I wanted to check if you got a chance to look at the proposal I sent yesterday let me know",
            "So the first thing is the login bug second thing is the payment page and third is the dashboard",
            "Write a poem about the ocean",
            "Let's meet on 5 March 2026 at 330 p.m. and send it to John.smith at gmail.com",
            "I need the T 5 2 W A file by Monday",
        ];
        for raw in samples {
            let t = std::time::Instant::now();
            let out = formatter.format(raw, true);
            println!("\nIN : {raw}\nOUT: {out}\n({} ms)", t.elapsed().as_millis());
            assert!(!out.trim().is_empty());
        }
    }

    #[test]
    fn joins_phrases_split_mid_sentence() {
        let parts = vec!["I want to go to the market.".to_string(), "And buy some milk.".to_string()];
        assert_eq!(join_phrases(&parts), "I want to go to the market and buy some milk.");
    }

    #[test]
    fn keeps_real_sentence_breaks() {
        let parts = vec!["The meeting is at 5.".to_string(), "Please be on time.".to_string()];
        assert_eq!(join_phrases(&parts), "The meeting is at 5. Please be on time.");
    }

    #[test]
    fn keeps_new_lines() {
        let parts = vec!["Dear team,\n".to_string(), "Thanks.".to_string()];
        assert_eq!(join_phrases(&parts), "Dear team,\nThanks.");
    }
}
