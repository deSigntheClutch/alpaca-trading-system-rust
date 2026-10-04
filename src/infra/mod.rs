//! Infra: concrete implementations of the ports (`crate::traits`). The only layer
//! that touches the network or a database. Swap an impl without touching engine.
//!
//! Two layers:
//!   - `clients/` — one file per vendor API: auth, base URLs, the HTTP client and the
//!     vendor's wire types. Nothing domain-shaped. `AlpacaClient`, `AnthropicClient`,
//!     `BedrockClient`.
//!   - one file per port, named like its trait, holding every adapter for that port.
//!     An adapter borrows a client (`Arc`) and translates wire types into domain types:
//!     `market.rs` — `BarSource` / `MarketStream`: `AlpacaBars`, `LocalBars`, `CryptoWs`
//!     `broker.rs` — `Broker`: `AlpacaBroker`
//!     `store.rs`  — `Store`: `ScyllaStore`, `MemStore`
//!     `llm.rs`    — `LlmClient`: `Claude`, `Bedrock`
//!     `news.rs`   — `NewsSource`: `AlpacaNews`
//!   - `bar_store.rs` — `BarFileStore`, the JSONL files under `data/bars/` that the
//!     backfill writes and `LocalBars` (#47) will read. Not a port impl; the storage
//!     primitive both of those share.
//!
//! New port → a new file here (e.g. `fundamentals.rs` for `EarningsCalendar`).
//! New vendor → a new file under `clients/`.

pub mod bar_store;
pub mod broker;
pub mod clients;
pub mod llm;
pub mod market;
pub mod news;
pub mod store;

pub use bar_store::{Appended, BarFileStore, Meta};
pub use broker::AlpacaBroker;
pub use clients::{AlpacaClient, AnthropicClient, BedrockClient, FinnhubClient};
pub use llm::{Bedrock, Claude, BEDROCK_SONNET, SONNET};
pub use market::{AlpacaBars, CryptoWs, LocalBars};
pub use news::AlpacaNews;
pub use store::{MemStore, ScyllaStore};
