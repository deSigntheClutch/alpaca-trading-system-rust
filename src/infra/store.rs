//! ScyllaDB store (impl #1 of `Store`) + local-file fallback. Writes go at QUORUM
//! (the WAL row must survive a node loss); on write failure we append to a local
//! file so an order is never placed with zero record.
#![allow(dead_code)] // Phase 0: fields wired but unused until Phase 1.

use async_trait::async_trait;

use crate::domain::error::StoreError;
use crate::traits::Store;
use crate::domain::types::{Bar, Symbol};

/// Scylla-backed store. Phase 1 adds a `CachingSession` + prepared statements.
pub struct ScyllaStore {
    contact_points: String,
    fallback_path: String,
}

impl ScyllaStore {
    /// Connect to the cluster. Phase 1: build the session, prepare statements.
    pub async fn connect(
        contact_points: String,
        fallback_path: String,
    ) -> Result<Self, StoreError> {
        // ponytail: no real session yet — Phase 1 wires CachingSession + QUORUM here.
        Ok(Self {
            contact_points,
            fallback_path,
        })
    }
}

#[async_trait]
impl Store for ScyllaStore {
    async fn append_event(
        &self,
        _strategy_id: &str,
        _symbol: &Symbol,
        _ts: i64,
        _step: &str,
        _decision_id: &str,
        _payload: &str,
    ) -> Result<(), StoreError> {
        todo!("Phase 1: prepared INSERT into trading.events at QUORUM; local-file fallback on err")
    }

    async fn write_bars(&self, _bars: &[Bar]) -> Result<(), StoreError> {
        todo!("Phase 1: batched INSERT into trading.market_data")
    }
}
