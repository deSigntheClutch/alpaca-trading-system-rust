//! Infra: concrete implementations of the ports (`crate::traits`). The only layer
//! that touches the network or a database. Swap an impl without touching engine.

pub mod alpaca; // REST (AlpacaRest) + crypto WS (CryptoWs)
pub mod llm; // Claude (Anthropic Messages API)
pub mod memstore; // MemStore (in-memory Store, for running without ScyllaDB)
pub mod store; // ScyllaStore (+ local-file fallback)
