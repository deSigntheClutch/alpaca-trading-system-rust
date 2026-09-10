//! Crypto streaming ingester — the tokio surface of the system.
//!
//! One owner task holds the websocket and fans every tick out to N consumers over a
//! `broadcast` channel (the pipeline is one such consumer). The stream *ending* is a
//! normal event (`MarketStream` contract), so `run` reconnects with exponential
//! backoff. Shutdown = abort the task (see `main`): a read loop has no state to
//! corrupt, so aborting mid-`await` just drops the socket.

use std::time::Duration;

use futures::stream::StreamExt;
use tokio::sync::broadcast;

use crate::domain::types::{Symbol, Tick};
use crate::traits::MarketStream;

const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Owns the market stream and a broadcast sender. Generic over the port (no infra
/// coupling), so it's testable with a fake stream and zero network.
pub struct Ingester<M: MarketStream> {
    market: M,
    tx: broadcast::Sender<Tick>,
}

impl<M: MarketStream> Ingester<M> {
    /// `capacity` bounds the per-consumer backlog; a slow consumer that overflows it
    /// gets a `Lagged` error and skips ahead rather than stalling the owner task.
    pub fn new(market: M, capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);
        Self { market, tx }
    }

    /// Subscribe a new consumer. Call before `run` to guarantee no ticks are missed.
    pub fn subscribe(&self) -> broadcast::Receiver<Tick> {
        self.tx.subscribe()
    }

    /// Connect → fan out ticks → reconnect forever. Returns only if aborted.
    pub async fn run(&self, symbols: &[Symbol]) {
        let mut backoff = BACKOFF_START;
        loop {
            match self.market.subscribe(symbols).await {
                Ok(mut stream) => {
                    backoff = BACKOFF_START; // healthy connect resets the backoff
                    tracing::info!(?symbols, "crypto stream connected");
                    while let Some(item) = stream.next().await {
                        match item {
                            // send() errs only when there are no receivers — harmless.
                            Ok(tick) => {
                                let _ = self.tx.send(tick);
                            }
                            Err(e) => tracing::warn!(error = %e, "tick error"),
                        }
                    }
                    tracing::warn!("crypto stream ended; reconnecting");
                }
                Err(e) => {
                    tracing::warn!(error = %e, backoff = ?backoff, "subscribe failed; backing off");
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::MarketError;
    use async_trait::async_trait;
    use futures::stream::BoxStream;
    use std::sync::Arc;

    fn tick(price: f64) -> Tick {
        Tick {
            symbol: Symbol::from("BTC/USD"),
            ts: 0,
            price,
            size: 0.0,
        }
    }

    /// Yields three ticks then ends (which drives a reconnect — fine for the test,
    /// we read exactly three and abort).
    struct FakeMarket;

    #[async_trait]
    impl MarketStream for FakeMarket {
        async fn subscribe(
            &self,
            _symbols: &[Symbol],
        ) -> Result<BoxStream<'static, Result<Tick, MarketError>>, MarketError> {
            let items = vec![Ok(tick(1.0)), Ok(tick(2.0)), Ok(tick(3.0))];
            Ok(futures::stream::iter(items).boxed())
        }
    }

    #[tokio::test]
    async fn fans_out_ticks_to_a_subscriber() {
        let ing = Arc::new(Ingester::new(FakeMarket, 16));
        let mut rx = ing.subscribe(); // subscribe BEFORE run so nothing is missed
        let bg = ing.clone();
        let handle = tokio::spawn(async move { bg.run(&[Symbol::from("BTC/USD")]).await });

        for expected in [1.0, 2.0, 3.0] {
            let got = rx.recv().await.expect("tick");
            assert_eq!(got.price, expected);
        }
        handle.abort();
    }
}
