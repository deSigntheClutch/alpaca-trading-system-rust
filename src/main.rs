//! `crypto-agent` — runs one decision cycle per configured symbol: fetch bars →
//! signal → analyst → decision → guards → (paper) execute, persisting every step to
//! ScyllaDB. Streaming ingest (`engine::ingest`) is wired + tested, ready to drive
//! intraday triggers in a later revision; this binary is the one-shot cycle.

use anyhow::Context as _;

use alpaca_trading_system::domain::config::{Config, Strategy};
use alpaca_trading_system::domain::dto::Arm;
use alpaca_trading_system::domain::types::{Side, Symbol};
use alpaca_trading_system::engine::backtest::{self, BacktestConfig, RunStamp};
use alpaca_trading_system::engine::pipeline::{Pipeline, PipelineConfig};
use alpaca_trading_system::infra::{
    AlpacaBars, AlpacaBroker, AlpacaClient, AnthropicClient, BacktestStore, BarFileStore, Bedrock,
    BedrockClient, Claude, LocalBars, MemStore, NoLlm, ScyllaStore, BEDROCK_SONNET, SONNET,
};
use alpaca_trading_system::traits::{BarSource, Broker, LlmClient, Store};
use rust_decimal::Decimal;
use std::sync::Arc;

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

    // ARM picks the decision function: rule (default, no model calls), llm, llm+debate.
    let arm: Arm = match std::env::var("ARM") {
        Ok(s) => s.parse().map_err(|e: String| anyhow::anyhow!(e))?,
        Err(_) => Arm::Rule,
    };
    println!("arm        : {arm}");

    // Missing creds → tell the user exactly what to provide, then stop (no crash). The
    // rule arm needs no model key at all.
    let llm_cred = if use_bedrock {
        ("BEDROCK_API_KEY", &bedrock_key)
    } else {
        ("ANTHROPIC_API_KEY", &cfg.anthropic_key)
    };
    let mut required = vec![
        ("ALPACA_KEY", &cfg.alpaca_key),
        ("ALPACA_SECRET", &cfg.alpaca_secret),
    ];
    if arm != Arm::Rule {
        required.push(llm_cred);
    }
    let missing: Vec<&str> = required
        .into_iter()
        .filter(|(_, v)| v.is_empty())
        .map(|(k, _)| k)
        .collect();
    if !missing.is_empty() {
        println!("\nmissing env: {missing:?}");
        println!("copy .env.example → .env and fill the keys (paper).");
        return Ok(());
    }

    // One Alpaca client (credentials + HTTP pool), shared by the two adapters built on
    // it; the adapters are then shared across symbols via the `&T` port impls.
    let alpaca = Arc::new(AlpacaClient::new(
        cfg.alpaca_key.clone(),
        cfg.alpaca_secret.clone(),
        cfg.alpaca_trade_base.clone(),
        cfg.alpaca_data_base.clone(),
    ));
    // Bars come from the local JSONL store first; Alpaca is asked only for what the
    // files lack (cold symbol, newer bars, periodic refresh) and the answer is written
    // through. BAR_STORE points at the root the backfill writes to.
    let bar_root = std::env::var("BAR_STORE").unwrap_or_else(|_| "data/bars".into());
    let bars = LocalBars::new(
        Arc::new(BarFileStore::new(bar_root.clone())),
        AlpacaBars::new(alpaca.clone()),
    );
    println!("bars       : {bar_root} (local-first, Alpaca on miss)");
    let broker = AlpacaBroker::new(alpaca.clone());

    let strat = cfg.strategies.first().context("no strategies configured")?;
    println!(
        "\nrunning strategy '{}' over {:?}\n",
        strat.id, strat.symbols
    );

    // Verification switch — see `PipelineConfig::force_side`.
    let force_side = match std::env::var("FORCE_SIDE").unwrap_or_default().as_str() {
        "buy" | "BUY" => Some(Side::Buy),
        "sell" | "SELL" => Some(Side::Sell),
        "" => None,
        other => {
            println!("FORCE_SIDE='{other}' not recognized (use buy|sell) — ignoring");
            None
        }
    };
    if let Some(side) = force_side {
        println!("FORCE_SIDE : {side:?} — bypassing signal/LLM, capped at MAX_NOTIONAL");
    }

    // BACKTEST=1 replays the SAME pipeline over history (no live orders) and writes
    // backtest.jsonl for the dashboard, instead of running the live cycle.
    let backtest = matches!(std::env::var("BACKTEST").as_deref(), Ok("1") | Ok("true"));

    // Build the chosen LLM client (none for the rule arm), then hand off to the
    // backtest or live run path.
    let run = Run {
        strat,
        arm,
        max_notional: cfg.max_notional,
        force_side,
        backtest,
    };
    if arm == Arm::Rule {
        println!("llm        : none (rule arm)");
        run.dispatch(&bars, &broker, &NoLlm, "", "").await?;
    } else if use_bedrock {
        let region = std::env::var("BEDROCK_REGION").unwrap_or_else(|_| "us-east-1".into());
        let model = std::env::var("BEDROCK_MODEL").unwrap_or_else(|_| BEDROCK_SONNET.into());
        println!("llm        : bedrock ({region} / {model})");
        let llm = Bedrock::new(
            Arc::new(BedrockClient::new(bedrock_key, region)),
            model.clone(),
        );
        run.dispatch(&bars, &broker, &llm, &model, "bedrock")
            .await?;
    } else {
        println!("llm        : anthropic ({SONNET})");
        let llm = Claude::new(
            Arc::new(AnthropicClient::new(cfg.anthropic_key.clone())),
            SONNET,
        );
        run.dispatch(&bars, &broker, &llm, SONNET, "anthropic")
            .await?;
    }

    Ok(())
}

/// What `main` decided before it knew which LLM adapter it would build. One generic
/// `dispatch` serves every adapter (and `NoLlm`), so the backtest-vs-live fork is
/// written once.
struct Run<'a> {
    strat: &'a Strategy,
    arm: Arm,
    max_notional: Decimal,
    force_side: Option<Side>,
    backtest: bool,
}

impl Run<'_> {
    async fn dispatch<B: BarSource, K: Broker, L: LlmClient>(
        &self,
        bars: &B,
        broker: &K,
        llm: &L,
        model: &str,
        adapter: &str,
    ) -> anyhow::Result<()> {
        if self.backtest {
            run_backtest_all(
                bars,
                llm,
                self.strat,
                self.arm,
                model,
                adapter,
                self.max_notional,
            )
            .await
        } else {
            run_with_store(
                bars,
                broker,
                llm,
                self.strat,
                self.arm,
                self.max_notional,
                self.force_side,
            )
            .await
        }
    }
}

/// Walk-forward backtest each symbol over history and append one `BacktestResult`
/// Bull/bear debate rounds from `DEBATE_ROUNDS` (default 1; 0 disables the debate). Each
/// round is 2 extra LLM calls per decision, so it directly drives cost — hence env-tunable.
fn debate_rounds() -> usize {
    std::env::var("DEBATE_ROUNDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
}

/// A float knob from the environment, or `default` when unset or unparseable.
fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// One run over the strategy's symbols, written as `data/backtests/<run_id>.json`
/// (never overwritten) for the dashboard. Reuses the live pipeline via
/// `engine::backtest`; no live orders are placed. `BACKTEST_DAYS` (default 90) caps
/// how many recent days get a decision; the arm decides what each day costs.
async fn run_backtest_all<B: BarSource, L: LlmClient>(
    bars_src: &B,
    llm: &L,
    strat: &Strategy,
    arm: Arm,
    model: &str,
    adapter: &str,
    max_notional: Decimal,
) -> anyhow::Result<()> {
    let days: usize = std::env::var("BACKTEST_DAYS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(90);
    let store_dir = std::env::var("BACKTEST_STORE").unwrap_or_else(|_| "data/backtests".into());
    let fee_bps = env_f64("FEE_BPS", 25.0);
    let slippage_bps = env_f64("SLIPPAGE_BPS", 5.0);
    let rounds = debate_rounds();

    let calls = strat.symbols.len() * days * arm.calls_per_day(rounds);
    println!("BACKTEST   : {days} days/symbol · arm {arm} → {store_dir}/");
    println!(
        "           : fills at next open · fee {fee_bps} bps/side · slippage {slippage_bps} bps"
    );
    if calls > 0 {
        println!("           : ≈ {calls} LLM calls — this costs money\n");
    } else {
        println!("           : 0 LLM calls\n");
    }

    let cfg = BacktestConfig {
        arm,
        model: model.to_string(),
        adapter: adapter.to_string(),
        starting_cash: 10_000.0,
        max_notional,
        days,
        debate_rounds: rounds,
        fee_bps,
        slippage_bps,
    };
    let stamp = RunStamp::new(strat.symbols.clone(), Vec::new());
    let results = backtest::run_many(bars_src, llm, &cfg, &stamp)
        .await
        .context("fetch bars")?;

    for result in &results {
        let symbol = &result.symbol;
        let m = &result.metrics;
        println!(
            "  {symbol}: {} days · net {:+.2}% · sharpe {:.2} · maxDD {:.1}% · {} closed trades · win {:.0}% · exposure {:.0}% · fees ${:.2} · buy&hold {:+.2}%",
            result.days.len(),
            m.total_return * 100.0,
            m.sharpe,
            m.max_drawdown * 100.0,
            m.num_trades,
            m.win_rate * 100.0,
            m.exposure * 100.0,
            m.total_fees,
            m.buy_hold_return * 100.0,
        );
    }

    let path = BacktestStore::new(&store_dir)
        .write(&results)
        .context("write run")?;
    println!(
        "\nrun {} · git {} · wrote {} — view it with `cargo run --bin web`",
        stamp.run_id,
        backtest::git_sha(),
        path.display()
    );
    Ok(())
}

/// Pick the store backend, then run the strategy. Generic over the LLM so both backends
/// reuse this (and the STORE branch) unchanged.
async fn run_with_store<B: BarSource, K: Broker, L: LlmClient>(
    bars: &B,
    broker: &K,
    llm: &L,
    strat: &Strategy,
    arm: Arm,
    max_notional: Decimal,
    force_side: Option<Side>,
) -> anyhow::Result<()> {
    // STORE=memory runs the decision + paper-order path with no external DB (no WAL
    // audit; events go to stdout). Default is Scylla — a real WAL for a trading service.
    match std::env::var("STORE").unwrap_or_default().as_str() {
        "memory" | "mem" | "none" => {
            // Optional EVENTS_PATH: append every event as a JSON line for `jq` inspection —
            // the only way to see the decision trail without a DB.
            let events_path = std::env::var("EVENTS_PATH").ok().map(Into::into);
            match &events_path {
                Some(p) => println!("store      : in-memory (STORE=memory) — events → {p:?} (no WAL)\n"),
                None => println!("store      : in-memory (STORE=memory) — stdout only (set EVENTS_PATH to log)\n"),
            }
            let store = MemStore::new(events_path);
            run_strategy(
                bars,
                broker,
                llm,
                &store,
                strat,
                arm,
                max_notional,
                force_side,
            )
            .await;
        }
        _ => {
            // Scylla is required — a trading service must not run without its WAL. Fail loud.
            let scylla_host =
                std::env::var("SCYLLA_HOST").unwrap_or_else(|_| "127.0.0.1:9042".into());
            let fallback =
                std::env::var("FALLBACK_PATH").unwrap_or_else(|_| "fallback.jsonl".into());
            let store = ScyllaStore::connect(&scylla_host, fallback)
                .await
                .with_context(|| {
                    format!("connect ScyllaDB at {scylla_host} — is it up? No Docker? Run with STORE=memory to skip it (no WAL), or start Scylla and apply db/schema.cql")
                })?;

            let replayed = store.replay_fallback().await?;
            if replayed > 0 {
                tracing::info!(replayed, "replayed buffered events from a prior outage");
            }
            run_strategy(
                bars,
                broker,
                llm,
                &store,
                strat,
                arm,
                max_notional,
                force_side,
            )
            .await;
        }
    }
    Ok(())
}

/// Run one decision cycle per symbol. Generic over the LLM and `Store` so the same loop
/// serves every backend (a shared `&L`/`&S` backs every symbol's pipeline).
#[allow(clippy::too_many_arguments)]
async fn run_strategy<B: BarSource, K: Broker, L: LlmClient, S: Store>(
    bars: &B,
    broker: &K,
    llm: &L,
    store: &S,
    strat: &Strategy,
    arm: Arm,
    max_notional: Decimal,
    force_side: Option<Side>,
) {
    for sym in &strat.symbols {
        let symbol = Symbol::from(sym.as_str());
        let pipe = Pipeline::new(
            bars,
            llm,
            store,
            broker,
            PipelineConfig {
                strategy_id: strat.id.clone(),
                arm,
                bar_window: 60,
                max_notional,
                debate_rounds: debate_rounds(),
                force_side,
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
