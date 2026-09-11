//! In-memory store (impl #2 of `Store`) — for running without ScyllaDB. Optionally
//! appends every event as a JSON line to a local file (`EVENTS_PATH`) so you can inspect
//! the full decision trail with `jq` even with no DB. Same fields as the Scylla `events`
//! table, so it's a faithful preview. No crash-safety guarantees: this is for looking,
//! not for a real WAL.

use async_trait::async_trait;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::PathBuf;

use crate::domain::error::StoreError;
use crate::domain::types::{Bar, Symbol};
use crate::traits::Store;

#[derive(Default)]
pub struct MemStore {
    /// If set, each event is also appended here as one JSON line.
    path: Option<PathBuf>,
}

impl MemStore {
    /// `path = Some(..)` → also append events as JSON lines there (for `jq`); `None` →
    /// stdout log only.
    pub fn new(path: Option<PathBuf>) -> Self {
        Self { path }
    }
}

#[async_trait]
impl Store for MemStore {
    async fn append_event(
        &self,
        strategy_id: &str,
        symbol: &Symbol,
        ts: i64,
        step: &str,
        decision_id: &str,
        payload: &str,
    ) -> Result<(), StoreError> {
        tracing::info!(strategy_id, %symbol, step, "event (mem)");

        if let Some(path) = &self.path {
            // Inline the payload as real JSON when it parses, else keep it as a string —
            // so `jq '.payload'` works on decision/analyst blobs.
            let payload_val: serde_json::Value = serde_json::from_str(payload)
                .unwrap_or_else(|_| serde_json::Value::String(payload.to_string()));
            let line = serde_json::json!({
                "ts": ts,
                "strategy_id": strategy_id,
                "symbol": symbol.as_str(),
                "step": step,
                "decision_id": decision_id,
                "payload": payload_val,
            });
            // Best-effort: a failed inspection-log write must not abort a decision cycle.
            match OpenOptions::new().create(true).append(true).open(path) {
                Ok(mut f) => {
                    let _ = writeln!(f, "{line}");
                }
                Err(e) => tracing::warn!(error = %e, ?path, "could not append event line"),
            }
        }
        Ok(())
    }

    async fn write_bars(&self, _bars: &[Bar]) -> Result<(), StoreError> {
        Ok(()) // dropped in memory mode; bars are re-fetchable
    }
}
