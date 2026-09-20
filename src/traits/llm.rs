use async_trait::async_trait;
use crate::domain::error::LlmError;

/// LLM chat. impl #1 = Claude (Anthropic Messages API).
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn message(&self, system: &str, user: &str) -> Result<String, LlmError>;
}


#[async_trait]
impl<T: LlmClient + ?Sized> LlmClient for &T {
    async fn message(&self, system: &str, user: &str) -> Result<String, LlmError> {
        (**self).message(system, user).await
    }
}
