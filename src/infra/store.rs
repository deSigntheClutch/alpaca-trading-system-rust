//! ScyllaDB store (impl #1 of `Store`) + local-file fallback.
//!
//! The WAL write (`order_intent` / `order_result`) goes at **QUORUM** — it must
//! survive losing a node. If Scylla is unreachable we append the event as a JSON
//! line to a local file so an order is NEVER placed with zero record; that file is
//! replayed into Scylla on a later run (ponytail: replay job is Phase 4).

use std::fs::OpenOptions;
use std::io::Write as _;

use async_trait::async_trait;
use scylla::client::session::Session;
use scylla::client::session_builder::SessionBuilder;
use scylla::statement::prepared::PreparedStatement;
use scylla::statement::Consistency;
use scylla::value::CqlTimestamp;

use crate::domain::error::StoreError;
use crate::domain::types::{Bar, Symbol};
use crate::traits::Store;

const APPEND_EVENT_CQL: &str = "INSERT INTO trading.events \
     (strategy_id, symbol, ts, step, decision_id, payload) VALUES (?, ?, ?, ?, ?, ?)";

pub struct ScyllaStore {
    session: Session,
    append_event: PreparedStatement,
    fallback_path: String,
}

impl ScyllaStore {
    /// Connect, prepare statements, pin QUORUM on the WAL write. Fails loudly if
    /// the cluster is unreachable at startup — a resident service wants that.
    pub async fn connect(contact_point: &str, fallback_path: String) -> Result<Self, StoreError> {
        let session = SessionBuilder::new()
            .known_node(contact_point)
            .build()
            .await
            .map_err(|e| StoreError::Connect(e.to_string()))?;

        let mut append_event = session
            .prepare(APPEND_EVENT_CQL)
            .await
            .map_err(|e| StoreError::Connect(format!("prepare append_event: {e}")))?;
        append_event.set_consistency(Consistency::Quorum);

        Ok(Self {
            session,
            append_event,
            fallback_path,
        })
    }
}

#[async_trait]
impl Store for ScyllaStore {
    async fn append_event(
        &self,
        strategy_id: &str,
        symbol: &Symbol,
        ts: i64,
        step: &str,
        decision_id: &str,
        payload: &str,
    ) -> Result<(), StoreError> {
        let values = (
            strategy_id,
            symbol.as_str(),
            CqlTimestamp(ts),
            step,
            decision_id,
            payload,
        );
        match self.session.execute_unpaged(&self.append_event, values).await {
            Ok(_) => Ok(()),
            // Scylla unreachable / timed out: never drop the record — write it local.
            // Only a fallback failure too is a hard error (truly no record possible).
            Err(e) => {
                let line =
                    fallback_line(strategy_id, symbol.as_str(), ts, step, decision_id, payload);
                tracing::warn!(error = %e, step, "scylla write failed; wrote local fallback");
                append_fallback(&self.fallback_path, &line).map_err(|io| {
                    StoreError::Write(format!("scylla failed ({e}); fallback failed ({io})"))
                })
            }
        }
    }

    async fn write_bars(&self, _bars: &[Bar]) -> Result<(), StoreError> {
        todo!("Phase 2: batched INSERT into trading.market_data (CqlDate day bucket)")
    }
}

/// One append-only, self-describing JSON line for a failed event (for replay).
/// `payload` is already a JSON string; it's embedded as a string field.
fn fallback_line(
    strategy_id: &str,
    symbol: &str,
    ts: i64,
    step: &str,
    decision_id: &str,
    payload: &str,
) -> String {
    serde_json::json!({
        "strategy_id": strategy_id,
        "symbol": symbol,
        "ts": ts,
        "step": step,
        "decision_id": decision_id,
        "payload": payload,
    })
    .to_string()
}

fn append_fallback(path: &str, line: &str) -> std::io::Result<()> {
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{line}")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies the safety-critical path that runs without Scylla: on a write
    // failure we must leave a replayable, append-only record.
    #[test]
    fn fallback_writes_replayable_json_lines() {
        let path = std::env::temp_dir().join("alpaca_fallback_test.jsonl");
        let _ = std::fs::remove_file(&path);
        let p = path.to_string_lossy().into_owned();

        let line = fallback_line(
            "crypto-momentum",
            "BTC/USD",
            1_700_000_000_000,
            "order_intent",
            "crypto-momentum:BTC/USD:2026-09-10",
            r#"{"action":"BUY","qty":"0.01"}"#,
        );
        append_fallback(&p, &line).unwrap();
        append_fallback(&p, &line).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2, "append mode adds one line per call");

        let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v["symbol"], "BTC/USD");
        assert_eq!(v["step"], "order_intent");
        assert_eq!(v["ts"], 1_700_000_000_000_i64);

        let _ = std::fs::remove_file(&path);
    }

    // Real round-trip against a local Scylla. Setup:
    //   docker compose up -d scylla
    //   docker exec -i alpaca-scylla cqlsh < db/schema.cql
    // Run with: cargo test -- --ignored
    #[tokio::test]
    #[ignore = "needs a local Scylla node with db/schema.cql applied"]
    async fn scylla_append_round_trip() {
        let fb = std::env::temp_dir().join("alpaca_it_fallback.jsonl");
        let _ = std::fs::remove_file(&fb);

        let store = ScyllaStore::connect("127.0.0.1:9042", fb.to_string_lossy().into_owned())
            .await
            .expect("connect to local scylla");
        store
            .append_event(
                "it-test",
                &Symbol::from("BTC/USD"),
                1_700_000_000_000,
                "order_intent",
                "it-test:BTC/USD:2026-09-10",
                r#"{"ok":true}"#,
            )
            .await
            .expect("append_event");

        // The Scylla path must have been taken — a fallback file means the write failed.
        assert!(
            !fb.exists(),
            "fallback file exists => scylla write did not succeed"
        );
    }
}
