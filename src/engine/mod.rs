//! Engine: orchestration over the ports. Generic over the traits, so the pipeline
//! is testable with fakes — no live network or database needed.

pub mod agents; // LLM decision roles — analyst, trader (researcher/debate next)
pub mod backtest; // walk-forward replay over history, reusing the live pipeline
pub mod guards; // deterministic safety guards
pub mod ingest; // streaming owner-task + fan-out (tokio)
pub mod pipeline; // orchestrates the agents + WAL persistence
pub mod regime; // market regime line: index vs its 200-day SMA
pub mod scorecard; // did the rule work: past picks vs what happened next (#35)
pub mod screener; // score the universe, rank, pick the top N (the morning report's core)
