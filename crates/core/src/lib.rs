//! Pure domain + compute: deterministic indicators and the composite signal.
//! Dependency-free on purpose — the agents call these as tools so the LLM never
//! does arithmetic. Tests run with `cargo test -p alpaca-core`.

pub mod data;
pub mod signals;
