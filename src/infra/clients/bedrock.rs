//! `BedrockClient` — transport for Amazon Bedrock's Converse API with a Bedrock API
//! key (bearer token). No AWS SDK, no SigV4. This is the Amazon-internal path.

use std::time::Duration;

use serde::Deserialize;

use crate::domain::error::LlmError;

pub struct BedrockClient {
    http: reqwest::Client,
    api_key: String,
    region: String,
}

impl BedrockClient {
    pub fn new(api_key: String, region: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("reqwest client");
        Self {
            http,
            api_key,
            region,
        }
    }

    /// `POST /model/{model}/converse`. The model ID stays literal in the path (colons
    /// are valid path chars); only the region varies structurally.
    pub(crate) async fn converse(
        &self,
        model: &str,
        body: &serde_json::Value,
    ) -> Result<ConverseResponse, LlmError> {
        let url = format!(
            "https://bedrock-runtime.{}.amazonaws.com/model/{model}/converse",
            self.region
        );
        tracing::debug!(%model, %url, request = %body, "bedrock converse request");
        let resp = self
            .http
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
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

/// Converse response: `{ "output": { "message": { "content": [ { "text": "..." } ] } } }`.
#[derive(Deserialize)]
pub(crate) struct ConverseResponse {
    pub(crate) output: ConverseOutput,
}
#[derive(Deserialize)]
pub(crate) struct ConverseOutput {
    pub(crate) message: ConverseMessage,
}
#[derive(Deserialize)]
pub(crate) struct ConverseMessage {
    pub(crate) content: Vec<ConverseBlock>,
}
#[derive(Deserialize)]
pub(crate) struct ConverseBlock {
    #[serde(default)]
    pub(crate) text: Option<String>,
}
