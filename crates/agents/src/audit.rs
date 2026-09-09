//! Append-only audit log: every agent output and every order, one JSON object
//! per line (JSONL). Immutable record for later analysis — no DB needed for v1.

use serde::Serialize;

/// Append one record to `path` as a JSON line. Creates the file if missing.
pub fn append<T: Serialize>(path: &str, record: &T) -> anyhow::Result<()> {
    todo!("open path append-mode, write serde_json::to_string(record) + newline")
}
