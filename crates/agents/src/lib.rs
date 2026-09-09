//! Multi-agent trading pipeline: Alpaca client, Claude client, the agents, and
//! the orchestrator that pipes them into a trade decision executed on paper.
//!
//! SCAFFOLD: types and signatures are the design. Bodies that need network/LLM
//! are `todo!()` until we implement (see DESIGN.md build order).

pub mod alpaca;
pub mod audit;
pub mod llm;
pub mod pipeline;

/// Loaded from env (`.env`): Alpaca paper keys + Anthropic key.
#[derive(Clone)]
pub struct Config {
    pub alpaca_key: String,
    pub alpaca_secret: String,
    pub anthropic_key: String,
    /// Alpaca base URL. Paper: https://paper-api.alpaca.markets
    pub alpaca_base: String,
    /// Require human approval before each order.
    pub human_in_loop: bool,
}

impl Config {
    /// Read from environment. Errors if a required key is missing.
    pub fn from_env() -> anyhow::Result<Self> {
        todo!("read ALPACA_KEY/ALPACA_SECRET/ANTHROPIC_API_KEY; default paper base + human_in_loop=true")
    }
}
