//! Claude client over the Anthropic Messages API (a single JSON POST — no SDK
//! crate needed). Agents are prompt-in / text-out; deterministic math is done in
//! Rust (`alpaca_core`) and handed to the model as numbers, so it never computes.

/// Model ids (see environment note). Sonnet for reasoning agents, Haiku for
/// cheap classification.
pub const SONNET: &str = "claude-sonnet-5";
pub const HAIKU: &str = "claude-haiku-4-5-20251001";

pub struct Claude {
    http: reqwest::Client,
    api_key: String,
    model: &'static str,
}

impl Claude {
    pub fn new(api_key: String, model: &'static str) -> Self {
        Self { http: reqwest::Client::new(), api_key, model }
    }

    /// One turn: system prompt + user message -> assistant text.
    pub async fn message(&self, system: &str, user: &str) -> anyhow::Result<String> {
        todo!("POST api.anthropic.com/v1/messages {{model, system, messages:[{{role:user,content:user}}]}}; parse content[0].text")
    }
}
