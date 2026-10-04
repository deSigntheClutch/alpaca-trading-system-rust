//! `BarFileStore` — the local bar store's file half: one JSONL file per symbol per
//! timeframe under a root, `data/bars/1Day/NVDA.jsonl`, one `Bar` per line in
//! chronological order. Written by the backfill binary (#30), read by `LocalBars`
//! (#47). No database: daily data for a whole universe is a few hundred MB of text,
//! and a file per symbol is the same partitioning a `(symbol, timeframe)` key would
//! give in Scylla, so the move is mechanical if it ever happens.
//!
//! Blocking `std::fs` on purpose: a write is a few hundred KB at most and happens
//! once per symbol per run; wrapping that in `tokio::fs` buys nothing here.

use std::fs::{self, OpenOptions};
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::domain::types::{AssetClass, Bar, Symbol, Timeframe};

pub struct BarFileStore {
    root: PathBuf,
}

/// What one `append` did, for the caller's summary line.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Appended {
    pub added: usize,
    pub first_ts: Option<i64>,
    pub last_ts: Option<i64>,
}

/// Sidecar `<SYMBOL>.meta.json` next to the bars. `fetched_at` is when the file was
/// last (re)built from the vendor, which is what the periodic full refresh keys on:
/// a split rewrites every adjusted price in history, and nothing in the bars
/// themselves says so. `first_ts`/`last_ts` save re-reading the file to know its span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    /// Epoch millis of the last full write (first append after a reset).
    pub fetched_at: i64,
    /// `"all"` for stocks (splits and dividends folded in), `"none"` for crypto.
    pub adjustment: String,
    pub first_ts: i64,
    pub last_ts: i64,
}

impl BarFileStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// `root/<timeframe>/<SYMBOL>.jsonl`. The slash in a crypto pair would be a
    /// directory separator, so `BTC/USD` is stored as `BTC-USD.jsonl`.
    pub fn path(&self, symbol: &Symbol, timeframe: Timeframe) -> PathBuf {
        self.root
            .join(timeframe.as_str())
            .join(format!("{}.jsonl", file_stem(symbol)))
    }

    fn meta_path(&self, symbol: &Symbol, timeframe: Timeframe) -> PathBuf {
        self.root
            .join(timeframe.as_str())
            .join(format!("{}.meta.json", file_stem(symbol)))
    }

    /// The sidecar, `None` when there is none (pre-sidecar files included: treat as
    /// "unknown age", which the refresh policy reads as "refresh").
    pub fn meta(&self, symbol: &Symbol, timeframe: Timeframe) -> Option<Meta> {
        let text = fs::read_to_string(self.meta_path(symbol, timeframe)).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn write_meta(
        &self,
        symbol: &Symbol,
        timeframe: Timeframe,
        meta: &Meta,
    ) -> std::io::Result<()> {
        let path = self.meta_path(symbol, timeframe);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(
            path,
            serde_json::to_vec_pretty(meta).map_err(std::io::Error::other)?,
        )
    }

    /// Timestamp of the newest stored bar, `None` when there is no file or it is
    /// empty. Reads the whole file: it is small, and the last line is all we need.
    pub fn last_ts(&self, symbol: &Symbol, timeframe: Timeframe) -> std::io::Result<Option<i64>> {
        Ok(self.read_all(symbol, timeframe)?.last().map(|b| b.ts))
    }

    /// Timestamp of the oldest stored bar.
    pub fn first_ts(&self, symbol: &Symbol, timeframe: Timeframe) -> std::io::Result<Option<i64>> {
        Ok(self.read_all(symbol, timeframe)?.first().map(|b| b.ts))
    }

    /// Every stored bar, chronological. A line that does not parse is skipped with a
    /// warning rather than failing the whole symbol: one bad line is a bug to fix,
    /// not a reason to lose the other 2,500.
    pub fn read_all(&self, symbol: &Symbol, timeframe: Timeframe) -> std::io::Result<Vec<Bar>> {
        let path = self.path(symbol, timeframe);
        let file = match fs::File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut out = Vec::new();
        for (i, line) in BufReader::new(file).lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Bar>(&line) {
                Ok(b) => out.push(b),
                Err(e) => {
                    tracing::warn!(path = %path.display(), line = i + 1, error = %e, "skipping bad bar line")
                }
            }
        }
        Ok(out)
    }

    /// Append `bars` newer than what is already stored, in order, deduplicated on
    /// `ts`. Creates the directory and file on first use. Returns what was added.
    pub fn append(
        &self,
        symbol: &Symbol,
        timeframe: Timeframe,
        bars: &[Bar],
    ) -> std::io::Result<Appended> {
        let last = self.last_ts(symbol, timeframe)?;
        let fresh = newer_than(last, bars);
        if fresh.is_empty() {
            return Ok(Appended::default());
        }
        let path = self.path(symbol, timeframe);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
        for b in &fresh {
            serde_json::to_writer(&mut f, b).map_err(std::io::Error::other)?;
            f.write_all(b"\n")?;
        }
        f.flush()?;

        // Keep the sidecar current. A fresh file (no previous bars) starts the
        // `fetched_at` clock; an incremental append only moves `last_ts`.
        let now = now_ms();
        let meta = match (last, self.meta(symbol, timeframe)) {
            (Some(_), Some(mut m)) => {
                m.last_ts = fresh.last().map(|b| b.ts).unwrap_or(m.last_ts);
                m
            }
            _ => Meta {
                fetched_at: now,
                adjustment: match symbol.asset_class() {
                    AssetClass::Stock => "all".into(),
                    AssetClass::Crypto => "none".into(),
                },
                first_ts: fresh.first().map(|b| b.ts).unwrap_or(0),
                last_ts: fresh.last().map(|b| b.ts).unwrap_or(0),
            },
        };
        self.write_meta(symbol, timeframe, &meta)?;

        Ok(Appended {
            added: fresh.len(),
            first_ts: fresh.first().map(|b| b.ts),
            last_ts: fresh.last().map(|b| b.ts),
        })
    }

    /// Delete the file so the next `append` rewrites from scratch — after a split,
    /// when `adjustment=all` has changed every historical price (#47 tracks when).
    pub fn reset(&self, symbol: &Symbol, timeframe: Timeframe) -> std::io::Result<()> {
        for path in [
            self.path(symbol, timeframe),
            self.meta_path(symbol, timeframe),
        ] {
            match fs::remove_file(path) {
                Ok(()) | Err(_) => {}
            }
        }
        Ok(())
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn file_stem(symbol: &Symbol) -> String {
    symbol.as_str().replace('/', "-")
}

/// Bars strictly after `last`, chronological, one per `ts`. Pure so the merge rule is
/// testable without a filesystem.
fn newer_than(last: Option<i64>, bars: &[Bar]) -> Vec<Bar> {
    let mut v: Vec<Bar> = bars
        .iter()
        .filter(|b| last.is_none_or(|l| b.ts > l))
        .cloned()
        .collect();
    v.sort_by_key(|b| b.ts);
    v.dedup_by_key(|b| b.ts);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(ts: i64) -> Bar {
        Bar {
            symbol: Symbol::from("T"),
            ts,
            open: 1.0,
            high: 1.0,
            low: 1.0,
            close: 1.0,
            volume: 1.0,
        }
    }

    fn scratch() -> BarFileStore {
        let dir = std::env::temp_dir().join(format!(
            "bar_store_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        BarFileStore::new(dir)
    }

    #[test]
    fn crypto_pair_gets_a_safe_file_name() {
        let s = BarFileStore::new("data/bars");
        assert_eq!(
            s.path(&Symbol::from("BTC/USD"), Timeframe::Day1),
            PathBuf::from("data/bars/1Day/BTC-USD.jsonl")
        );
        assert_eq!(
            s.path(&Symbol::from("NVDA"), Timeframe::Hour1),
            PathBuf::from("data/bars/1Hour/NVDA.jsonl")
        );
    }

    #[test]
    fn newer_than_filters_sorts_and_dedups() {
        let got = newer_than(
            Some(200),
            &[bar(300), bar(100), bar(400), bar(300), bar(200)],
        );
        assert_eq!(got.iter().map(|b| b.ts).collect::<Vec<_>>(), vec![300, 400]);
        assert_eq!(newer_than(None, &[bar(2), bar(1)]).len(), 2);
    }

    #[test]
    fn append_then_incremental_append_round_trips() {
        let s = scratch();
        let sym = Symbol::from("NVDA");
        assert_eq!(
            s.last_ts(&sym, Timeframe::Day1).unwrap(),
            None,
            "empty store"
        );

        let a = s
            .append(&sym, Timeframe::Day1, &[bar(1), bar(2), bar(3)])
            .unwrap();
        assert_eq!(a.added, 3);
        assert_eq!((a.first_ts, a.last_ts), (Some(1), Some(3)));

        // Second run overlaps the first: only the genuinely new bar lands.
        let b = s
            .append(&sym, Timeframe::Day1, &[bar(2), bar(3), bar(4)])
            .unwrap();
        assert_eq!(b.added, 1);
        assert_eq!(s.last_ts(&sym, Timeframe::Day1).unwrap(), Some(4));

        let all = s.read_all(&sym, Timeframe::Day1).unwrap();
        assert_eq!(
            all.iter().map(|b| b.ts).collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );

        // Nothing new → nothing written, file untouched.
        assert_eq!(
            s.append(&sym, Timeframe::Day1, &[bar(4)]).unwrap(),
            Appended::default()
        );

        let m = s.meta(&sym, Timeframe::Day1).expect("sidecar written");
        assert_eq!(
            (m.first_ts, m.last_ts, m.adjustment.as_str()),
            (1, 4, "all")
        );
        assert!(m.fetched_at > 0);

        s.reset(&sym, Timeframe::Day1).unwrap();
        assert!(s.read_all(&sym, Timeframe::Day1).unwrap().is_empty());
        assert!(
            s.meta(&sym, Timeframe::Day1).is_none(),
            "reset drops the sidecar too"
        );
        s.reset(&sym, Timeframe::Day1).unwrap(); // idempotent
        let _ = fs::remove_dir_all(&s.root);
    }

    #[test]
    fn a_bad_line_is_skipped_not_fatal() {
        let s = scratch();
        let sym = Symbol::from("X");
        let path = s.path(&sym, Timeframe::Day1);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            format!(
                "{}\nnot json\n{}\n",
                serde_json::to_string(&bar(1)).unwrap(),
                serde_json::to_string(&bar(2)).unwrap()
            ),
        )
        .unwrap();
        let all = s.read_all(&sym, Timeframe::Day1).unwrap();
        assert_eq!(all.iter().map(|b| b.ts).collect::<Vec<_>>(), vec![1, 2]);
        let _ = fs::remove_dir_all(&s.root);
    }
}
