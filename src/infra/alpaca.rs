//! Alpaca adapters:
//!   - `AlpacaRest`  — historical crypto bars (`BarSource`) + orders/account (`Broker`).
//!   - `CryptoWs`    — the 24/7 crypto market-data websocket (`MarketStream`).
//!
//! Market data lives on a different host than trading (`data.` vs `paper-api.`), so
//! `AlpacaRest` holds both bases. The WS auths in-band (a message, not a header).

use std::collections::VecDeque;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use futures::SinkExt;
use rust_decimal::Decimal;
use serde::Deserialize;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::domain::error::{BrokerError, MarketError};
use crate::domain::types::{Bar, Order, OrderRequest, Position, Symbol, Tick};
use crate::traits::{BarSource, Broker, MarketStream};

const KEY_HEADER: &str = "APCA-API-KEY-ID";
const SECRET_HEADER: &str = "APCA-API-SECRET-KEY";

// ---------------------------------------------------------------------------
// REST: historical bars + orders + account
// ---------------------------------------------------------------------------

/// Alpaca REST client. `trade_base` = paper-api (orders/account), `data_base` =
/// data.alpaca.markets (historical bars). Paper by default (enforced in config).
pub struct AlpacaRest {
    http: reqwest::Client,
    key: String,
    secret: String,
    trade_base: String,
    data_base: String,
}

impl AlpacaRest {
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

/// Alpaca crypto bars response: `{ "bars": { "BTC/USD": [ {...} ] } }`.
#[derive(Deserialize)]
struct BarsResponse {
    #[serde(default)]
    bars: std::collections::HashMap<String, Vec<RawBar>>,
}

#[derive(Deserialize)]
struct RawBar {
    t: String, // RFC3339
    o: f64,
    h: f64,
    l: f64,
    c: f64,
    v: f64,
}

#[async_trait]
impl BarSource for AlpacaRest {
    /// Most-recent `limit` daily bars, returned oldest→newest (what `signals::compute`
    /// wants). Same crypto endpoint works for `"BTC/USD"` and future stock symbols
    /// (via a different path — that's the stock milestone).
    async fn daily_bars(&self, symbol: &Symbol, limit: usize) -> Result<Vec<Bar>, MarketError> {
        let url = format!("{}/v1beta3/crypto/us/bars", self.data_base);
        let limit_s = limit.to_string();
        // Without an explicit `start`, Alpaca returns only the current day's bar. Look
        // back `limit` days (+buffer) so `limit` daily bars actually come back.
        let start = (chrono::Utc::now() - chrono::Duration::days(limit as i64 + 5))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let resp = self
            .auth(self.http.get(&url))
            .query(&[
                ("symbols", symbol.as_str()),
                ("timeframe", "1Day"),
                ("limit", &limit_s),
                ("start", &start),
                ("sort", "desc"), // newest first, so `limit` gives the RECENT window
            ])
            .send()
            .await
            .map_err(|e| MarketError::Request(e.to_string()))?;

        if !resp.status().is_success() {
            let code = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(MarketError::Request(format!("bars {code}: {body}")));
        }

        let mut parsed: BarsResponse = resp
            .json()
            .await
            .map_err(|e| MarketError::Decode(e.to_string()))?;

        let raws = parsed.bars.remove(symbol.as_str()).unwrap_or_default();
        let mut bars: Vec<Bar> = raws
            .into_iter()
            .filter_map(|r| {
                Some(Bar {
                    symbol: symbol.clone(),
                    ts: parse_rfc3339_millis(&r.t)?,
                    open: r.o,
                    high: r.h,
                    low: r.l,
                    close: r.c,
                    volume: r.v,
                })
            })
            .collect();
        bars.reverse(); // desc -> asc (chronological)
        Ok(bars)
    }
}

/// Order/account JSON as echoed by Alpaca. Numeric money fields arrive as strings.
#[derive(Deserialize)]
struct RawOrder {
    id: String,
    client_order_id: String,
    symbol: String,
    status: String,
}

#[derive(Deserialize)]
struct RawAccount {
    buying_power: String,
}

#[derive(Deserialize)]
struct RawPosition {
    symbol: String,
    qty: String,
    market_value: String,
}

#[async_trait]
impl Broker for AlpacaRest {
    async fn buying_power(&self) -> Result<Decimal, BrokerError> {
        let url = format!("{}/v2/account", self.trade_base);
        let acct: RawAccount = self
            .auth(self.http.get(&url))
            .send()
            .await
            .map_err(|e| BrokerError::Request(e.to_string()))?
            .error_for_status()
            .map_err(|e| BrokerError::Request(e.to_string()))?
            .json()
            .await
            .map_err(|e| BrokerError::Decode(e.to_string()))?;
        acct.buying_power
            .parse()
            .map_err(|e| BrokerError::Decode(format!("buying_power: {e}")))
    }

    async fn positions(&self) -> Result<Vec<Position>, BrokerError> {
        let url = format!("{}/v2/positions", self.trade_base);
        let raws: Vec<RawPosition> = self
            .auth(self.http.get(&url))
            .send()
            .await
            .map_err(|e| BrokerError::Request(e.to_string()))?
            .error_for_status()
            .map_err(|e| BrokerError::Request(e.to_string()))?
            .json()
            .await
            .map_err(|e| BrokerError::Decode(e.to_string()))?;
        raws.into_iter()
            .map(|p| {
                Ok(Position {
                    symbol: Symbol::from(p.symbol),
                    qty: p.qty.parse().map_err(|e| BrokerError::Decode(format!("qty: {e}")))?,
                    market_value: p
                        .market_value
                        .parse()
                        .map_err(|e| BrokerError::Decode(format!("market_value: {e}")))?,
                })
            })
            .collect()
    }

    async fn submit_order(&self, req: &OrderRequest) -> Result<Order, BrokerError> {
        let url = format!("{}/v2/orders", self.trade_base);
        let body = serde_json::json!({
            "symbol": req.symbol.as_str(),
            "qty": req.qty.to_string(),
            "side": req.side,           // Side serializes lowercase
            "type": "market",
            "time_in_force": "gtc",
            "client_order_id": req.client_order_id,
        });
        let resp = self
            .auth(self.http.post(&url))
            .json(&body)
            .send()
            .await
            .map_err(|e| BrokerError::Request(e.to_string()))?;

        // 422 = duplicate client_order_id (idempotency): resolve to the existing order
        // rather than double-submitting.
        if resp.status() == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
            return self.order_by_client_id(&req.client_order_id).await;
        }
        let resp = resp
            .error_for_status()
            .map_err(|e| BrokerError::Rejected(e.to_string()))?;
        let raw: RawOrder = resp
            .json()
            .await
            .map_err(|e| BrokerError::Decode(e.to_string()))?;
        Ok(raw.into())
    }
}

impl AlpacaRest {
    async fn order_by_client_id(&self, client_order_id: &str) -> Result<Order, BrokerError> {
        let url = format!("{}/v2/orders:by_client_order_id", self.trade_base);
        let raw: RawOrder = self
            .auth(self.http.get(&url))
            .query(&[("client_order_id", client_order_id)])
            .send()
            .await
            .map_err(|e| BrokerError::Request(e.to_string()))?
            .error_for_status()
            .map_err(|e| BrokerError::Rejected(e.to_string()))?
            .json()
            .await
            .map_err(|e| BrokerError::Decode(e.to_string()))?;
        Ok(raw.into())
    }
}

impl From<RawOrder> for Order {
    fn from(r: RawOrder) -> Self {
        Order {
            id: r.id,
            client_order_id: r.client_order_id,
            symbol: Symbol::from(r.symbol),
            status: r.status,
        }
    }
}

// ---------------------------------------------------------------------------
// WS: crypto market-data stream
// ---------------------------------------------------------------------------

/// Alpaca crypto market-data websocket. Free tier, one connection, 24/7. In-band
/// auth: connect, send `auth`, wait for `authenticated`, send `subscribe`.
pub struct CryptoWs {
    key: String,
    secret: String,
    ws_url: String,
}

impl CryptoWs {
    pub fn new(key: String, secret: String, ws_url: String) -> Self {
        Self { key, secret, ws_url }
    }
}

/// One element of a WS frame (frames are JSON arrays of these). Fields are optional
/// because a frame carries several message types (`success`, `error`, `t`rade, ...).
#[derive(Deserialize)]
struct WsMsg {
    #[serde(rename = "T")]
    typ: String,
    #[serde(rename = "S")]
    symbol: Option<String>,
    #[serde(rename = "p")]
    price: Option<f64>,
    #[serde(rename = "s")]
    size: Option<f64>,
    #[serde(rename = "t")]
    ts: Option<String>,
    msg: Option<String>,
}

#[async_trait]
impl MarketStream for CryptoWs {
    async fn subscribe(
        &self,
        symbols: &[Symbol],
    ) -> Result<BoxStream<'static, Result<Tick, MarketError>>, MarketError> {
        let (mut ws, _resp) = connect_async(self.ws_url.as_str())
            .await
            .map_err(|e| MarketError::Connect(e.to_string()))?;

        // 1. auth
        let auth = serde_json::json!({"action":"auth","key":self.key,"secret":self.secret});
        ws.send(Message::text(auth.to_string()))
            .await
            .map_err(|e| MarketError::Subscribe(e.to_string()))?;

        // 2. wait for `authenticated` (skip the initial `connected`; bail on `error`)
        loop {
            match ws.next().await {
                Some(Ok(Message::Text(t))) => match classify(t.as_str()) {
                    Handshake::Authenticated => break,
                    Handshake::Error(m) => return Err(MarketError::Subscribe(m)),
                    Handshake::Other => continue,
                },
                Some(Ok(_)) => continue, // ping/binary during handshake
                Some(Err(e)) => return Err(MarketError::Connect(e.to_string())),
                None => return Err(MarketError::Connect("closed during auth".into())),
            }
        }

        // 3. subscribe to trades
        let syms: Vec<&str> = symbols.iter().map(|s| s.as_str()).collect();
        let sub = serde_json::json!({"action":"subscribe","trades":syms});
        ws.send(Message::text(sub.to_string()))
            .await
            .map_err(|e| MarketError::Subscribe(e.to_string()))?;

        // 4. stream ticks. A single frame can hold many ticks, so buffer them and
        // drain one per poll. Stream *ending* (Close/None/Err) is normal — the caller
        // (Ingester) reconnects; we just terminate here.
        let init: (_, VecDeque<Tick>) = (ws, VecDeque::new());
        let stream = futures::stream::unfold(init, |(mut ws, mut buf)| async move {
            loop {
                if let Some(tick) = buf.pop_front() {
                    return Some((Ok(tick), (ws, buf)));
                }
                match ws.next().await {
                    Some(Ok(Message::Text(t))) => {
                        for tick in parse_ticks(t.as_str()) {
                            buf.push_back(tick);
                        }
                        // if the frame carried no trades (subscription ack, etc.), loop
                    }
                    Some(Ok(Message::Close(_))) | None => return None,
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => continue,
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => {
                        tracing::warn!(error = %e, "crypto ws error; ending stream for reconnect");
                        return None;
                    }
                }
            }
        });
        Ok(stream.boxed())
    }
}

enum Handshake {
    Authenticated,
    Error(String),
    Other,
}

fn classify(frame: &str) -> Handshake {
    let msgs: Vec<WsMsg> = match serde_json::from_str(frame) {
        Ok(m) => m,
        Err(_) => return Handshake::Other,
    };
    for m in &msgs {
        if m.typ == "error" {
            return Handshake::Error(m.msg.clone().unwrap_or_else(|| "unknown ws error".into()));
        }
        if m.typ == "success" && m.msg.as_deref() == Some("authenticated") {
            return Handshake::Authenticated;
        }
    }
    Handshake::Other
}

/// Extract trade ticks (`T == "t"`) from one frame; non-trade messages are dropped.
fn parse_ticks(frame: &str) -> Vec<Tick> {
    let msgs: Vec<WsMsg> = match serde_json::from_str(frame) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, "undecodable ws frame");
            return Vec::new();
        }
    };
    msgs.into_iter()
        .filter(|m| m.typ == "t")
        .filter_map(|m| {
            Some(Tick {
                symbol: Symbol::from(m.symbol?),
                ts: parse_rfc3339_millis(m.ts.as_deref()?)?,
                price: m.price?,
                size: m.size.unwrap_or(0.0),
            })
        })
        .collect()
}

/// RFC3339 (`2024-01-02T03:04:05.678Z`) → unix epoch millis.
fn parse_rfc3339_millis(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_trade_frame_ignoring_status_and_bad_ts() {
        let frame = r#"[
            {"T":"subscription","trades":["BTC/USD"]},
            {"T":"t","S":"BTC/USD","p":42000.5,"s":0.01,"t":"2024-01-02T03:04:05.678Z"},
            {"T":"t","S":"ETH/USD","p":2500.0,"s":1.0,"t":"not-a-date"}
        ]"#;
        let ticks = parse_ticks(frame);
        assert_eq!(ticks.len(), 1, "only the well-formed trade survives");
        assert_eq!(ticks[0].symbol.as_str(), "BTC/USD");
        assert_eq!(ticks[0].price, 42000.5);
        assert_eq!(ticks[0].ts, 1_704_164_645_678);
    }

    #[test]
    fn classify_handshake_frames() {
        assert!(matches!(
            classify(r#"[{"T":"success","msg":"authenticated"}]"#),
            Handshake::Authenticated
        ));
        assert!(matches!(
            classify(r#"[{"T":"success","msg":"connected"}]"#),
            Handshake::Other
        ));
        assert!(matches!(
            classify(r#"[{"T":"error","code":402,"msg":"auth failed"}]"#),
            Handshake::Error(_)
        ));
    }

    #[test]
    fn trade_base_never_doubles_v2() {
        let want = "https://paper-api.alpaca.markets";
        assert_eq!(normalize_trade_base("https://paper-api.alpaca.markets"), want);
        assert_eq!(normalize_trade_base("https://paper-api.alpaca.markets/"), want);
        assert_eq!(normalize_trade_base("https://paper-api.alpaca.markets/v2"), want);
        assert_eq!(normalize_trade_base("https://paper-api.alpaca.markets/v2/"), want);
    }
}
