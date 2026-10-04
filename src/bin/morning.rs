//! `morning` — the daily report, built up step by step. Today: regime line and the
//! screener (#61, #64), printed as a table or dumped as JSON. The earnings, news,
//! LLM commentary, scorecard and report file arrive with #65-#68.
//!
//!     cargo run --release --bin morning
//!     cargo run --release --bin morning -- --top 10 --json > screen.json
//!
//! Reads bars through `LocalBars`, so run `backfill --universe config/universe.csv`
//! once first; after that a run is a second or two and makes no requests.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;

use alpaca_trading_system::domain::config::Config;
use alpaca_trading_system::domain::types::Symbol;
use alpaca_trading_system::domain::universe::Universe;
use alpaca_trading_system::engine::regime;
use alpaca_trading_system::engine::screener::{self, ScreenerConfig};
use alpaca_trading_system::infra::{AlpacaBars, AlpacaClient, BarFileStore, LocalBars};

#[derive(Parser, Debug)]
#[command(about = "Score the universe and print today's candidates")]
struct Args {
    #[arg(long, default_value = "config/universe.csv")]
    universe: PathBuf,
    /// Buy candidates to list.
    #[arg(long, default_value_t = 8)]
    top: usize,
    /// Bars per symbol handed to the signal.
    #[arg(long, default_value_t = 60)]
    window: usize,
    /// Regime index.
    #[arg(long, default_value = regime::DEFAULT_SYMBOL)]
    regime_symbol: String,
    #[arg(long, default_value = "data/bars")]
    bars: PathBuf,
    /// Print the full `Screen` as JSON instead of the table.
    #[arg(long)]
    json: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();
    let args = Args::parse();

    let universe = Universe::load(&args.universe)
        .with_context(|| format!("load universe {}", args.universe.display()))?;
    let cfg = Config::load("config/strategies.toml")?;
    let alpaca = Arc::new(AlpacaClient::new(
        cfg.alpaca_key,
        cfg.alpaca_secret,
        cfg.alpaca_trade_base,
        cfg.alpaca_data_base,
    ));
    let bars = Arc::new(LocalBars::new(
        Arc::new(BarFileStore::new(args.bars.clone())),
        AlpacaBars::new(alpaca),
    ));

    let regime_symbol = Symbol::from(args.regime_symbol.as_str());
    let screener_cfg = ScreenerConfig {
        top_n: args.top,
        bar_window: args.window,
        ..Default::default()
    };
    // The regime read and the 165-name screen share nothing; run them together.
    let (regime, screen) = tokio::join!(
        regime::regime(&*bars, &regime_symbol, regime::DEFAULT_WINDOW),
        screener::screen(bars.clone(), &universe, &screener_cfg),
    );
    let regime = regime.with_context(|| format!("regime bars for {regime_symbol}"))?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({ "regime": regime, "screen": screen })
            )?
        );
        return Ok(());
    }

    match &regime {
        Some(r) => println!(
            "regime  {} {} 200d SMA  close {:.2}  sma {:.2}  {:+.1}%",
            r.symbol,
            if r.above { "ABOVE" } else { "BELOW" },
            r.close,
            r.sma,
            r.distance_pct
        ),
        None => println!(
            "regime  unavailable (fewer than {} bars for {regime_symbol})",
            regime::DEFAULT_WINDOW
        ),
    }
    println!(
        "screen  {} scored · {} skipped · {} ms · as of {}\n",
        screen.ranked.len(),
        screen.skipped.len(),
        screen.elapsed_ms,
        day(screen.as_of)
    );

    println!(
        "{:<7} {:<24} {:<10} {:>6} {:<10} {:>5} {:>9} {:>7} {:>7}",
        "sym", "name", "sector", "score", "rating", "rsi", "close", "5d", "20d"
    );
    for s in &screen.picks {
        println!(
            "{:<7} {:<24} {:<10} {:>6.2} {:<10} {:>5.0} {:>9.2} {:>6.1}% {:>6.1}%",
            s.symbol.as_str(),
            trunc(&s.name, 24),
            trunc(&s.sector, 10),
            s.score,
            format!("{:?}", s.rating),
            s.rsi,
            s.close,
            s.change_5d.map_or(0.0, |c| c * 100.0),
            s.change_20d.map_or(0.0, |c| c * 100.0),
        );
    }

    println!("\nby theme (mean score, n)");
    for a in screen.by_theme.iter().take(8) {
        println!("  {:<16} {:>+5.2}  {}", a.key, a.mean_score, a.count);
    }
    if !screen.skipped.is_empty() {
        println!("\nskipped");
        for k in &screen.skipped {
            println!("  {:<7} {}", k.symbol.as_str(), k.reason);
        }
    }
    Ok(())
}

fn trunc(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn day(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| ms.to_string())
}
