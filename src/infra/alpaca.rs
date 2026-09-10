//! Alpaca adapters: `AlpacaRest` (trading + historical bars) and `CryptoWs` (the
//! 24/7 crypto market-data websocket, impl #1 of `MarketStream`).
#![allow(dead_code)] // Phase 0: fields wired but unused until their phase lands.

use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::domain::error::MarketError;
use crate::traits::{BarSource, MarketStream};
use crate::domain::types::{Bar, Symbol, Tick};

/// Alpaca REST client (orders, positions, historical bars). Paper by default.
pub struct AlpacaRest {
    http: reqwest::Client,
    key: String,
    secret: String,
    base: String,
}

impl AlpacaRest {
    pub fn new(key: String, secret: String, base: String) -> Self {
        // Timeout set up front — a hung request is the WAL hole (a hang, not a crash,
        // leaves an order in limbo). Real value tuned in Phase 3.
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client");
        Self {
            http,
            key,
            secret,
            base,
        }
    }
}

#[async_trait]
impl BarSource for AlpacaRest {
    async fn daily_bars(&self, _symbol: &Symbol, _limit: usize) -> Result<Vec<Bar>, MarketError> {
        todo!("stock milestone: GET /v2/stocks/bars?symbols=... (batch), deserialize -> Bar")
    }
}

/// Alpaca crypto market-data websocket. Free tier, one connection, 24/7 — the
/// single-owner-task + fan-out home for the tokio surface.
pub struct CryptoWs {
    key: String,
    secret: String,
    ws_url: String,
}

impl CryptoWs {
    pub fn new(key: String, secret: String, ws_url: String) -> Self {
        Self {
            key,
            secret,
            ws_url,
        }
    }
}

#[async_trait]
impl MarketStream for CryptoWs {
    async fn subscribe(
        &self,
        _symbols: &[Symbol],
    ) -> Result<BoxStream<'static, Result<Tick, MarketError>>, MarketError> {
        todo!("Phase 2.1: tokio-tungstenite connect + auth + subscribe; frames -> Tick stream")
    }
}
