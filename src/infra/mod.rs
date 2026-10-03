//! Infra: concrete implementations of the ports (`crate::traits`). The only layer
//! that touches the network or a database. Swap an impl without touching engine.
//!
//! Two layers:
//!   - `clients/` — one file per vendor API: auth, base URLs, the HTTP client and the
//!     vendor's wire types. Nothing domain-shaped. `AlpacaClient`, `AnthropicClient`,
//!     `BedrockClient`.
//!   - one file per port, named like its trait, holding every adapter for that port.
//!     An adapter borrows a client (`Arc`) and translates wire types into domain types:
//!     `market.rs` — `BarSource` / `MarketStream`: `AlpacaBars`, `CryptoWs`
//!     `broker.rs` — `Broker`: `AlpacaBroker`
//!     `store.rs`  — `Store`: `ScyllaStore`, `MemStore`
//!     `llm.rs`    — `LlmClient`: `Claude`, `Bedrock`
//!
//! New port → a new file here (e.g. `fundamentals.rs` for `EarningsCalendar`).
//! New vendor → a new file under `clients/`.

pub mod broker;
pub mod clients;
pub mod llm;
pub mod market;
pub mod store;

pub use broker::AlpacaBroker;
pub use clients::{AlpacaClient, AnthropicClient, BedrockClient};
pub use llm::{Bedrock, Claude, BEDROCK_SONNET, SONNET};
pub use market::{AlpacaBars, CryptoWs};
pub use store::{MemStore, ScyllaStore};
