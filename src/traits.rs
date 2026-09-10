//! The seams (ports). `infra` provides impls; `engine` is generic over these and
//! never depends on `infra` — so the pipeline is testable with fakes.

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::domain::error::{LlmError, MarketError, StoreError};
use crate::domain::types::{Bar, Symbol, Tick};

/// A live market-data source. impl #1 = Alpaca crypto WS; a stock WS / replay feed
/// drops in as impl #2 with no pipeline change.
#[async_trait]
pub trait MarketStream: Send + Sync {
    /// Connect + subscribe, yielding ticks until the stream ends. The caller owns
    /// reconnect (the stream ending is a normal event, not an error).
    async fn subscribe(
        &self,
        symbols: &[Symbol],
    ) -> Result<BoxStream<'static, Result<Tick, MarketError>>, MarketError>;
}

/// Historical bars over REST. Used by the (later) stock cron strategies + backfill.
#[async_trait]
pub trait BarSource: Send + Sync {
    async fn daily_bars(&self, symbol: &Symbol, limit: usize) -> Result<Vec<Bar>, MarketError>;
}

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

/// LLM chat. impl #1 = Claude (Anthropic Messages API).
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn message(&self, system: &str, user: &str) -> Result<String, LlmError>;
}
