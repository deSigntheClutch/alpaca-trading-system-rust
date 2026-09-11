//! LLM adapters, both implementing `LlmClient`:
//! - `Claude`  — Anthropic Messages API direct (needs an Anthropic API key).
//! - `Bedrock` — Amazon Bedrock Converse API with a Bedrock API key (bearer token).
//!   Same-shaped HTTP call, just a different URL/auth/body — no AWS SDK, no SigV4.
//!   This is the Amazon-internal path.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::domain::error::LlmError;
use crate::traits::LlmClient;

pub const SONNET: &str = "claude-sonnet-5";
pub const HAIKU: &str = "claude-haiku-4-5-20251001";

/// Bedrock inference-profile IDs (region us-east-1). Use the `us.`-prefixed *profile*
/// ID, not the bare model ID — newer Claude models reject on-demand invocation by bare
/// ID ("...isn't supported. Retry with an inference profile"). Override via `BEDROCK_MODEL`.
pub const BEDROCK_HAIKU: &str = "us.anthropic.claude-haiku-4-5-20251001-v1:0";
pub const BEDROCK_SONNET: &str = "us.anthropic.claude-sonnet-4-5-20250929-v1:0";

const API_BASE: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";
const MAX_TOKENS: u32 = 1024;

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

/// Amazon Bedrock via the Converse API + a Bedrock API key (bearer token). URL encodes
/// the region and model ID; auth is a single `Authorization: Bearer` header.
pub struct Bedrock {
    http: reqwest::Client,
    api_key: String,
    region: String,
    model: String,
}

impl Bedrock {
    pub fn new(api_key: String, region: String, model: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("reqwest client");
        Self {
            http,
            api_key,
            region,
            model,
        }
    }
}

/// Converse response: `{ "output": { "message": { "content": [ { "text": "..." } ] } } }`.
#[derive(Deserialize)]
struct ConverseResponse {
    output: ConverseOutput,
}
#[derive(Deserialize)]
struct ConverseOutput {
    message: ConverseMessage,
}
#[derive(Deserialize)]
struct ConverseMessage {
    content: Vec<ConverseBlock>,
}
#[derive(Deserialize)]
struct ConverseBlock {
    #[serde(default)]
    text: Option<String>,
}

#[async_trait]
impl LlmClient for Bedrock {
    async fn message(&self, system: &str, user: &str) -> Result<String, LlmError> {
        // model ID stays literal in the path (colons are valid path chars); only the
        // region varies structurally.
        let url = format!(
            "https://bedrock-runtime.{}.amazonaws.com/model/{}/converse",
            self.region, self.model
        );
        let body = serde_json::json!({
            "system":   [{ "text": system }],
            "messages": [{ "role": "user", "content": [{ "text": user }] }],
            "inferenceConfig": { "maxTokens": MAX_TOKENS },
        });
        let resp = self
            .http
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Request(e.to_string()))?;

        if !resp.status().is_success() {
            let code = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(LlmError::Request(format!("{code}: {text}")));
        }

        let parsed: ConverseResponse = resp
            .json()
            .await
            .map_err(|e| LlmError::BadResponse(e.to_string()))?;

        parsed
            .output
            .message
            .content
            .into_iter()
            .find_map(|b| b.text)
            .ok_or_else(|| LlmError::BadResponse("no text block in Converse output".into()))
    }
}
