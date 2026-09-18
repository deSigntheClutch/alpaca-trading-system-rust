//! Alpaca exchange adapters:
//!   - `AlpacaRest` (`rest.rs`) — historical crypto bars (`BarSource`) + orders/account
//!     (`Broker`).
//!   - `CryptoWs`   (`ws.rs`)   — the 24/7 crypto market-data websocket (`MarketStream`).
//!
//! Split by transport because they differ structurally: market data lives on a different
//! host than trading (`data.` vs `paper-api.`), and the WS auths in-band (a message, not
//! a header). The one thing they share — RFC3339 timestamp parsing — lives here.

mod rest;
mod ws;

pub use rest::AlpacaRest;
pub use ws::CryptoWs;

/// RFC3339 (`2024-01-02T03:04:05.678Z`) → unix epoch millis. Used by both adapters
/// (bar timestamps and trade-tick timestamps arrive in the same format).
pub(crate) fn parse_rfc3339_millis(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}
