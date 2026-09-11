//! Engine: orchestration over the ports. Generic over the traits, so the pipeline
//! is testable with fakes — no live network or database needed.

pub mod backtest; // walk-forward replay over history, reusing the live pipeline
pub mod guards; // deterministic safety guards
pub mod ingest; // streaming owner-task + fan-out (tokio)
pub mod pipeline; // agents + WAL persistence
