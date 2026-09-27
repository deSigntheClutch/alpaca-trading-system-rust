use std::collections::HashMap;

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::domain::error::MarketError;
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
    /// Most-recent `limit` daily bars for one symbol, oldest→newest.
    async fn daily_bars(&self, symbol: &Symbol, limit: usize) -> Result<Vec<Bar>, MarketError>;

    /// The same for several symbols at once, keyed by symbol. Every requested
    /// symbol is present in the map, with an empty `Vec` when nothing came back.
    ///
    /// The default loops over `daily_bars`; a network-backed source should
    /// override it with one batched request (#52).
    async fn get_bars_many(
        &self,
        symbols: &[Symbol],
        limit: usize,
    ) -> Result<HashMap<Symbol, Vec<Bar>>, MarketError> {
        let mut out = HashMap::with_capacity(symbols.len());
        for s in symbols {
            out.insert(s.clone(), self.daily_bars(s, limit).await?);
        }
        Ok(out)
    }
}

// Blanket impls so a shared `&Client` satisfies a port — one Scylla connection / one
// HTTP client can back a generic `Pipeline` for every symbol, no clone-per-symbol.

#[async_trait]
impl<T: BarSource + ?Sized> BarSource for &T {
    async fn daily_bars(&self, symbol: &Symbol, limit: usize) -> Result<Vec<Bar>, MarketError> {
        (**self).daily_bars(symbol, limit).await
    }

    async fn get_bars_many(
        &self,
        symbols: &[Symbol],
        limit: usize,
    ) -> Result<HashMap<Symbol, Vec<Bar>>, MarketError> {
        (**self).get_bars_many(symbols, limit).await
    }
}