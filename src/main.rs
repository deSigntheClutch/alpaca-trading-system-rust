//! `crypto-agent` — runs one decision cycle per configured symbol: fetch bars →
//! signal → analyst → decision → guards → (paper) execute, persisting every step to
//! ScyllaDB. Streaming ingest (`engine::ingest`) is wired + tested, ready to drive
//! intraday triggers in a later revision; this binary is the one-shot cycle.

use anyhow::Context as _;

use alpaca_trading_system::domain::config::{Config, Strategy};
use alpaca_trading_system::domain::types::Symbol;
use alpaca_trading_system::engine::pipeline::{Pipeline, PipelineConfig};
use alpaca_trading_system::infra::alpaca::AlpacaRest;
use alpaca_trading_system::infra::llm::{Bedrock, Claude, BEDROCK_SONNET4, SONNET};
use alpaca_trading_system::infra::memstore::MemStore;
use alpaca_trading_system::infra::store::ScyllaStore;
use alpaca_trading_system::traits::{LlmClient, Store};
use rust_decimal::Decimal;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env into the process env (no-op if the file is absent). Real env vars win.
    let _ = dotenvy::dotenv();

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

    // LLM backend: LLM=bedrock uses Amazon Bedrock (bearer API key); default is Anthropic
    // direct. Each needs a different key, so the creds check depends on the choice.
    let use_bedrock = matches!(
        std::env::var("LLM").unwrap_or_default().as_str(),
        "bedrock" | "aws"
    );
    let bedrock_key = std::env::var("BEDROCK_API_KEY").unwrap_or_default();

    // Missing creds → tell the user exactly what to provide, then stop (no crash).
    let llm_cred = if use_bedrock {
        ("BEDROCK_API_KEY", &bedrock_key)
    } else {
        ("ANTHROPIC_API_KEY", &cfg.anthropic_key)
    };
    let missing: Vec<&str> = [
        ("ALPACA_KEY", &cfg.alpaca_key),
        ("ALPACA_SECRET", &cfg.alpaca_secret),
        llm_cred,
    ]
    .into_iter()
    .filter(|(_, v)| v.is_empty())
    .map(|(k, _)| k)
    .collect();
    if !missing.is_empty() {
        println!("\nmissing env: {missing:?}");
        println!("copy .env.example → .env and fill the keys (paper).");
        return Ok(());
    }

    // One HTTP client each, shared across symbols via the `&T` port impls.
    let alpaca = AlpacaRest::new(
        cfg.alpaca_key.clone(),
        cfg.alpaca_secret.clone(),
        cfg.alpaca_trade_base.clone(),
        cfg.alpaca_data_base.clone(),
    );

    let strat = cfg
        .strategies
        .first()
        .context("no strategies configured")?;
    println!("\nrunning strategy '{}' over {:?}\n", strat.id, strat.symbols);

    // Build the chosen LLM client, then hand off to the (generic) store+run path.
    if use_bedrock {
        let region = std::env::var("BEDROCK_REGION").unwrap_or_else(|_| "us-east-1".into());
        let model = std::env::var("BEDROCK_MODEL").unwrap_or_else(|_| BEDROCK_SONNET4.into());
        println!("llm        : bedrock ({region} / {model})");
        let llm = Bedrock::new(bedrock_key, region, model);
        run_with_store(&alpaca, &llm, strat, cfg.max_notional).await?;
    } else {
        println!("llm        : anthropic ({SONNET})");
        let llm = Claude::new(cfg.anthropic_key.clone(), SONNET);
        run_with_store(&alpaca, &llm, strat, cfg.max_notional).await?;
    }

    Ok(())
}

/// Pick the store backend, then run the strategy. Generic over the LLM so both backends
/// reuse this (and the STORE branch) unchanged.
async fn run_with_store<L: LlmClient>(
    alpaca: &AlpacaRest,
    llm: &L,
    strat: &Strategy,
    max_notional: Decimal,
) -> anyhow::Result<()> {
    // STORE=memory runs the decision + paper-order path with no external DB (no WAL
    // audit; events go to stdout). Default is Scylla — a real WAL for a trading service.
    match std::env::var("STORE").unwrap_or_default().as_str() {
        "memory" | "mem" | "none" => {
            println!("store      : in-memory (STORE=memory) — no WAL persistence\n");
            let store = MemStore::default();
            run_strategy(alpaca, llm, &store, strat, max_notional).await;
        }
        _ => {
            // Scylla is required — a trading service must not run without its WAL. Fail loud.
            let scylla_host =
                std::env::var("SCYLLA_HOST").unwrap_or_else(|_| "127.0.0.1:9042".into());
            let fallback = std::env::var("FALLBACK_PATH").unwrap_or_else(|_| "fallback.jsonl".into());
            let store = ScyllaStore::connect(&scylla_host, fallback)
                .await
                .with_context(|| {
                    format!("connect ScyllaDB at {scylla_host} — is it up? No Docker? Run with STORE=memory to skip it (no WAL), or start Scylla and apply db/schema.cql")
                })?;

            let replayed = store.replay_fallback().await?;
            if replayed > 0 {
                tracing::info!(replayed, "replayed buffered events from a prior outage");
            }
            run_strategy(alpaca, llm, &store, strat, max_notional).await;
        }
    }
    Ok(())
}

/// Run one decision cycle per symbol. Generic over the LLM and `Store` so the same loop
/// serves every backend (a shared `&L`/`&S` backs every symbol's pipeline).
async fn run_strategy<L: LlmClient, S: Store>(
    alpaca: &AlpacaRest,
    llm: &L,
    store: &S,
    strat: &Strategy,
    max_notional: Decimal,
) {
    for sym in &strat.symbols {
        let symbol = Symbol::from(sym.as_str());
        let pipe = Pipeline::new(
            alpaca,
            llm,
            store,
            alpaca,
            PipelineConfig {
                strategy_id: strat.id.clone(),
                bar_window: 60,
                max_notional,
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
}
