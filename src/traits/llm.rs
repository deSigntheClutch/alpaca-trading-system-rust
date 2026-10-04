use async_trait::async_trait;

use crate::domain::error::LlmError;

/// What a structured call returns besides the text: enough to log cost and to tell a
/// clean answer from a truncated or refused one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LlmReply {
    pub text: String,
    /// Vendor's stop reason when it reports one: `end_turn`, `max_tokens`, `refusal`.
    pub stop_reason: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl LlmReply {
    pub fn refused(&self) -> bool {
        self.stop_reason.as_deref() == Some("refusal")
    }
    pub fn truncated(&self) -> bool {
        self.stop_reason.as_deref() == Some("max_tokens")
    }
}

/// A chat model. impl #1 = Anthropic Messages API, #2 = Bedrock Converse.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Free text in, free text out.
    async fn message(&self, system: &str, user: &str) -> Result<String, LlmError>;

    /// Ask for a JSON object matching `schema`. The default has no schema enforcement:
    /// it appends the schema to the prompt, calls `message` and reports no usage —
    /// enough for a fake or a vendor without structured outputs. Vendors that have
    /// them (Anthropic) override it so the reply is schema-valid by construction.
    async fn message_json(
        &self,
        system: &str,
        user: &str,
        schema: &serde_json::Value,
    ) -> Result<LlmReply, LlmError> {
        let user = format!(
            "{user}\n\nReply with one JSON object matching this JSON Schema and nothing else:\n{schema}"
        );
        let text = self.message(system, &user).await?;
        Ok(LlmReply {
            text,
            ..Default::default()
        })
    }

    /// Model identifier for the record (the report stores it next to each note).
    fn model_id(&self) -> &str {
        "unknown"
    }
}

#[async_trait]
impl<T: LlmClient + ?Sized> LlmClient for &T {
    async fn message(&self, system: &str, user: &str) -> Result<String, LlmError> {
        (**self).message(system, user).await
    }

    async fn message_json(
        &self,
        system: &str,
        user: &str,
        schema: &serde_json::Value,
    ) -> Result<LlmReply, LlmError> {
        (**self).message_json(system, user, schema).await
    }

    fn model_id(&self) -> &str {
        (**self).model_id()
    }
}
