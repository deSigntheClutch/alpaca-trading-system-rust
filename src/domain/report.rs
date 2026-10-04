//! The morning report and everything in it. Pure data: produced by `engine`
//! (screener, regime, commentary agent), written by `infra::report_store`, read by
//! the Next.js page through the ts-rs bindings these derives generate. Change a field
//! here and the TypeScript changes with it.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::signals::Rating;
use super::types::{EarningsEvent, Headline, Symbol};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub struct Regime {
    #[ts(type = "string")]
    pub symbol: Symbol,
    /// Timestamp of the close used, epoch millis.
    pub as_of: i64,
    pub close: f64,
    pub sma: f64,
    pub window: usize,
    pub above: bool,
    /// `(close / sma − 1) × 100`; negative when below.
    pub distance_pct: f64,
}

/// One scored name: the instrument's tags plus the signal and a few price facts the
/// report shows next to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub struct Scored {
    #[ts(type = "string")]
    pub symbol: Symbol,
    pub name: String,
    pub sector: String,
    pub themes: Vec<String>,
    pub region: String,
    pub is_etf: bool,
    pub score: f64,
    pub rating: Rating,
    pub rsi: f64,
    pub close: f64,
    /// Fractional changes over the last 1, 5 and 20 bars; `None` when the series is
    /// too short for that look-back.
    pub change_1d: Option<f64>,
    pub change_5d: Option<f64>,
    pub change_20d: Option<f64>,
    /// Timestamp of the bar the score was computed on, epoch millis.
    pub as_of: i64,
    pub bars: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub struct Skipped {
    #[ts(type = "string")]
    pub symbol: Symbol,
    pub reason: String,
}

/// Mean score of a group, with its size so a mean of one name reads as what it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub struct Aggregate {
    pub key: String,
    pub count: usize,
    pub mean_score: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub struct Screen {
    /// Newest bar timestamp across all scored names, epoch millis.
    pub as_of: i64,
    /// Every scored name, ETFs included, best score first; ties broken by symbol.
    pub ranked: Vec<Scored>,
    /// Top `top_n` of `ranked` excluding ETFs: the buy candidates.
    pub picks: Vec<Scored>,
    pub by_sector: Vec<Aggregate>,
    pub by_theme: Vec<Aggregate>,
    pub skipped: Vec<Skipped>,
    pub elapsed_ms: u64,
}

/// Why a name made the list, as one chip on the page. Chosen by the commentary agent
/// (#66) from this closed set; `Mixed` when no single reason dominates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
#[serde(rename_all = "snake_case")]
pub enum WatchReason {
    Momentum,
    Breakout,
    Oversold,
    Earnings,
    News,
    Mixed,
}

/// The LLM's note on one pick. The rule chose the name; this only describes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub struct Commentary {
    /// Two or three sentences.
    pub summary: String,
    pub watch_reason: WatchReason,
    /// One sentence: what would make this a bad idea.
    pub risk: String,
    pub model: String,
    pub generated_at: i64,
}

/// One buy candidate with everything the page shows for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub struct Pick {
    pub scored: Scored,
    /// This name's earnings event in the next seven days, if any.
    pub earnings: Option<EarningsEvent>,
    /// Last 24 hours, newest first, at most a handful.
    pub headlines: Vec<Headline>,
    /// `None` until the commentary agent runs, or when it declined to answer.
    pub commentary: Option<Commentary>,
}

/// How the run went. `warnings` is where a step that failed soft says so, so a report
/// with no earnings section is distinguishable from a week with no earnings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub struct RunStats {
    pub elapsed_ms: u64,
    pub warnings: Vec<String>,
    pub llm_calls: u32,
    pub llm_cached: u32,
    pub llm_input_tokens: u64,
    pub llm_output_tokens: u64,
    pub llm_cost_usd: f64,
}

/// The report. One file per day under `data/reports/`, plus `latest.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub struct MorningReport {
    /// Report date, UTC.
    #[ts(type = "string")]
    pub date: NaiveDate,
    pub generated_at: i64,
    pub regime: Option<Regime>,
    pub picks: Vec<Pick>,
    /// Universe names reporting in the next seven days, soonest first.
    pub earnings_this_week: Vec<EarningsEvent>,
    pub by_sector: Vec<Aggregate>,
    pub by_theme: Vec<Aggregate>,
    pub skipped: Vec<Skipped>,
    pub universe_size: usize,
    pub run: RunStats,
}
