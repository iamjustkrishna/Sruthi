//! AI clean-up through a cloud provider with the user's own API key, as an
//! alternative to the local model: smarter, but the text (never the audio)
//! leaves the PC. Same instructions and examples as the local model, and the
//! same faithfulness check on what comes back; on any error the dictation
//! falls back to the local model or the rules.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};

use crate::context::AppKind;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Gemini,
    Groq,
    Anthropic,
}

pub const PROVIDERS: &[Provider] = &[Provider::Gemini, Provider::Groq, Provider::Anthropic];

/// Longest a cloud clean-up may take before the dictation falls back.
const TIMEOUT: Duration = Duration::from_secs(8);

/// Claude models that accept server-side refusal fallbacks
/// (`fallbacks: "default"`).
const CLAUDE_FALLBACK_MODELS: &[&str] = &["claude-opus-5-5", "claude-opus-5", "claude-fable-5-1", "claude-sonnet-5-5"];

impl Provider {
    pub fn id(self) -> &'static str {
        match self {
            Provider::Gemini => "gemini",
            Provider::Groq => "groq",
            Provider::Anthropic => "anthropic",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Provider::Gemini => "Google Gemini",
            Provider::Groq => "Groq",
            Provider::Anthropic => "Anthropic Claude",
        }
    }

    pub fn default_model(self) -> &'static str {
        match self {
            Provider::Gemini => "gemini-3.5-flash-lite",
            Provider::Groq => "openai/gpt-oss-20b",
            Provider::Anthropic => "claude-opus-5-5",
        }
    }

    /// Where to create an API key.
    pub fn key_url(self) -> &'static str {
        match self {
            Provider::Gemini => "https://aistudio.google.com/apikey",
            Provider::Groq => "https://console.groq.com/keys",
            Provider::Anthropic => "https://platform.claude.com/settings/keys",
        }
    }

    pub fn find(id: &str) -> Option<Provider> {
        PROVIDERS.iter().copied().find(|p| p.id() == id)
    }
}

/// A configured provider, ready to clean up dictations.
pub struct Cloud {
    pub provider: Provider,
    pub model: String,
    key: String,
    client: reqwest::blocking::Client,
}

impl Cloud {
    pub fn new(provider: Provider, model: Option<String>) -> Result<Self> {
        let key = crate::secrets::get(provider.id()).ok_or_else(|| anyhow!("No API key saved for {}", provider.name()))?;
        let model = model.filter(|m| !m.trim().is_empty()).unwrap_or_else(|| provider.default_model().to_string());
        Ok(Self { provider, model: model.trim().to_string(), key, client: reqwest::blocking::Client::builder().timeout(TIMEOUT).build()? })
    }

    /// Same contract as the local model's `clean_up`.
    pub fn clean_up(&self, text: &str, glossary: &[String], app: AppKind) -> Result<String> {
        let (system, examples) = super::llm::prompt(glossary, app);
        let out = match self.provider {
            Provider::Anthropic => self.anthropic(&system, &examples, text),
            Provider::Gemini => self.gemini(&system, &examples, text),
            Provider::Groq => self.groq(&system, &examples, text),
        }
        .with_context(|| format!("{} ({})", self.provider.name(), self.model))?;
        Ok(out.trim().trim_matches('"').trim().to_string())
    }

    fn post(&self, request: reqwest::blocking::RequestBuilder, body: Value) -> Result<Value> {
        let resp = request.header(reqwest::header::CONTENT_TYPE, "application/json").body(body.to_string()).send()?;
        let status = resp.status();
        let text = resp.text()?;
        if !status.is_success() {
            // The provider's own message says what's wrong (bad key, unknown
            // model, quota); keep it short.
            let detail = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
                .unwrap_or(text);
            bail!("{status}: {}", detail.chars().take(200).collect::<String>());
        }
        Ok(serde_json::from_str(&text)?)
    }

    /// Claude Messages API (raw HTTP: there's no official Rust SDK).
    fn anthropic(&self, system: &str, examples: &[(&str, &str)], text: &str) -> Result<String> {
        let mut messages = Vec::new();
        for (input, output) in examples {
            messages.push(json!({ "role": "user", "content": input }));
            messages.push(json!({ "role": "assistant", "content": output }));
        }
        messages.push(json!({ "role": "user", "content": text }));
        let mut body = json!({
            "model": self.model,
            "max_tokens": 8000,
            "system": system,
            "messages": messages,
            // Light clean-up, not a hard problem: keep thinking (and latency) low.
            "output_config": { "effort": "low" },
        });
        let mut request = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.key)
            .header("anthropic-version", "2023-06-01");
        if CLAUDE_FALLBACK_MODELS.contains(&self.model.as_str()) {
            // A declined request is retried on Anthropic's recommended model.
            body["fallbacks"] = json!("default");
            request = request.header("anthropic-beta", "server-side-fallback-2026-07-01");
        }
        let resp = self.post(request, body)?;
        if resp["stop_reason"] == "refusal" {
            bail!("the model declined to clean this up");
        }
        let text: String = resp["content"]
            .as_array()
            .ok_or_else(|| anyhow!("unexpected reply"))?
            .iter()
            .filter(|block| block["type"] == "text")
            .filter_map(|block| block["text"].as_str())
            .collect();
        Ok(text)
    }

    /// Gemini generateContent.
    fn gemini(&self, system: &str, examples: &[(&str, &str)], text: &str) -> Result<String> {
        let mut contents = Vec::new();
        for (input, output) in examples {
            contents.push(json!({ "role": "user", "parts": [{ "text": input }] }));
            contents.push(json!({ "role": "model", "parts": [{ "text": output }] }));
        }
        contents.push(json!({ "role": "user", "parts": [{ "text": text }] }));
        let body = json!({
            "system_instruction": { "parts": [{ "text": system }] },
            "contents": contents,
            "generationConfig": { "maxOutputTokens": 8000 },
        });
        let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent", self.model);
        let resp = self.post(self.client.post(url).header("x-goog-api-key", &self.key), body)?;
        let parts = resp["candidates"][0]["content"]["parts"].as_array().ok_or_else(|| {
            anyhow!("no text in the reply ({})", resp["candidates"][0]["finishReason"].as_str().unwrap_or("blocked"))
        })?;
        // Skip thought summaries, if any.
        Ok(parts.iter().filter(|p| p["thought"] != true).filter_map(|p| p["text"].as_str()).collect())
    }

    /// Groq's OpenAI-compatible chat completions.
    fn groq(&self, system: &str, examples: &[(&str, &str)], text: &str) -> Result<String> {
        let mut messages = vec![json!({ "role": "system", "content": system })];
        for (input, output) in examples {
            messages.push(json!({ "role": "user", "content": input }));
            messages.push(json!({ "role": "assistant", "content": output }));
        }
        messages.push(json!({ "role": "user", "content": text }));
        let mut body = json!({ "model": self.model, "messages": messages, "temperature": 0, "max_tokens": 8000 });
        if self.model.starts_with("openai/gpt-oss") {
            // Reasoning models: a quick look is plenty for a clean-up.
            body["reasoning_effort"] = json!("low");
        }
        let request = self.client.post("https://api.groq.com/openai/v1/chat/completions").bearer_auth(&self.key);
        let resp = self.post(request, body)?;
        resp["choices"][0]["message"]["content"].as_str().map(str::to_string).ok_or_else(|| anyhow!("unexpected reply"))
    }
}
