//! `BarSource` / `MarketStream` adapters.
//!   - `AlpacaBars` — historical bars (crypto and stocks) over a shared `AlpacaClient`.
//!   - `CryptoWs`   — the 24/7 crypto trade websocket. In-band auth (a message, not a
//!     header): connect, send `auth`, wait for `authenticated`, send `subscribe`.

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use futures::SinkExt;
use serde::Deserialize;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::domain::error::MarketError;
use crate::domain::types::{AssetClass, Bar, Symbol, Tick, Timeframe};
use crate::traits::{BarSource, MarketStream};

use super::clients::alpaca::{BarsResponse, Host, RawBar};
use super::clients::{parse_rfc3339_millis, AlpacaClient};

// ---- AlpacaBars -------------------------------------------------------------------

pub struct AlpacaBars {
    client: Arc<AlpacaClient>,
}

impl AlpacaBars {
    pub fn new(client: Arc<AlpacaClient>) -> Self {
        Self { client }
    }
}

/// Calendar days to look back so that `limit` bars of `timeframe` fit in the window.
/// Crypto trades every hour of every day; stocks five days a week, and hourly stock
/// bars are counted at the 7 regular-session bars a day, so extended hours only add
/// margin. Always generous: a short window silently returns fewer bars than asked.
fn lookback_days(class: AssetClass, timeframe: Timeframe, limit: usize) -> usize {
    let bars_per_day = match (class, timeframe) {
        (AssetClass::Crypto, Timeframe::Hour1) => 24,
        (AssetClass::Stock, Timeframe::Hour1) => 7,
        (_, Timeframe::Day1) => 1,
        (_, Timeframe::Week1) => return limit * 7 + 14,
    };
    let days = limit.div_ceil(bars_per_day);
    match class {
        AssetClass::Crypto => days + 5,
        AssetClass::Stock => days * 7 / 5 + 10,
    }
}

/// Alpaca caps one bars page at this many rows.
const MAX_PAGE: usize = 10_000;

/// Endpoint and the class-specific query parameters shared by both fetch shapes.
/// `adjustment=all` folds splits and dividends into history so a 4:1 split is not a
/// -75% bar; the crypto endpoint has no such parameter.
fn endpoint(class: AssetClass) -> (&'static str, &'static [(&'static str, &'static str)]) {
    match class {
        AssetClass::Crypto => ("/v1beta3/crypto/us/bars", &[]),
        AssetClass::Stock => ("/v2/stocks/bars", &[("adjustment", "all")]),
    }
}

/// Wire → domain for one symbol's rows; a row whose timestamp does not parse is
/// dropped rather than given a made-up time.
fn into_bars(symbol: &Symbol, raws: Vec<RawBar>) -> Vec<Bar> {
    raws.into_iter()
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
        .collect()
}

fn rfc3339(ms: i64) -> Result<String, MarketError> {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .ok_or_else(|| MarketError::Request(format!("timestamp out of range: {ms}")))
}

#[async_trait]
impl BarSource for AlpacaBars {
    /// Most-recent `limit` bars of `timeframe`, returned oldest→newest (what
    /// `signals::compute` wants). One symbol per request: Alpaca fills a multi-symbol
    /// `limit` symbol by symbol, so batching buys nothing here. Callers wanting
    /// several symbols run these concurrently.
    async fn bars(
        &self,
        symbol: &Symbol,
        timeframe: Timeframe,
        limit: usize,
    ) -> Result<Vec<Bar>, MarketError> {
        let class = symbol.asset_class();
        let (path, extra) = endpoint(class);

        // Without an explicit `start`, Alpaca returns only the current day's bars. Look
        // back far enough in calendar days that `limit` bars actually come back;
        // over-fetching is harmless because `sort=desc` + `limit` keeps the recent end.
        let start = (chrono::Utc::now()
            - chrono::Duration::days(lookback_days(class, timeframe, limit) as i64))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let limit_s = limit.to_string();

        let mut params: Vec<(&str, &str)> = vec![
            ("symbols", symbol.as_str()),
            ("timeframe", timeframe.as_str()),
            ("limit", &limit_s),
            ("start", &start),
            ("sort", "desc"), // newest first, so `limit` gives the RECENT window
        ];
        params.extend_from_slice(extra);

        let mut parsed: BarsResponse = self.client.get_json(Host::Data, path, &params).await?;
        let mut bars = into_bars(
            symbol,
            parsed.bars.remove(symbol.as_str()).unwrap_or_default(),
        );
        bars.reverse(); // desc -> asc (chronological)
        Ok(bars)
    }

    /// Everything in `[start, end)`, oldest first, following `next_page_token` until
    /// the vendor runs out. Pages are 10,000 rows, so ten years of daily bars is one
    /// request and five years of hourly is two. The range is also enforced locally:
    /// Alpaca's `end` is inclusive and we want half-open.
    async fn bars_range(
        &self,
        symbol: &Symbol,
        timeframe: Timeframe,
        start: i64,
        end: i64,
    ) -> Result<Vec<Bar>, MarketError> {
        if start >= end {
            return Ok(Vec::new());
        }
        let class = symbol.asset_class();
        let (path, extra) = endpoint(class);
        let (start_s, end_s) = (rfc3339(start)?, rfc3339(end)?);
        let page = MAX_PAGE.to_string();

        let mut base: Vec<(&str, &str)> = vec![
            ("symbols", symbol.as_str()),
            ("timeframe", timeframe.as_str()),
            ("start", &start_s),
            ("end", &end_s),
            ("limit", &page),
            ("sort", "asc"),
        ];
        base.extend_from_slice(extra);

        let mut out: Vec<Bar> = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut parsed: BarsResponse = {
                let mut params = base.clone();
                if let Some(tok) = &page_token {
                    params.push(("page_token", tok.as_str()));
                }
                self.client.get_json(Host::Data, path, &params).await?
            };
            let rows = parsed.bars.remove(symbol.as_str()).unwrap_or_default();
            let got = rows.len();
            out.extend(into_bars(symbol, rows));
            match parsed.next_page_token {
                Some(tok) if !tok.is_empty() && got > 0 => page_token = Some(tok),
                _ => break,
            }
        }

        out.retain(|b| b.ts >= start && b.ts < end);
        out.sort_by_key(|b| b.ts);
        out.dedup_by_key(|b| b.ts);
        Ok(out)
    }
}

#[cfg(test)]
mod lookback_tests {
    use super::*;

    #[test]
    fn stocks_look_back_further_than_crypto_for_the_same_bar_count() {
        let c = lookback_days(AssetClass::Crypto, Timeframe::Day1, 60);
        let s = lookback_days(AssetClass::Stock, Timeframe::Day1, 60);
        assert_eq!(c, 65);
        assert!(s > c, "stock={s} crypto={c}");
        assert!(
            s >= 60 * 7 / 5,
            "60 trading days need at least 84 calendar days"
        );
    }

    #[test]
    fn hourly_and_weekly_scale_with_the_timeframe() {
        assert_eq!(lookback_days(AssetClass::Crypto, Timeframe::Hour1, 48), 7); // 2 days + 5
        assert!(lookback_days(AssetClass::Stock, Timeframe::Hour1, 70) >= 14); // 10 sessions
        assert_eq!(lookback_days(AssetClass::Stock, Timeframe::Week1, 10), 84);
    }
}

// ---- CryptoWs ---------------------------------------------------------------------

/// Alpaca crypto market-data websocket. Free tier, one connection, 24/7. In-band
/// auth: connect, send `auth`, wait for `authenticated`, send `subscribe`.
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

#[cfg(test)]
mod ws_tests {
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
}
