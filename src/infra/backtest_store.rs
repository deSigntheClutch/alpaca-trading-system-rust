//! `BacktestStore` — one file per run as `data/backtests/<run_id>.json`, never
//! overwritten (#32, folded in from #41). A run is the unit of comparison: every
//! result in it shares a `run_id` and a `RunConfig` (arm, model, fees, window, git
//! sha), so "what changed between these two numbers" is always answerable from
//! disk. The dashboard lists runs newest first and serves one by id.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::domain::dto::{BacktestResult, RunSummary};

use super::report_store::{atomic_write, read_opt};

/// What one run file holds: the results plus when it was written.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: String,
    /// Epoch millis.
    pub created_at: i64,
    pub results: Vec<BacktestResult>,
}

impl RunRecord {
    pub fn summary(&self) -> Option<RunSummary> {
        self.results.first().map(|r| RunSummary {
            run_id: self.run_id.clone(),
            created_at: self.created_at,
            config: r.config.clone(),
        })
    }
}

pub struct BacktestStore {
    dir: PathBuf,
}

impl BacktestStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self, run_id: &str) -> PathBuf {
        self.dir.join(format!("{run_id}.json"))
    }

    /// Write one run. The `run_id` comes from the results, which all share it; an
    /// empty result list is an error rather than an empty file nobody can list.
    pub fn write(&self, results: &[BacktestResult]) -> std::io::Result<PathBuf> {
        let Some(first) = results.first() else {
            return Err(std::io::Error::other("a run needs at least one result"));
        };
        fs::create_dir_all(&self.dir)?;
        let record = RunRecord {
            run_id: first.run_id.clone(),
            created_at: chrono::Utc::now().timestamp_millis(),
            results: results.to_vec(),
        };
        let json = serde_json::to_vec(&record).map_err(std::io::Error::other)?;
        let path = self.path(&record.run_id);
        atomic_write(&path, &json)?;
        Ok(path)
    }

    pub fn read(&self, run_id: &str) -> std::io::Result<Option<RunRecord>> {
        // A run id is a file stem we chose; refuse anything that could leave the dir.
        if run_id.contains(['/', '\\', '.']) {
            return Ok(None);
        }
        read_opt(&self.path(run_id))
    }

    /// Every run on disk, newest first.
    pub fn list(&self) -> std::io::Result<Vec<RunSummary>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut runs: Vec<RunSummary> = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match read_opt::<RunRecord>(&path) {
                Ok(Some(r)) => runs.extend(r.summary()),
                Ok(None) => {}
                Err(e) => tracing::warn!(path = %path.display(), error = %e, "skipping run file"),
            }
        }
        runs.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        Ok(runs)
    }

    pub fn latest(&self) -> std::io::Result<Option<RunRecord>> {
        match self.list()?.first() {
            Some(s) => self.read(&s.run_id),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::dto::{Arm, Metrics, RunConfig};

    fn result(run_id: &str, symbol: &str) -> BacktestResult {
        BacktestResult {
            run_id: run_id.into(),
            config: RunConfig {
                arm: Arm::Rule,
                model: String::new(),
                adapter: String::new(),
                fee_bps: 25.0,
                slippage_bps: 5.0,
                starting_cash: 10_000.0,
                window_start: "2026-01-02".into(),
                window_end: "2026-03-31".into(),
                symbols: vec![symbol.into()],
                context_symbols: vec![],
                git_sha: "abc".into(),
                error_days: 0,
            },
            symbol: symbol.into(),
            starting_cash: 10_000.0,
            days: vec![],
            trades: vec![],
            metrics: Metrics {
                total_return: 0.0,
                max_drawdown: 0.0,
                sharpe: 0.0,
                num_trades: 0,
                win_rate: 0.0,
                avg_win: 0.0,
                avg_loss: 0.0,
                profit_factor: None,
                exposure: 0.0,
                buy_hold_return: 0.0,
                total_fees: 0.0,
            },
        }
    }

    #[test]
    fn write_list_read_round_trip_newest_first() {
        let dir = std::env::temp_dir().join(format!("bt-store-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let store = BacktestStore::new(&dir);
        assert!(
            store.list().unwrap().is_empty(),
            "missing dir lists as empty"
        );

        store.write(&[result("run-a", "NVDA")]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        store
            .write(&[result("run-b", "MU"), result("run-b", "AMD")])
            .unwrap();

        let runs = store.list().unwrap();
        assert_eq!(
            runs.iter().map(|r| r.run_id.as_str()).collect::<Vec<_>>(),
            ["run-b", "run-a"]
        );
        assert_eq!(runs[0].config.symbols, ["MU"]);
        let b = store.read("run-b").unwrap().unwrap();
        assert_eq!(b.results.len(), 2);
        assert_eq!(store.latest().unwrap().unwrap().run_id, "run-b");
        assert!(store.read("../etc/passwd").unwrap().is_none());
        assert!(store.write(&[]).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
