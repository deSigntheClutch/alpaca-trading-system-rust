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
use scylla::value::{CqlDate, CqlTimestamp};
use serde::Deserialize;

use crate::domain::error::StoreError;
use crate::domain::types::{Bar, Event, Symbol};
use crate::traits::Store;

const MS_PER_DAY: i64 = 86_400_000;

const APPEND_EVENT_CQL: &str = "INSERT INTO trading.events \
     (strategy_id, symbol, ts, step, decision_id, payload) VALUES (?, ?, ?, ?, ?, ?)";

const WRITE_BAR_CQL: &str = "INSERT INTO trading.market_data \
     (symbol, day, ts, open, high, low, close, volume) VALUES (?, ?, ?, ?, ?, ?, ?, ?)";

pub struct ScyllaStore {
    session: Session,
    append_event: PreparedStatement,
    write_bar: PreparedStatement,
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

        // Bars are re-fetchable (daily_bars) so they don't need the WAL guarantee —
        // ONE is fine (leave the driver default consistency).
        let write_bar = session
            .prepare(WRITE_BAR_CQL)
            .await
            .map_err(|e| StoreError::Connect(format!("prepare write_bar: {e}")))?;

        Ok(Self {
            session,
            append_event,
            write_bar,
            fallback_path,
        })
    }

    /// Replay locally-buffered events (from a prior Scylla outage) into the cluster.
    /// Successful rows are dropped from the file; anything that still fails is kept
    /// for the next attempt. Returns the number of events replayed. Phase 4a.
    pub async fn replay_fallback(&self) -> Result<usize, StoreError> {
        let content = match std::fs::read_to_string(&self.fallback_path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(StoreError::Query(e.to_string())),
        };

        let mut replayed = 0usize;
        let mut still_failing: Vec<String> = Vec::new();
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(rec) = serde_json::from_str::<FallbackRecord>(line) else {
                tracing::warn!(line, "undecodable fallback line; keeping for inspection");
                still_failing.push(line.to_string());
                continue;
            };
            let values = (
                rec.strategy_id.as_str(),
                rec.symbol.as_str(),
                CqlTimestamp(rec.ts),
                rec.step.as_str(),
                rec.decision_id.as_str(),
                rec.payload.as_str(),
            );
            match self
                .session
                .execute_unpaged(&self.append_event, values)
                .await
            {
                Ok(_) => replayed += 1,
                Err(_) => still_failing.push(line.to_string()),
            }
        }

        if still_failing.is_empty() {
            let _ = std::fs::remove_file(&self.fallback_path);
        } else {
            std::fs::write(&self.fallback_path, still_failing.join("\n") + "\n")
                .map_err(|e| StoreError::Write(e.to_string()))?;
        }
        Ok(replayed)
    }
}

/// The shape written by `fallback_line` — used to replay buffered events.
#[derive(Deserialize)]
struct FallbackRecord {
    strategy_id: String,
    symbol: String,
    ts: i64,
    step: String,
    decision_id: String,
    payload: String,
}

/// Unix epoch millis → Scylla `date` (days since epoch, offset by 2^31 per CQL).
fn cql_day(ts_ms: i64) -> CqlDate {
    let days = ts_ms.div_euclid(MS_PER_DAY);
    CqlDate(((1i64 << 31) + days) as u32)
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
        match self
            .session
            .execute_unpaged(&self.append_event, values)
            .await
        {
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

    async fn write_bars(&self, bars: &[Bar]) -> Result<(), StoreError> {
        // ponytail: sequential inserts — fine for daily-bar backfill (tens of rows).
        // Switch to concurrent execute or a per-partition BATCH if a high-rate tick
        // firehose ever lands here.
        for b in bars {
            let values = (
                b.symbol.as_str(),
                cql_day(b.ts),
                CqlTimestamp(b.ts),
                b.open,
                b.high,
                b.low,
                b.close,
                b.volume,
            );
            self.session
                .execute_unpaged(&self.write_bar, values)
                .await
                .map_err(|e| StoreError::Write(e.to_string()))?;
        }
        Ok(())
    }

    /// Single-partition read (PK is `(strategy_id, symbol)`), so no cross-partition scan.
    /// Not prepared — the learning loop runs it rarely, not on the decision hot path.
    /// Clustering is `ts DESC`; we sort back to oldest-first for the learning loop.
    async fn read_events(
        &self,
        strategy_id: &str,
        symbol: &Symbol,
    ) -> Result<Vec<Event>, StoreError> {
        const READ_EVENTS_CQL: &str = "SELECT ts, step, decision_id, payload \
             FROM trading.events WHERE strategy_id = ? AND symbol = ?";
        let result = self
            .session
            .query_unpaged(READ_EVENTS_CQL, (strategy_id, symbol.as_str()))
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;
        let rows = result
            .into_rows_result()
            .map_err(|e| StoreError::Query(e.to_string()))?;
        let mut out = Vec::new();
        for row in rows
            .rows::<(CqlTimestamp, String, String, String)>()
            .map_err(|e| StoreError::Query(e.to_string()))?
        {
            let (ts, step, decision_id, payload) =
                row.map_err(|e| StoreError::Query(e.to_string()))?;
            out.push(Event {
                ts: ts.0,
                step,
                decision_id,
                payload,
            });
        }
        out.sort_by_key(|e| e.ts); // stable → oldest-first
        Ok(out)
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

    // The replay path must be able to decode exactly what the write path emitted.
    #[test]
    fn fallback_line_round_trips_into_record() {
        let line = fallback_line(
            "s1",
            "BTC/USD",
            1_700_000_000_000,
            "order_intent",
            "s1:BTC/USD:d",
            r#"{"a":1}"#,
        );
        let rec: FallbackRecord = serde_json::from_str(&line).unwrap();
        assert_eq!(rec.strategy_id, "s1");
        assert_eq!(rec.symbol, "BTC/USD");
        assert_eq!(rec.ts, 1_700_000_000_000);
        assert_eq!(rec.step, "order_intent");
        assert_eq!(rec.payload, r#"{"a":1}"#);
    }

    #[test]
    fn cql_day_matches_cql_epoch_offset() {
        // Epoch itself is day 2^31; one day later is 2^31 + 1.
        assert_eq!(cql_day(0).0, 1 << 31);
        assert_eq!(cql_day(MS_PER_DAY).0, (1 << 31) + 1);
        // Any ts within a day maps to that day's bucket.
        assert_eq!(cql_day(MS_PER_DAY + 1).0, (1 << 31) + 1);
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
