//! `LlmClient` adapters: `Claude` over `AnthropicClient`, `Bedrock` over `BedrockClient`.
//! Both turn `(system, user)` into the vendor's request body and the vendor's response
//! into one text string. Model IDs and the shared token cap live here, not in the clients.

use std::sync::Arc;

use async_trait::async_trait;

use crate::domain::error::LlmError;
use crate::traits::LlmClient;

use super::clients::{AnthropicClient, BedrockClient};

pub const SONNET: &str = "claude-sonnet-5";

/// Bedrock inference-profile ID (region us-east-1). Use the `us.`-prefixed *profile*
/// ID, not the bare model ID — newer Claude models reject on-demand invocation by bare
/// ID ("...isn't supported. Retry with an inference profile"). Override via `BEDROCK_MODEL`.
pub const BEDROCK_SONNET: &str = "us.anthropic.claude-sonnet-4-5-20250929-v1:0";

/// Max response tokens — same cap for both adapters.
pub(crate) const MAX_TOKENS: u32 = 1024;

/// Anthropic Messages API direct.
pub struct Claude {
    client: Arc<AnthropicClient>,
    model: String,
}

impl Claude {
    pub fn new(client: Arc<AnthropicClient>, model: &str) -> Self {
        Self {
            client,
            model: model.to_string(),
        }
    }
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
        let parsed = self.client.messages(&body).await?;
        parsed
            .content
            .into_iter()
            .find_map(|b| b.text)
            .ok_or_else(|| LlmError::BadResponse("no text block in response".into()))
    }
}

/// Amazon Bedrock via the Converse API.
pub struct Bedrock {
    client: Arc<BedrockClient>,
    model: String,
}

impl Bedrock {
    pub fn new(client: Arc<BedrockClient>, model: String) -> Self {
        Self { client, model }
    }
}

#[async_trait]
impl LlmClient for Bedrock {
    async fn message(&self, system: &str, user: &str) -> Result<String, LlmError> {
        let body = serde_json::json!({
            "system":   [{ "text": system }],
            "messages": [{ "role": "user", "content": [{ "text": user }] }],
            "inferenceConfig": { "maxTokens": MAX_TOKENS },
        });
        let parsed = self.client.converse(&self.model, &body).await?;
        parsed
            .output
            .message
            .content
            .into_iter()
            .find_map(|b| b.text)
            .ok_or_else(|| LlmError::BadResponse("no text block in Converse output".into()))
    }
}
