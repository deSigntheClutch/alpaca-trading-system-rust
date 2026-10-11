//! `LlmClient` adapters: `Claude` over `AnthropicClient`, `Bedrock` over `BedrockClient`.
//! Both turn `(system, user)` into the vendor's request body and the vendor's response
//! into one text string. Model IDs and the shared token cap live here, not in the clients.

use std::sync::Arc;

use async_trait::async_trait;

use crate::domain::error::LlmError;
use crate::traits::{LlmClient, LlmReply};

use super::clients::{AnthropicClient, BedrockClient};

pub const SONNET: &str = "claude-sonnet-5";

/// Bedrock inference-profile ID (region us-east-1). Use the `us.`-prefixed *profile*
/// ID, not the bare model ID — newer Claude models reject on-demand invocation by bare
/// ID ("...isn't supported. Retry with an inference profile"). Override via `BEDROCK_MODEL`.
pub const BEDROCK_SONNET: &str = "us.anthropic.claude-sonnet-4-5-20250929-v1:0";

/// Max response tokens — same cap for both adapters.
pub(crate) const MAX_TOKENS: u32 = 1024;

/// List price in USD per million tokens, (input, output), for the cost line in the
/// report. A table because the API does not return prices; `None` for a model we do
/// not know, in which case the report shows tokens and no dollars. Update when the
/// price list does.
pub fn price_per_mtok(model: &str) -> Option<(f64, f64)> {
    match model {
        "claude-opus-5" | "claude-opus-4-8" | "claude-opus-4-7" | "claude-opus-4-6" => {
            Some((5.0, 25.0))
        }
        "claude-sonnet-5" => Some((2.0, 10.0)),
        "claude-sonnet-4-6" => Some((3.0, 15.0)),
        "claude-haiku-4-5" => Some((1.0, 5.0)),
        m if m.contains("claude-sonnet-4-5") => Some((3.0, 15.0)),
        _ => None,
    }
}

pub fn cost_usd(model: &str, input_tokens: u64, output_tokens: u64) -> Option<f64> {
    let (i, o) = price_per_mtok(model)?;
    Some(input_tokens as f64 * i / 1e6 + output_tokens as f64 * o / 1e6)
}

/// The null adapter: the rule arm never calls a model, and anything that does call
/// one through this gets a clear error instead of a quiet fake reply. It is also how
/// the read-only dashboard can run a rule backtest without holding an API key.
pub struct NoLlm;

#[async_trait]
impl LlmClient for NoLlm {
    async fn message(&self, _system: &str, _user: &str) -> Result<String, LlmError> {
        Err(LlmError::Request(
            "no LLM configured: this arm must not call a model".into(),
        ))
    }
}

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

    /// Structured outputs: `output_config.format` with the JSON Schema, no beta header.
    /// The reply text is schema-valid JSON unless `stop_reason` says `refusal` or
    /// `max_tokens`, which is why both are returned instead of being swallowed here.
    async fn message_json(
        &self,
        system: &str,
        user: &str,
        schema: &serde_json::Value,
    ) -> Result<LlmReply, LlmError> {
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "system": system,
            "messages": [{ "role": "user", "content": user }],
            "output_config": { "format": { "type": "json_schema", "schema": schema } },
        });
        let parsed = self.client.messages(&body).await?;
        Ok(LlmReply {
            text: parsed
                .content
                .into_iter()
                .find_map(|b| b.text)
                .unwrap_or_default(),
            stop_reason: parsed.stop_reason,
            input_tokens: parsed.usage.input_tokens,
            output_tokens: parsed.usage.output_tokens,
        })
    }

    fn model_id(&self) -> &str {
        &self.model
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

    fn model_id(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod price_tests {
    use super::*;

    #[test]
    fn known_models_price_unknown_ones_do_not() {
        assert_eq!(cost_usd("claude-sonnet-5", 1_000_000, 100_000), Some(3.0));
        assert_eq!(cost_usd("claude-opus-5", 2_000_000, 0), Some(10.0));
        assert_eq!(cost_usd("gpt-9", 1, 1), None);
    }
}
