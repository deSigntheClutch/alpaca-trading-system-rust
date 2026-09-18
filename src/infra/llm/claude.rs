//! `Claude` — Anthropic Messages API direct (needs an Anthropic API key).

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::domain::error::LlmError;
use crate::traits::LlmClient;

use super::MAX_TOKENS;

pub const SONNET: &str = "claude-sonnet-5";

const API_BASE: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";

pub struct Claude {
    http: reqwest::Client,
    api_key: String,
    model: String,
}

impl Claude {
    pub fn new(api_key: String, model: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("reqwest client");
        Self {
            http,
            api_key,
            model: model.to_string(),
        }
    }
}

/// `{ "content": [ { "type": "text", "text": "..." } ] }`
#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(default)]
    text: Option<String>,
}

#[async_trait]
impl LlmClient for Claude {
    async fn message(&self, system: &str, user: &str) -> Result<String, LlmError> {
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "system": system,
            "messages": [{ "role": "user", "content": user }],
        });
        // The exact wire payload (prompt included). API key is a header, never here, so
        // this is safe to log. See with RUST_LOG=alpaca_trading_system::infra::llm=debug.
        tracing::debug!(model = %self.model, request = %body, "anthropic request");
        let resp = self
            .http
            .post(format!("{API_BASE}/v1/messages"))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Request(e.to_string()))?;

        if !resp.status().is_success() {
            let code = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(LlmError::Request(format!("{code}: {text}")));
        }

        let parsed: MessagesResponse = resp
            .json()
            .await
            .map_err(|e| LlmError::BadResponse(e.to_string()))?;

        parsed
            .content
            .into_iter()
            .find_map(|b| b.text)
            .ok_or_else(|| LlmError::BadResponse("no text block in response".into()))
    }
}
