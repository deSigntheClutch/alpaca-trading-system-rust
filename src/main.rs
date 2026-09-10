//! `crypto-agent` — runs one decision cycle per configured symbol: fetch bars →
//! signal → analyst → decision → guards → (paper) execute, persisting every step to
//! ScyllaDB. Streaming ingest (`engine::ingest`) is wired + tested, ready to drive
//! intraday triggers in a later revision; this binary is the one-shot cycle.

use anyhow::Context as _;

use alpaca_trading_system::domain::config::Config;
use alpaca_trading_system::domain::types::Symbol;
use alpaca_trading_system::engine::pipeline::{Pipeline, PipelineConfig};
use alpaca_trading_system::infra::alpaca::AlpacaRest;
use alpaca_trading_system::infra::llm::{Claude, SONNET};
use alpaca_trading_system::infra::store::ScyllaStore;

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
    println!("data base  : {}", cfg.alpaca_data_base);
    println!("strategies : {}", cfg.strategies.len());

    // Missing creds → tell the user exactly what to provide, then stop (no crash).
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
        println!("\nmissing env: {missing:?}");
        println!("copy .env.example → .env, fill the keys (paper), and start Scylla:");
        println!("  docker compose up -d scylla && docker exec -i alpaca-scylla cqlsh < db/schema.cql");
        return Ok(());
    }

    // Scylla is required — a trading service must not run without its WAL. Fail loud.
    let scylla_host = std::env::var("SCYLLA_HOST").unwrap_or_else(|_| "127.0.0.1:9042".into());
    let fallback = std::env::var("FALLBACK_PATH").unwrap_or_else(|_| "fallback.jsonl".into());
    let store = ScyllaStore::connect(&scylla_host, fallback)
        .await
        .with_context(|| {
            format!("connect ScyllaDB at {scylla_host} — is it up? `docker compose up -d scylla` and apply db/schema.cql")
        })?;

    let replayed = store.replay_fallback().await?;
    if replayed > 0 {
        tracing::info!(replayed, "replayed buffered events from a prior outage");
    }

    // One HTTP client each, shared across symbols via the `&T` port impls.
    let alpaca = AlpacaRest::new(
        cfg.alpaca_key.clone(),
        cfg.alpaca_secret.clone(),
        cfg.alpaca_trade_base.clone(),
        cfg.alpaca_data_base.clone(),
    );
    let claude = Claude::new(cfg.anthropic_key.clone(), SONNET);

    let strat = cfg
        .strategies
        .first()
        .context("no strategies configured")?;
    println!("\nrunning strategy '{}' over {:?}\n", strat.id, strat.symbols);

    for sym in &strat.symbols {
        let symbol = Symbol::from(sym.as_str());
        let pipe = Pipeline::new(
            &alpaca,
            &claude,
            &store,
            &alpaca,
            PipelineConfig {
                strategy_id: strat.id.clone(),
                bar_window: 60,
                max_notional: cfg.max_notional,
            },
        );
        match pipe.run_symbol(&symbol).await {
            Ok(d) => println!(
                "  {symbol}: {:?} qty={} score={:.3} — {}",
                d.action, d.qty, d.score, d.reason
            ),
            Err(e) => tracing::error!(error = %e, %symbol, "cycle failed"),
        }
    }

    Ok(())
}
