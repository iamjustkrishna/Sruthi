//! Turns a raw transcript into well-formatted text: instant rules first,
//! then (in Smart mode) a small local LLM, with a guard that falls back to
//! the rules output if the LLM drifts from what was said.

pub mod dictionary;
pub mod llm;
pub mod rules;

use std::sync::Mutex;

use crate::context::AppKind;

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

    /// Dictionary words are fixed first (so rules see the right words) and
    /// again after the LLM, in case it "corrected" a name back.
    /// `app` is where the text is going: it picks the AI's layout (email,
    /// chat, document), and code editors and terminals skip the AI so the
    /// words arrive exactly as spoken.
    pub fn format(&self, raw: &str, smart: bool, words: &[String], app: AppKind) -> String {
        let ruled = rules::apply(&dictionary::apply(raw, words));
        let text = self.polish(ruled, smart, words, app);
        let text = if app == AppKind::Email { rules::email_layout(&text) } else { text };
        rules::tidy_lines(&text)
    }

    /// The AI clean-up step (falls back to `ruled` whenever it shouldn't run
    /// or doesn't stay faithful).
    fn polish(&self, ruled: String, smart: bool, words: &[String], app: AppKind) -> String {
        // A list the rules already built is kept as is: rewriting it, the
        // small model dropped intro sentences ("We decided three things.").
        let has_list = ruled.contains("
1. ");
        if !smart || has_list || app == AppKind::Code || ruled.split_whitespace().count() < SMART_MIN_WORDS {
            return ruled;
        }
        let server = self.server.lock().unwrap();
        let Some(server) = server.as_ref() else { return ruled };
        let started = std::time::Instant::now();
        match server.clean_up(&ruled, words, app) {
            Ok(out) if llm::faithful(&ruled, &out) => {
                log::info!("LLM clean-up in {} ms", started.elapsed().as_millis());
                dictionary::apply(&out, words)
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

#[cfg(test)]
mod tests {
    use super::{llm, AppKind, Formatter};

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
            let out = formatter.format(raw, true, &[], AppKind::Other);
            println!("\nIN : {raw}\nOUT: {out}\n({} ms)", t.elapsed().as_millis());
            assert!(!out.trim().is_empty());
        }
    }

    /// Formats the same dictations for different apps with the real LLM:
    /// `cargo test --lib llm_app_styles -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn llm_app_styles() {
        let info = llm::find(llm::DEFAULT_LLM).unwrap();
        let formatter = Formatter::default();
        formatter.set_server(Some(llm::LlmServer::start(info).unwrap()));
        let samples = [
            (AppKind::Email, "hi ajay hope you are doing well I wanted to let you know that the demo is ready we can review it on monday at 11 am let me know if that works thanks krishna"),
            (AppKind::Email, "Hi team, quick update. Number one, the app now records instantly. Number two, whispers work with the hybrid model. Thanks, Krishna"),
            (AppKind::Chat, "hey are you coming to the party tonight let me know"),
            (AppKind::Document, "Meeting notes. We decided three things. Number one, launch on Monday. Number two, fix the login bug. Number three, update the docs."),
            (AppKind::Code, "um create a function called parse config"),
        ];
        for (app, raw) in samples {
            let t = std::time::Instant::now();
            let out = formatter.format(raw, true, &["Srutam".to_string()], app);
            println!("
[{app:?}] ({} ms)
IN : {raw}
OUT:
{out}", t.elapsed().as_millis());
        }
    }
}