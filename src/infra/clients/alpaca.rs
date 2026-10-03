//! `AlpacaClient` — transport for Alpaca's two hosts: trading (`paper-api.`) and market
//! data (`data.`). Holds the credentials and the HTTP pool; returns Alpaca's wire types.
//! Which port an endpoint serves is the adapters' business (`market.rs`, `broker.rs`,
//! `news.rs`); they say which `Host` and which path, nothing else.

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Deserialize;

use super::http::{send_json, HttpError};

const KEY_HEADER: &str = "APCA-API-KEY-ID";
const SECRET_HEADER: &str = "APCA-API-SECRET-KEY";

/// Alpaca splits its API across two hosts; market data and trading never share one.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Host {
    /// `data.alpaca.markets`: bars, news.
    Data,
    /// `paper-api.alpaca.markets` (or live): account, positions, orders.
    Trade,
}

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

    /// Authenticated GET, decoded as `T`. Errors name the path.
    pub(crate) async fn get_json<T: DeserializeOwned>(
        &self,
        host: Host,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<T, HttpError> {
        let req = self.auth(self.http.get(self.url(host, path))).query(params);
        send_json(path, req).await
    }

    /// Authenticated POST with a JSON body, decoded as `T`.
    pub(crate) async fn post_json<T: DeserializeOwned>(
        &self,
        host: Host,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, HttpError> {
        let req = self.auth(self.http.post(self.url(host, path))).json(body);
        send_json(path, req).await
    }

    fn url(&self, host: Host, path: &str) -> String {
        match host {
            Host::Data => format!("{}{path}", self.data_base),
            Host::Trade => format!("{}{path}", self.trade_base),
        }
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

/// `GET /v1beta1/news`: `{ "news": [ {...} ], "next_page_token": "..." | null }`.
#[derive(Deserialize)]
pub(crate) struct NewsResponse {
    #[serde(default)]
    pub(crate) news: Vec<RawNews>,
    #[serde(default)]
    pub(crate) next_page_token: Option<String>,
}

/// One news item. `content`, `summary`, `images`, `author` are on the wire too and
/// deliberately not read: the report wants headlines, and every extra field is more
/// untrusted text heading for a prompt.
#[derive(Deserialize)]
pub(crate) struct RawNews {
    pub(crate) id: u64,
    pub(crate) headline: String,
    #[serde(default)]
    pub(crate) source: String,
    #[serde(default)]
    pub(crate) url: Option<String>,
    pub(crate) created_at: String, // RFC3339
    #[serde(default)]
    pub(crate) symbols: Vec<String>,
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
