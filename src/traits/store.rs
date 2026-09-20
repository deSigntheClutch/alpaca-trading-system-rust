use async_trait::async_trait;
use crate::domain::error::StoreError;
use crate::domain::types::{Bar, Symbol};

/// Append-only event log + market-data store. impl #1 = ScyllaDB (+ local-file
/// fallback); QuestDB analytics is the upgrade path.
#[async_trait]
pub trait Store: Send + Sync {
    /// One row per pipeline step. `step` is part of the clustering key so multiple
    /// steps at the same ts don't overwrite each other.
    async fn append_event(
        &self,
        strategy_id: &str,
        symbol: &Symbol,
        ts: i64,
        step: &str,
        decision_id: &str,
        payload: &str,
    ) -> Result<(), StoreError>;

    /// Bulk-persist bars/ticks into the market-data firehose table.
    async fn write_bars(&self, bars: &[Bar]) -> Result<(), StoreError>;
}

#[async_trait]
impl<T: Store + ?Sized> Store for &T {
    async fn append_event(
        &self,
        strategy_id: &str,
        symbol: &Symbol,
        ts: i64,
        step: &str,
        decision_id: &str,
        payload: &str,
    ) -> Result<(), StoreError> {
        (**self)
            .append_event(strategy_id, symbol, ts, step, decision_id, payload)
            .await
    }
    async fn write_bars(&self, bars: &[Bar]) -> Result<(), StoreError> {
        (**self).write_bars(bars).await
    }
}