//! `AnthropicClient` — transport for the Anthropic Messages API. Holds the key and the
//! HTTP pool; takes a request body and returns the wire response. Prompting, model
//! choice and parsing live in the `LlmClient` adapter (`llm.rs`).

use std::time::Duration;

use serde::Deserialize;

use crate::domain::error::LlmError;

const API_BASE: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";

pub struct AnthropicClient {
    http: reqwest::Client,
    api_key: String,
}

impl AnthropicClient {
    pub fn new(api_key: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("reqwest client");
        Self { http, api_key }
    }

    /// `POST /v1/messages` with a caller-built body (model, max_tokens, system,
    /// messages, and later `output_config`). The key is a header, never in the body, so
    /// the body is safe to log at debug.
    pub(crate) async fn messages(
        &self,
        body: &serde_json::Value,
    ) -> Result<MessagesResponse, LlmError> {
        tracing::debug!(request = %body, "anthropic request");
        let resp = self
            .http
            .post(format!("{API_BASE}/v1/messages"))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .json(body)
            .send()
            .await
            .map_err(|e| LlmError::Request(e.to_string()))?;

        if !resp.status().is_success() {
            let code = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(LlmError::Request(format!("{code}: {text}")));
        }
        resp.json()
            .await
            .map_err(|e| LlmError::BadResponse(e.to_string()))
    }
}

/// `{ "content": [ { "type": "text", "text": "..." } ], "stop_reason": "end_turn",
///    "usage": { "input_tokens": n, "output_tokens": n } }`
#[derive(Deserialize)]
pub(crate) struct MessagesResponse {
    #[serde(default)]
    pub(crate) content: Vec<ContentBlock>,
    #[serde(default)]
    pub(crate) stop_reason: Option<String>,
    #[serde(default)]
    pub(crate) usage: Usage,
}

#[derive(Deserialize)]
pub(crate) struct ContentBlock {
    #[serde(default)]
    pub(crate) text: Option<String>,
}

#[derive(Deserialize, Default, Clone, Copy)]
pub(crate) struct Usage {
    #[serde(default)]
    pub(crate) input_tokens: u64,
    #[serde(default)]
    pub(crate) output_tokens: u64,
}
