//! The seams (ports). `infra` provides impls; `engine` is generic over these and
//! never depends on `infra` — so the pipeline is testable with fakes.

use async_trait::async_trait;
use futures::stream::BoxStream;

use rust_decimal::Decimal;

use crate::domain::error::{BrokerError, LlmError, MarketError, StoreError};
use crate::domain::types::{Bar, Order, OrderRequest, Position, Symbol, Tick};

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

/// Order placement + account state. impl #1 = Alpaca REST (paper). The pipeline is
/// generic over this, so execution is testable with a fake broker (no live orders).
#[async_trait]
pub trait Broker: Send + Sync {
    /// Cash available to open new positions.
    async fn buying_power(&self) -> Result<Decimal, BrokerError>;
    /// Current open positions (source of truth for SELL sizing).
    async fn positions(&self) -> Result<Vec<Position>, BrokerError>;
    /// Submit a market order. Idempotent on `client_order_id`: a duplicate must
    /// resolve to the already-placed order, never a second submission.
    async fn submit_order(&self, req: &OrderRequest) -> Result<Order, BrokerError>;
}

// Blanket impls so a shared `&Client` satisfies a port — one Scylla connection / one
// HTTP client can back a generic `Pipeline` for every symbol, no clone-per-symbol.

#[async_trait]
impl<T: BarSource + ?Sized> BarSource for &T {
    async fn daily_bars(&self, symbol: &Symbol, limit: usize) -> Result<Vec<Bar>, MarketError> {
        (**self).daily_bars(symbol, limit).await
    }
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

#[async_trait]
impl<T: LlmClient + ?Sized> LlmClient for &T {
    async fn message(&self, system: &str, user: &str) -> Result<String, LlmError> {
        (**self).message(system, user).await
    }
}

#[async_trait]
impl<T: Broker + ?Sized> Broker for &T {
    async fn buying_power(&self) -> Result<Decimal, BrokerError> {
        (**self).buying_power().await
    }
    async fn positions(&self) -> Result<Vec<Position>, BrokerError> {
        (**self).positions().await
    }
    async fn submit_order(&self, req: &OrderRequest) -> Result<Order, BrokerError> {
        (**self).submit_order(req).await
    }
}
