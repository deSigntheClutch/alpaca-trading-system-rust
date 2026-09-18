//! LLM adapters, both implementing `LlmClient`:
//!   - `Claude`  (`claude.rs`)  — Anthropic Messages API direct (needs an Anthropic key).
//!   - `Bedrock` (`bedrock.rs`) — Amazon Bedrock Converse API with a Bedrock API key
//!     (bearer token). Same-shaped HTTP call, just a different URL/auth/body — no AWS SDK,
//!     no SigV4. This is the Amazon-internal path.

mod bedrock;
mod claude;

pub use bedrock::{Bedrock, BEDROCK_SONNET};
pub use claude::{Claude, SONNET};

/// Max response tokens — same cap for both adapters.
pub(crate) const MAX_TOKENS: u32 = 1024;
