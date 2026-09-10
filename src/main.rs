//! `crypto-agent` — resident crypto trading service.
//!
//! Phase 0: load config, print the resolved strategies + base URLs, exit. The WS
//! ingester and agent pipeline wire in from Phase 2 on.

use alpaca_trading_system::domain::config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let path = std::env::var("STRATEGIES").unwrap_or_else(|_| "config/strategies.toml".into());
    let cfg = Config::load(&path)?;

    tracing::info!(base = %cfg.alpaca_trade_base, live = cfg.allow_live, "config loaded");
    println!("trade base : {}", cfg.alpaca_trade_base);
    println!("data ws    : {}", cfg.alpaca_data_ws);
    println!("strategies : {}", cfg.strategies.len());
    for s in &cfg.strategies {
        println!("  - {} [{:?}] {:?}", s.id, s.tempo, s.symbols);
    }

    let missing: Vec<&str> = [
        ("ALPACA_KEY", &cfg.alpaca_key),
        ("ALPACA_SECRET", &cfg.alpaca_secret),
        ("ANTHROPIC_API_KEY", &cfg.anthropic_key),
    ]
    .into_iter()
    .filter(|(_, v)| v.is_empty())
    .map(|(k, _)| k)
    .collect();
    if !missing.is_empty() {
        tracing::warn!(missing = ?missing, "env keys unset (ok for Phase 0 skeleton)");
    }

    println!("\nPhase 0 skeleton — streaming + pipeline arrive in later phases.");
    Ok(())
}
