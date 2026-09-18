//! Data-access layer: the two `Store` impls.
//!   - `ScyllaStore` (`scylla.rs`) — ScyllaDB WAL + local-file fallback; the resident
//!     production store.
//!   - `MemStore`    (`mem.rs`)    — in-memory (+ optional JSONL log), for running
//!     without a DB.

mod mem;
mod scylla;

pub use mem::MemStore;
// `self::` disambiguates the child module from the extern crate `scylla`.
pub use self::scylla::ScyllaStore;
