//! `AlpacaRest` — historical crypto bars (`BarSource`) + orders/account (`Broker`).
//!
//! Market data lives on a different host than trading (`data.` vs `paper-api.`), so
//! `AlpacaRest` holds both bases.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use rust_decimal::Decimal;
use serde::Deserialize;

use crate::domain::error::{BrokerError, MarketError};
use crate::domain::types::{AssetClass, Bar, Order, OrderRequest, Position, Symbol};
use crate::traits::{BarSource, Broker};

use super::parse_rfc3339_millis;

const KEY_HEADER: &str = "APCA-API-KEY-ID";
const SECRET_HEADER: &str = "APCA-API-SECRET-KEY";

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

/// Alpaca bars response, same shape for crypto and stocks:
/// `{ "bars": { "BTC/USD": [ {...} ] }, "next_page_token": null }`.
#[derive(Deserialize)]
struct BarsResponse {
    #[serde(default)]
    bars: HashMap<String, Vec<RawBar>>,
    #[serde(default)]
    next_page_token: Option<String>,
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

/// Alpaca caps one bars request at this many data points across all symbols.
const MAX_PAGE: usize = 10_000;

#[async_trait]
impl BarSource for AlpacaRest {
    async fn daily_bars(&self, symbol: &Symbol, limit: usize) -> Result<Vec<Bar>, MarketError> {
        let mut many = self.get_bars_many(std::slice::from_ref(symbol), limit).await?;
        Ok(many.remove(symbol).unwrap_or_default())
    }

    /// One request for all symbols (plus pagination), not N. The endpoint follows
    /// the asset class of the first symbol; callers do not mix classes in one call.
    /// Each symbol's bars come back oldest→newest, at most `limit` of them.
    async fn get_bars_many(
        &self,
        symbols: &[Symbol],
        limit: usize,
    ) -> Result<HashMap<Symbol, Vec<Bar>>, MarketError> {
        let mut out: HashMap<Symbol, Vec<Bar>> =
            symbols.iter().map(|s| (s.clone(), Vec::new())).collect();
        if symbols.is_empty() || limit == 0 {
            return Ok(out);
        }
        let class = symbols[0].asset_class();
        debug_assert!(symbols.iter().all(|s| s.asset_class() == class));

        let url = match class {
            AssetClass::Crypto => format!("{}/v1beta3/crypto/us/bars", self.data_base),
            AssetClass::Stock => format!("{}/v2/stocks/bars", self.data_base),
        };

        // Without an explicit `start`, Alpaca returns only the current day's bar. Look
        // back far enough in calendar days that `limit` *trading* days come back:
        // crypto trades every day; stocks five days a week plus holidays. The trading
        // calendar (#52) replaces this estimate with an exact count.
        let lookback_days = match class {
            AssetClass::Crypto => limit + 5,
            AssetClass::Stock => limit * 7 / 5 + 10,
        };
        let start = (chrono::Utc::now() - chrono::Duration::days(lookback_days as i64))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

        // `limit` on the wire counts data points across ALL symbols, not per symbol.
        let page_limit = (limit * symbols.len()).min(MAX_PAGE).to_string();
        let joined = symbols.iter().map(Symbol::as_str).collect::<Vec<_>>().join(",");

        let mut params: Vec<(&str, &str)> = vec![
            ("symbols", &joined),
            ("timeframe", "1Day"),
            ("limit", &page_limit),
            ("start", &start),
            ("sort", "desc"), // newest first, so `limit` gives the RECENT window
        ];
        if class == AssetClass::Stock {
            // Fold splits and dividends into history so a 4:1 split is not a -75% bar.
            // The crypto endpoint has no such parameter.
            params.push(("adjustment", "all"));
        }

        let mut page_token: Option<String> = None;
        loop {
            let mut req = self.auth(self.http.get(&url)).query(&params);
            if let Some(tok) = &page_token {
                req = req.query(&[("page_token", tok.as_str())]);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| MarketError::Request(e.to_string()))?;

            if !resp.status().is_success() {
                let code = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(MarketError::Request(format!("bars {code}: {body}")));
            }

            let parsed: BarsResponse = resp
                .json()
                .await
                .map_err(|e| MarketError::Decode(e.to_string()))?;

            for (key, raws) in parsed.bars {
                let symbol = Symbol::from(key);
                let bars = out.entry(symbol.clone()).or_default();
                bars.extend(raws.into_iter().filter_map(|r| {
                    Some(Bar {
                        symbol: symbol.clone(),
                        ts: parse_rfc3339_millis(&r.t)?,
                        open: r.o,
                        high: r.h,
                        low: r.l,
                        close: r.c,
                        volume: r.v,
                    })
                }));
            }

            match parsed.next_page_token {
                Some(tok) if !tok.is_empty() => page_token = Some(tok),
                _ => break,
            }
        }

        // Pages arrive newest-first and are grouped by symbol on the wire, so sort
        // explicitly rather than trusting the order, then keep the recent `limit`.
        for bars in out.values_mut() {
            bars.sort_by_key(|b| b.ts);
            bars.dedup_by_key(|b| b.ts);
            if bars.len() > limit {
                bars.drain(..bars.len() - limit);
            }
        }
        Ok(out)
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
                    qty: p
                        .qty
                        .parse()
                        .map_err(|e| BrokerError::Decode(format!("qty: {e}")))?,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trade_base_never_doubles_v2() {
        let want = "https://paper-api.alpaca.markets";
        assert_eq!(
            normalize_trade_base("https://paper-api.alpaca.markets"),
            want
        );
        assert_eq!(
            normalize_trade_base("https://paper-api.alpaca.markets/"),
            want
        );
        assert_eq!(
            normalize_trade_base("https://paper-api.alpaca.markets/v2"),
            want
        );
        assert_eq!(
            normalize_trade_base("https://paper-api.alpaca.markets/v2/"),
            want
        );
    }
}
