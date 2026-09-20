//! The seams (ports). `infra` provides impls; `engine` is generic over these and
//! never depends on `infra` — so the pipeline is testable with fakes.
//!
//! One file per port, mirroring `infra/`: `market` (stream + historical bars),
//! `broker` (orders + account), `store` (event log), `llm` (chat). Each file also
//! carries its `&T` forwarding impl so a shared client can back a generic `Pipeline`.

pub mod broker;
pub mod llm;
pub mod market;
pub mod store;

pub use broker::Broker;
pub use llm::LlmClient;
pub use market::{BarSource, MarketStream};
pub use store::Store;
