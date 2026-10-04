use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::domain::error::MarketError;
use crate::domain::types::{Bar, Symbol, Tick, Timeframe};

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
    /// Most-recent `limit` bars of `timeframe` for one symbol, oldest→newest.
    async fn bars(
        &self,
        symbol: &Symbol,
        timeframe: Timeframe,
        limit: usize,
    ) -> Result<Vec<Bar>, MarketError>;

    /// Every bar of `timeframe` with `start <= ts < end` (epoch millis), oldest→newest,
    /// however many pages that takes. This is the history load; `bars` is the recent
    /// window. Half-open so `end = midnight today` means "through yesterday's close".
    async fn bars_range(
        &self,
        symbol: &Symbol,
        timeframe: Timeframe,
        start: i64,
        end: i64,
    ) -> Result<Vec<Bar>, MarketError>;

    /// `bars` at the daily timeframe — what the pipeline and backtest read.
    async fn daily_bars(&self, symbol: &Symbol, limit: usize) -> Result<Vec<Bar>, MarketError> {
        self.bars(symbol, Timeframe::Day1, limit).await
    }
}

// Blanket impls so a shared `&Client` satisfies a port — one Scylla connection / one
// HTTP client can back a generic `Pipeline` for every symbol, no clone-per-symbol.

#[async_trait]
impl<T: BarSource + ?Sized> BarSource for &T {
    async fn bars(
        &self,
        symbol: &Symbol,
        timeframe: Timeframe,
        limit: usize,
    ) -> Result<Vec<Bar>, MarketError> {
        (**self).bars(symbol, timeframe, limit).await
    }

    async fn bars_range(
        &self,
        symbol: &Symbol,
        timeframe: Timeframe,
        start: i64,
        end: i64,
    ) -> Result<Vec<Bar>, MarketError> {
        (**self).bars_range(symbol, timeframe, start, end).await
    }

    async fn daily_bars(&self, symbol: &Symbol, limit: usize) -> Result<Vec<Bar>, MarketError> {
        (**self).daily_bars(symbol, limit).await
    }
}
