//! Vendor API clients: transport only. Each one knows how to authenticate and talk to
//! a single API and returns that API's wire types; the port adapters in `infra/*.rs`
//! turn those into domain types. One client is shared (`Arc`) by every adapter built on
//! the same vendor, so one HTTP pool and one credential per vendor.

pub(crate) mod alpaca;
pub(crate) mod anthropic;
pub(crate) mod bedrock;
pub(crate) mod finnhub;

pub use alpaca::AlpacaClient;
pub use anthropic::AnthropicClient;
pub use bedrock::BedrockClient;
pub use finnhub::FinnhubClient;

/// RFC3339 → epoch millis, shared by every vendor that sends ISO timestamps.
pub(crate) fn parse_rfc3339_millis(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}
