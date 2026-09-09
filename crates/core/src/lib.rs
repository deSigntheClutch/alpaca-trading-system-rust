//! Pure domain + compute, shared by the wasm `web` UI and (future) native crates.
//! Dependency-light on purpose so it compiles for BOTH native and wasm — native
//! fetch (reqwest/DB) will live in a separate crate, not here.
//!
//! Modules added as we implement (DESIGN.md §8): a native marketdata/storage
//! crate for Alpaca + QuestDB/Scylla.

pub mod data;
pub mod layout;
pub mod signals;
