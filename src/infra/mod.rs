//! Infra: concrete implementations of the ports (`crate::traits`). The only layer
//! that touches the network or a database. Swap an impl without touching engine.
//!
//! Grouped by concern:
//!   - `exchange` — Alpaca REST (`AlpacaRest`) + crypto WS (`CryptoWs`).
//!   - `dal`      — the `Store` impls: `ScyllaStore` (WAL) and `MemStore` (in-memory).
//!   - `llm`      — the `LlmClient` impls: `Claude` and `Bedrock`.

pub mod dal;
pub mod exchange;
pub mod llm;
