//! In-memory store (impl #2 of `Store`) — for running without ScyllaDB. Logs every
//! event to stdout so you can watch the audit trail live, and keeps them in a Vec.
//! No persistence: use it to see the decision + paper-order path, not for real audit.

use async_trait::async_trait;
use std::sync::Mutex;

use crate::domain::error::StoreError;
use crate::domain::types::{Bar, Symbol};
use crate::traits::Store;

#[derive(Default)]
pub struct MemStore {
    events: Mutex<Vec<String>>,
}

impl MemStore {
    /// Number of events recorded so far.
    pub fn len(&self) -> usize {
        self.events.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl Store for MemStore {
    async fn append_event(
        &self,
        strategy_id: &str,
        symbol: &Symbol,
        _ts: i64,
        step: &str,
        _decision_id: &str,
        payload: &str,
    ) -> Result<(), StoreError> {
        tracing::info!(strategy_id, %symbol, step, "event (mem)");
        self.events
            .lock()
            .unwrap()
            .push(format!("{strategy_id} {symbol} {step} {payload}"));
        Ok(())
    }

    async fn write_bars(&self, _bars: &[Bar]) -> Result<(), StoreError> {
        Ok(()) // dropped in memory mode; bars are re-fetchable
    }
}
