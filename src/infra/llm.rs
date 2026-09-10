//! Claude adapter (Anthropic Messages API). Sonnet for reasoning agents, Haiku for
//! cheap classification later. Verify model ids before the first live call.
#![allow(dead_code)] // Phase 0: fields wired but unused until Phase 3.

use std::time::Duration;

use async_trait::async_trait;

use crate::domain::error::LlmError;
use crate::traits::LlmClient;

pub const SONNET: &str = "claude-sonnet-5";
pub const HAIKU: &str = "claude-haiku-4-5-20251001";

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

#[async_trait]
impl LlmClient for Claude {
    async fn message(&self, _system: &str, _user: &str) -> Result<String, LlmError> {
        todo!("Phase 3.2: POST /v1/messages (x-api-key, anthropic-version); parse content[0].text")
    }
}
