//! The orchestrator. Per symbol, runs the thin-slice pipeline:
//!
//!   fetch bars/quote  ->  Technical Analyst  ->  Decision  ->  [human gate]  ->  Execute
//!
//! Each step is a plain async fn; the orchestrator (`run`) sequences them and
//! writes every step to the audit log. Sentiment + Risk agents slot in between
//! Analyst and Decision later (see DESIGN.md).

use crate::{alpaca::Alpaca, llm::Claude, Config};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Action {
    Buy,
    Sell,
    Hold,
}

/// The Decision agent's output — what we'd execute, plus its reasoning and the
/// deterministic signal score it was given.
#[derive(Debug, Clone, Serialize)]
pub struct Decision {
    pub symbol: String,
    pub action: Action,
    pub qty: f64,
    pub score: f64, // from alpaca_core::signals (deterministic)
    pub reason: String, // the LLM's justification
}

/// Market Data step: fetch daily closes + latest quote.
pub async fn fetch(alpaca: &Alpaca, symbol: &str) -> anyhow::Result<Vec<f64>> {
    todo!("alpaca.daily_bars(symbol, ~60).map(|b| b.close)")
}

/// Technical Analyst: run the deterministic signal (alpaca_core) on the closes,
/// then ask Claude to interpret the numbers into a short analysis. The model
/// never computes indicators — it only reads them.
pub async fn analyze(llm: &Claude, symbol: &str, closes: &[f64]) -> anyhow::Result<(f64, String)> {
    todo!("let sig = alpaca_core::signals::compute(closes); llm.message(TA_SYSTEM, numbers) -> (sig.score, analysis)")
}

/// Decision agent: turn the analysis into a concrete Action + qty.
pub async fn decide(llm: &Claude, symbol: &str, score: f64, analysis: &str) -> anyhow::Result<Decision> {
    todo!("llm.message(DECIDE_SYSTEM, analysis) -> parse Action/qty/reason into Decision")
}

/// Execution agent: place the order on paper (skips on Hold).
pub async fn execute(alpaca: &Alpaca, d: &Decision) -> anyhow::Result<Option<crate::alpaca::Order>> {
    todo!("map Decision.action -> Side, alpaca.submit_order; None if Hold")
}

/// Orchestrate one symbol end to end. `approve` gates execution (human-in-loop).
pub async fn run(
    cfg: &Config,
    alpaca: &Alpaca,
    llm: &Claude,
    symbol: &str,
    approve: impl Fn(&Decision) -> bool,
) -> anyhow::Result<Option<crate::alpaca::Order>> {
    todo!("fetch -> analyze -> decide -> audit::append -> if cfg.human_in_loop && !approve(&d) skip -> execute -> audit")
}
