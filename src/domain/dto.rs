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

/// Which decision path produced a run. `Rule` maps the signal's rating straight to an
/// action and never calls a model: it is the control arm, free and reproducible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub enum Arm {
    #[serde(rename = "rule")]
    Rule,
    #[serde(rename = "llm")]
    Llm,
    #[serde(rename = "llm+debate")]
    LlmDebate,
}

impl Arm {
    pub fn as_str(self) -> &'static str {
        match self {
            Arm::Rule => "rule",
            Arm::Llm => "llm",
            Arm::LlmDebate => "llm+debate",
        }
    }

    /// LLM calls one decision day costs on this arm, at `debate_rounds` rounds.
    pub fn calls_per_day(self, debate_rounds: usize) -> usize {
        match self {
            Arm::Rule => 0,
            Arm::Llm => 2,
            Arm::LlmDebate => 2 + 2 * debate_rounds,
        }
    }
}

impl std::str::FromStr for Arm {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "rule" => Ok(Arm::Rule),
            "llm" => Ok(Arm::Llm),
            "llm+debate" | "debate" => Ok(Arm::LlmDebate),
            other => Err(format!("unknown arm '{other}' (rule | llm | llm+debate)")),
        }
    }
}

impl std::fmt::Display for Arm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Snapshot of everything that produced a result, echoed on every result so two runs
/// can be compared and attributed to whatever changed between them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub struct RunConfig {
    pub arm: Arm,
    /// Model id for an LLM arm; empty for the rule arm.
    pub model: String,
    /// `anthropic` | `bedrock` | empty for the rule arm.
    pub adapter: String,
    pub fee_bps: f64,
    pub slippage_bps: f64,
    pub starting_cash: f64,
    /// First and last decision day, `YYYY-MM-DD`.
    pub window_start: String,
    pub window_end: String,
    /// Symbols traded in this run.
    pub symbols: Vec<String>,
    /// Read-only symbols that inform the decision but are never traded (#49; empty
    /// until that lands).
    pub context_symbols: Vec<String>,
    /// Commit the binary was built from (`build.rs`), or "unknown".
    pub git_sha: String,
    /// Days whose decision failed and were recorded as a Hold.
    pub error_days: u32,
}

/// Row in the run history list (`GET /api/backtest/runs`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub struct RunSummary {
    pub run_id: String,
    /// Epoch millis.
    pub created_at: i64,
    pub config: RunConfig,
}

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

/// One lot: a completed or still-open round trip. The only source for "did this
/// trade make money". Accounting is per lot, FIFO (`engine::metrics::pair_trades`):
/// every buy marker on the chart maps to exactly one row; a sell that closes three
/// lots produces three rows sharing an `exit_ts`. An open lot has `exit_* = None`
/// and a `pnl` marked at the last close.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub struct Trade {
    pub symbol: String,
    pub entry_ts: i64,
    pub entry_price: f64,
    pub exit_ts: Option<i64>,
    pub exit_price: Option<f64>,
    pub qty: f64,
    /// Net of fees on both legs.
    pub pnl: f64,
    /// `pnl` over the entry notional.
    pub pnl_pct: f64,
    pub fees: f64,
    pub entry_reason: String,
    pub exit_reason: String,
}

/// The summary cards. Computed by `engine::metrics::summarize` from the equity curve,
/// the paired trades and the daily exposure series.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub struct Metrics {
    /// Strategy total return as a fraction (0.12 = +12%), net of fees and slippage.
    pub total_return: f64,
    /// Worst peak-to-trough drop as a positive fraction (0.30 = -30% drawdown).
    pub max_drawdown: f64,
    /// Annualised, from daily equity returns, risk-free rate zero; 0 when undefined.
    pub sharpe: f64,
    /// Closed round trips. Open lots are listed in `trades` but not counted.
    pub num_trades: u32,
    /// Closed trades with positive net P&L over closed trades; 0 when none closed.
    pub win_rate: f64,
    /// Mean net P&L of winning closed trades (0 when none).
    pub avg_win: f64,
    /// Mean net P&L of losing closed trades, a negative number (0 when none).
    pub avg_loss: f64,
    /// Gross wins over gross losses. `None` when there are no losing trades, which
    /// would otherwise be infinity — and serde writes non-finite floats as null anyway.
    pub profit_factor: Option<f64>,
    /// Average share of equity held as position over the window, so the strategy and
    /// a fully-invested buy-and-hold benchmark become comparable.
    pub exposure: f64,
    /// Buy-and-hold return over the same window — the benchmark to beat.
    pub buy_hold_return: f64,
    /// Fees paid over the window, in cash.
    pub total_fees: f64,
}

/// One symbol's backtest: the per-day series, the trade ledger and the summary. One
/// of these is written per line to `backtest.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub struct BacktestResult {
    /// Shared by every result of the same run.
    pub run_id: String,
    pub config: RunConfig,
    pub symbol: String,
    /// Cash the sim started with — baseline for the equity curve.
    pub starting_cash: f64,
    pub days: Vec<DayRecord>,
    pub trades: Vec<Trade>,
    pub metrics: Metrics,
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
