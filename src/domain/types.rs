//! Shared domain types. Money (order qty, position value, notional) is `Decimal`,
//! never f64. f64 appears only on bar OHLCV — that's indicator input, not money.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// A tradable symbol: `"BTC/USD"` (crypto) or `"AAPL"` (stock). Newtype so it can't
/// be swapped with an arbitrary string at call sites.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Symbol(pub String);

impl Symbol {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Symbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for Symbol {
    fn from(s: &str) -> Self {
        Symbol(s.to_string())
    }
}

impl From<String> for Symbol {
    fn from(s: String) -> Self {
        Symbol(s)
    }
}

/// One OHLCV bar. OHLCV are f64 on purpose — they feed `signals`/`data` math.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bar {
    pub symbol: Symbol,
    /// Unix epoch milliseconds.
    pub ts: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

/// A streamed trade print (crypto WS `t` message).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tick {
    pub symbol: Symbol,
    pub ts: i64,
    pub price: f64,
    pub size: f64,
}

/// Trade direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Buy,
    Sell,
}

/// What the decision agent wants to do. Hold is a first-class outcome (guards
/// downgrade to it on any doubt).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Action {
    Buy,
    Sell,
    Hold,
}

/// The decision agent's output for one symbol. `qty` is money → `Decimal`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub symbol: Symbol,
    pub action: Action,
    pub qty: Decimal,
    pub score: f64,
    pub reason: String,
}

/// A live position snapshot from the broker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub symbol: Symbol,
    pub qty: Decimal,
    pub market_value: Decimal,
}

/// An order to submit. `client_order_id` is the idempotency key
/// (`strategy_id:symbol:trading_day`) so a restart never double-submits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRequest {
    pub symbol: Symbol,
    pub qty: Decimal,
    pub side: Side,
    pub client_order_id: String,
}

/// An order as echoed back by the broker (Alpaca is source of truth).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub id: String,
    pub client_order_id: String,
    pub symbol: Symbol,
    pub status: String,
}

/// One row from the append-only events log, read side. Mirrors the persisted columns
/// (`Store::append_event`); the learning loop reads these back to score past decisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub ts: i64,
    pub step: String,
    pub decision_id: String,
    /// JSON blob, same as written — callers parse the fields they care about.
    pub payload: String,
}
