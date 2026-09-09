//! Alpaca paper client: market data (bars, quotes, news) + trading (orders,
//! positions). Thin wrapper over the REST API with `reqwest`.

use serde::{Deserialize, Serialize};

/// One daily OHLCV bar.
#[derive(Debug, Clone, Deserialize)]
pub struct Bar {
    #[serde(rename = "t")]
    pub timestamp: String,
    #[serde(rename = "o")]
    pub open: f64,
    #[serde(rename = "h")]
    pub high: f64,
    #[serde(rename = "l")]
    pub low: f64,
    #[serde(rename = "c")]
    pub close: f64,
    #[serde(rename = "v")]
    pub volume: f64,
}

/// Latest quote (best bid/ask).
#[derive(Debug, Clone, Deserialize)]
pub struct Quote {
    pub bid: f64,
    pub ask: f64,
}

/// A news headline used by the (later) sentiment agent.
#[derive(Debug, Clone, Deserialize)]
pub struct NewsItem {
    pub headline: String,
    pub summary: String,
    pub created_at: String,
}

/// Order side.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Buy,
    Sell,
}

/// A market-order request (paper). Keep it minimal for v1.
#[derive(Debug, Clone, Serialize)]
pub struct OrderRequest {
    pub symbol: String,
    pub qty: f64,
    pub side: Side,
    #[serde(rename = "type")]
    pub order_type: &'static str, // "market"
    pub time_in_force: &'static str, // "day"
}

/// The order Alpaca echoes back.
#[derive(Debug, Clone, Deserialize)]
pub struct Order {
    pub id: String,
    pub symbol: String,
    pub status: String,
}

/// Current position in a symbol.
#[derive(Debug, Clone, Deserialize)]
pub struct Position {
    pub symbol: String,
    pub qty: f64,
    pub market_value: f64,
}

/// Alpaca REST client. `base` is the trading base (paper by default); market
/// data uses the data host regardless.
pub struct Alpaca {
    http: reqwest::Client,
    key: String,
    secret: String,
    base: String,
}

impl Alpaca {
    pub fn new(key: String, secret: String, base: String) -> Self {
        Self { http: reqwest::Client::new(), key, secret, base }
    }

    /// Daily bars, most recent `limit`, oldest-first. Returns closes-ready bars.
    pub async fn daily_bars(&self, symbol: &str, limit: usize) -> anyhow::Result<Vec<Bar>> {
        todo!("GET data.alpaca.markets/v2/stocks/{symbol}/bars?timeframe=1Day&limit={limit}")
    }

    /// Latest quote.
    pub async fn latest_quote(&self, symbol: &str) -> anyhow::Result<Quote> {
        todo!("GET data.alpaca.markets/v2/stocks/{symbol}/quotes/latest")
    }

    /// Recent news for a symbol (for the later sentiment agent).
    pub async fn news(&self, symbol: &str, limit: usize) -> anyhow::Result<Vec<NewsItem>> {
        todo!("GET data.alpaca.markets/v1beta1/news?symbols={symbol}&limit={limit}")
    }

    /// Submit a market order (paper).
    pub async fn submit_order(&self, req: &OrderRequest) -> anyhow::Result<Order> {
        todo!("POST {{base}}/v2/orders with req")
    }

    /// Current positions.
    pub async fn positions(&self) -> anyhow::Result<Vec<Position>> {
        todo!("GET {{base}}/v2/positions")
    }
}
