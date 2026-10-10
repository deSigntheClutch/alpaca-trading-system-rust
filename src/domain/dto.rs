//! Read-only DTOs at the Rust↔frontend boundary — the SINGLE SOURCE OF TRUTH for
//! the SPA's types. `engine::backtest` writes these into `backtest.jsonl`; `bin/web`
//! serves them as JSON; `#[derive(TS)]` generates the matching TypeScript into
//! `frontend/src/bindings/` (run `cargo test` to regenerate). Change a field here and
//! the frontend types change with it — no hand-maintained duplicate to drift.
//!
//! Types are plain (`f64`, `String`) not domain types (`Decimal`, `Action`): this is
//! the wire shape the browser consumes, deliberately decoupled from internal types.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One day of a backtest — everything the chart needs for that point: the price
/// (line + marker y), the signal that drove it, the action taken, running equity
/// (equity curve), and the LLM's own words (shown when the marker is clicked).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub struct DayRecord {
    /// Bar timestamp, epoch millis — the chart x-axis.
    pub ts: i64,
    /// Close price that day — price line and the y of any buy/sell marker.
    pub close: f64,
    /// Signal score in [-1, 1].
    pub score: f64,
    pub rsi: f64,
    /// "Buy" | "Sell" | "Hold" — drives the marker (▲/▼) or none.
    pub action: String,
    /// Filled qty in base asset (0 for Hold).
    pub qty: f64,
    /// Portfolio value (cash + position·close) at this day's close — the equity curve.
    /// An order decided today fills at the next open, so it shows up tomorrow.
    pub equity: f64,
    /// The decision agent's `reason`.
    pub decision_reason: String,
    /// The analyst agent's full analysis text.
    pub analyst_text: String,
}

/// One symbol's backtest: the per-day series plus summary metrics. One of these is
/// written per line to `backtest.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub struct BacktestResult {
    pub symbol: String,
    /// Cash the sim started with — baseline for the equity curve.
    pub starting_cash: f64,
    pub days: Vec<DayRecord>,
    /// Strategy total return as a fraction (0.12 = +12%).
    pub total_return: f64,
    /// Worst peak-to-trough drop as a positive fraction (0.30 = -30% drawdown).
    pub max_drawdown: f64,
    pub num_trades: u32,
    /// Buy-and-hold return over the same window — the benchmark to beat.
    pub buy_hold_return: f64,
}

/// One row of the live/paper event trail (`events.jsonl`), served as-is to the events
/// view. Mirrors what `MemStore` writes: `payload` is inlined JSON, not a string.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub struct EventRow {
    pub strategy_id: String,
    pub symbol: String,
    pub ts: i64,
    pub step: String,
    pub decision_id: String,
    #[ts(type = "unknown")]
    pub payload: serde_json::Value,
}
