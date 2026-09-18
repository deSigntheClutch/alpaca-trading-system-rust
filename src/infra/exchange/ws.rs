//! `CryptoWs` — the 24/7 crypto market-data websocket (`MarketStream`).
//!
//! In-band auth (a message, not a header): connect, send `auth`, wait for
//! `authenticated`, send `subscribe`.

use std::collections::VecDeque;

use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use futures::SinkExt;
use serde::Deserialize;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::domain::error::MarketError;
use crate::domain::types::{Symbol, Tick};
use crate::traits::MarketStream;

use super::parse_rfc3339_millis;

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
}
