//! `AlpacaClient` — transport for Alpaca's two hosts: trading (`paper-api.`) and market
//! data (`data.`). Holds the credentials and the HTTP pool; returns Alpaca's wire types.
//! Which port an endpoint serves is the adapters' business (`market.rs`, `broker.rs`).

use std::time::Duration;

use serde::Deserialize;

const KEY_HEADER: &str = "APCA-API-KEY-ID";
const SECRET_HEADER: &str = "APCA-API-SECRET-KEY";

pub struct AlpacaClient {
    http: reqwest::Client,
    key: String,
    secret: String,
    trade_base: String,
    data_base: String,
}

impl AlpacaClient {
    pub fn new(key: String, secret: String, trade_base: String, data_base: String) -> Self {
        // Timeout set up front — a hung request is the WAL hole (a hang, not a crash,
        // leaves an order in limbo).
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("reqwest client");
        Self {
            http,
            key,
            secret,
            // Paths below append `/v2/...` themselves, so strip a trailing `/` and any
            // `/v2` the user left on the base (tolerate `.../markets` and `.../markets/v2`).
            trade_base: normalize_trade_base(&trade_base),
            data_base: data_base.trim_end_matches('/').to_string(),
        }
    }

    /// Full URL on the market-data host, e.g. `data_url("/v2/stocks/bars")`.
    pub(crate) fn data_url(&self, path: &str) -> String {
        format!("{}{path}", self.data_base)
    }

    /// Full URL on the trading host, e.g. `trade_url("/v2/orders")`.
    pub(crate) fn trade_url(&self, path: &str) -> String {
        format!("{}{path}", self.trade_base)
    }

    /// Authenticated GET builder; the adapter adds query params and sends.
    pub(crate) fn get(&self, url: &str) -> reqwest::RequestBuilder {
        self.auth(self.http.get(url))
    }

    /// Authenticated POST builder; the adapter adds the JSON body and sends.
    pub(crate) fn post(&self, url: &str) -> reqwest::RequestBuilder {
        self.auth(self.http.post(url))
    }

    fn auth(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        rb.header(KEY_HEADER, &self.key)
            .header(SECRET_HEADER, &self.secret)
    }
}

/// Strip a trailing `/` and a trailing `/v2` from the trading base, so the request
/// paths (which add `/v2/...`) don't produce `/v2/v2/...` when a user's base already
/// ends in `/v2`.
fn normalize_trade_base(base: &str) -> String {
    let b = base.trim_end_matches('/');
    b.strip_suffix("/v2").unwrap_or(b).to_string()
}

// ---- wire types -------------------------------------------------------------------

/// Bars response, same shape for crypto and stocks: `{ "bars": { "BTC/USD": [ {...} ] } }`.
#[derive(Deserialize)]
pub(crate) struct BarsResponse {
    #[serde(default)]
    pub(crate) bars: std::collections::HashMap<String, Vec<RawBar>>,
}

#[derive(Deserialize)]
pub(crate) struct RawBar {
    pub(crate) t: String, // RFC3339
    pub(crate) o: f64,
    pub(crate) h: f64,
    pub(crate) l: f64,
    pub(crate) c: f64,
    pub(crate) v: f64,
}

/// Order/account JSON as echoed by Alpaca. Numeric money fields arrive as strings.
#[derive(Deserialize)]
pub(crate) struct RawOrder {
    pub(crate) id: String,
    pub(crate) client_order_id: String,
    pub(crate) symbol: String,
    pub(crate) status: String,
}

#[derive(Deserialize)]
pub(crate) struct RawAccount {
    pub(crate) buying_power: String,
}

#[derive(Deserialize)]
pub(crate) struct RawPosition {
    pub(crate) symbol: String,
    pub(crate) qty: String,
    pub(crate) market_value: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trade_base_never_doubles_v2() {
        let want = "https://paper-api.alpaca.markets";
        for base in [
            "https://paper-api.alpaca.markets",
            "https://paper-api.alpaca.markets/",
            "https://paper-api.alpaca.markets/v2",
            "https://paper-api.alpaca.markets/v2/",
        ] {
            assert_eq!(normalize_trade_base(base), want, "{base}");
        }
    }
}
