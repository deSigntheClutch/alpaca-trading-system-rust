//! Crypto streaming ingester. One owner task reads the WS, fans ticks out to
//! consumers (broadcast) and persists to the Store. tokio patterns land in Phase 2:
//!
//!   2.1 connect + read one symbol   2.2 select! + CancellationToken shutdown
//!   2.3 broadcast fan-out           2.4 exp-backoff reconnect + heartbeat timeout
//!   2.5 bounded channels / Lagged backpressure
#![allow(dead_code)] // Phase 0: fields wired but unused until Phase 2.

use crate::traits::{MarketStream, Store};

/// Owns the market stream and the store. Generic over both ports (no infra coupling).
pub struct Ingester<M: MarketStream, S: Store> {
    market: M,
    store: S,
}

impl<M: MarketStream, S: Store> Ingester<M, S> {
    pub fn new(market: M, store: S) -> Self {
        Self { market, store }
    }

    // Phase 2.2+: async fn run(&self, symbols, shutdown: CancellationToken)
    // owner task -> broadcast fan-out -> reconnect. See module docs.
}
