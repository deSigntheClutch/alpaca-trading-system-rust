//! `backfill` — pull history from Alpaca into the local bar store, one JSONL file per
//! symbol per timeframe under `data/bars/`. Incremental by default: a second run only
//! fetches what is newer than the last stored bar, so the daily report's refresh is a
//! few seconds, and a cold run on an empty store is one request per symbol.
//!
//!     cargo run --bin backfill -- NVDA MU BTC/USD --start 2020-01-01
//!     cargo run --bin backfill -- --universe config/universe.csv
//!     cargo run --bin backfill -- --universe config/universe.csv --timeframe 1Hour --start 2024-01-01
//!
//! Concurrency: one tokio task per symbol, `--concurrency` of them in flight, and a
//! pacer that spaces request starts so the run stays under Alpaca's 200 req/min.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use chrono::{NaiveDate, Utc};
use clap::Parser;
use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinSet;
use tokio::time::{Interval, MissedTickBehavior};

use alpaca_trading_system::domain::config::Config;
use alpaca_trading_system::domain::types::{Symbol, Timeframe};
use alpaca_trading_system::domain::universe::Universe;
use alpaca_trading_system::infra::{AlpacaBars, AlpacaClient, BarFileStore};
use alpaca_trading_system::traits::BarSource;

/// Minimum spacing between request starts. 350 ms ≈ 170 req/min, under the free
/// tier's 200 with room for the retry `send_json` makes on a 429.
const REQUEST_SPACING: Duration = Duration::from_millis(350);

#[derive(Parser, Debug)]
#[command(about = "Pull historical bars into data/bars/<timeframe>/<SYMBOL>.jsonl")]
struct Args {
    /// Symbols to fetch, e.g. NVDA MU BTC/USD. Omit to use --universe.
    #[arg(required_unless_present = "universe")]
    symbols: Vec<String>,

    /// Take the symbol list from a universe CSV (config/universe.csv).
    #[arg(long)]
    universe: Option<PathBuf>,

    /// Bar interval: 1Hour | 1Day | 1Week
    #[arg(long, default_value = "1Day", value_parser = parse_timeframe)]
    timeframe: Timeframe,

    /// First day to fetch (UTC). Ignored for a symbol that already has newer bars on
    /// disk unless --full is given.
    #[arg(long, default_value = "2016-01-01")]
    start: NaiveDate,

    /// Exclusive end day (UTC). Default is today, so yesterday is the last bar and
    /// today's unfinished bar never lands in the store.
    #[arg(long)]
    end: Option<NaiveDate>,

    /// Root of the bar store.
    #[arg(long, default_value = "data/bars")]
    out: PathBuf,

    /// Symbols fetched at the same time.
    #[arg(long, default_value_t = 4)]
    concurrency: usize,

    /// Drop what is on disk for each symbol and fetch the whole range again. Needed
    /// after a split: adjustment=all rewrites every historical price.
    #[arg(long)]
    full: bool,
}

fn parse_timeframe(s: &str) -> Result<Timeframe, String> {
    match s {
        "1Hour" => Ok(Timeframe::Hour1),
        "1Day" => Ok(Timeframe::Day1),
        "1Week" => Ok(Timeframe::Week1),
        other => Err(format!(
            "unknown timeframe {other:?}; expected 1Hour, 1Day or 1Week"
        )),
    }
}

fn midnight_ms(d: NaiveDate) -> i64 {
    d.and_hms_opt(0, 0, 0)
        .expect("midnight exists")
        .and_utc()
        .timestamp_millis()
}

fn day(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| ms.to_string())
}

/// Spaces request *starts* `REQUEST_SPACING` apart across all tasks while letting
/// responses overlap. `Interval` is not `Sync`, hence the mutex: lock, take a tick,
/// release — the lock is held for the wait, which is the point.
struct Pacer(Mutex<Interval>);

impl Pacer {
    fn new(spacing: Duration) -> Self {
        let mut iv = tokio::time::interval(spacing);
        // After a stall (a slow page), do not fire a burst of catch-up ticks.
        iv.set_missed_tick_behavior(MissedTickBehavior::Delay);
        Self(Mutex::new(iv))
    }

    async fn wait(&self) {
        self.0.lock().await.tick().await;
    }
}

/// One symbol's outcome, for the summary.
struct Done {
    symbol: Symbol,
    added: usize,
    span: Option<(i64, i64)>,
    skipped: Option<&'static str>,
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

    let symbols: Vec<Symbol> = match &args.universe {
        Some(path) => Universe::load(path)
            .with_context(|| format!("load universe {}", path.display()))?
            .symbols()
            .cloned()
            .collect(),
        None => args
            .symbols
            .iter()
            .map(|s| Symbol::from(s.as_str()))
            .collect(),
    };
    let end_day = args.end.unwrap_or_else(|| Utc::now().date_naive());
    let (start_ms, end_ms) = (midnight_ms(args.start), midnight_ms(end_day));
    anyhow::ensure!(
        start_ms < end_ms,
        "--start must be before --end ({} .. {end_day})",
        args.start
    );

    let cfg = Config::load("config/strategies.toml")?;
    let bars = Arc::new(AlpacaBars::new(Arc::new(AlpacaClient::new(
        cfg.alpaca_key,
        cfg.alpaca_secret,
        cfg.alpaca_trade_base,
        cfg.alpaca_data_base,
    ))));
    let store = Arc::new(BarFileStore::new(args.out.clone()));
    let gate = Arc::new(Semaphore::new(args.concurrency.max(1)));
    let pacer = Arc::new(Pacer::new(REQUEST_SPACING));

    println!(
        "backfill {} symbols · {} · {} .. {} (exclusive) · → {}{}",
        symbols.len(),
        args.timeframe.as_str(),
        args.start,
        end_day,
        args.out.display(),
        if args.full { " · FULL" } else { "" }
    );
    let t0 = std::time::Instant::now();

    let mut set = JoinSet::new();
    for symbol in symbols {
        let (bars, store, gate, pacer) = (bars.clone(), store.clone(), gate.clone(), pacer.clone());
        let (tf, full) = (args.timeframe, args.full);
        set.spawn(async move {
            let _permit = gate.acquire().await.expect("semaphore open");
            if full {
                store.reset(&symbol, tf)?;
            }
            // Incremental: continue from the newest stored bar, never re-fetch history.
            let last = store.last_ts(&symbol, tf)?;
            let from = match last {
                Some(l) if l + 1 > start_ms => l + 1,
                _ => start_ms,
            };
            if from >= end_ms {
                // Stored through yesterday already; nothing to ask the vendor for.
                return Ok(Done {
                    symbol,
                    added: 0,
                    span: None,
                    skipped: Some("up to date (no request)"),
                });
            }
            pacer.wait().await;
            let fetched = bars
                .bars_range(&symbol, tf, from, end_ms)
                .await
                .with_context(|| format!("fetch {symbol}"))?;
            if fetched.is_empty() {
                // Asked, and nothing came back. With history on disk that is a weekend,
                // a holiday, or a run before today's bar is complete. With an empty
                // store it means the feed has no bars for this name at all — the four
                // OTC ADRs in the universe (BYDDY, NTDOY, POAHY, TCEHY) are not on the
                // free SIP feed — and the screener will have to skip it.
                let why = if last.is_none() {
                    "no bars on this feed"
                } else {
                    "up to date"
                };
                return Ok(Done {
                    symbol,
                    added: 0,
                    span: None,
                    skipped: Some(why),
                });
            }
            let a = store.append(&symbol, tf, &fetched)?;
            Ok::<Done, anyhow::Error>(Done {
                symbol,
                added: a.added,
                span: a.first_ts.zip(a.last_ts),
                skipped: None,
            })
        });
    }

    let (mut total, mut failures) = (0usize, Vec::new());
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(d)) => {
                total += d.added;
                match (d.skipped, d.span) {
                    (Some(why), _) => println!("  {:<8} {why}", d.symbol),
                    (None, Some((a, b))) => {
                        println!("  {:<8} +{:<5} {} .. {}", d.symbol, d.added, day(a), day(b))
                    }
                    (None, None) => println!("  {:<8} +0", d.symbol),
                }
            }
            Ok(Err(e)) => {
                println!("  FAILED   {e:#}");
                failures.push(format!("{e:#}"));
            }
            Err(join) => failures.push(format!("task panicked: {join}")),
        }
    }

    println!(
        "\n{total} bars added in {:.1}s · {} failed",
        t0.elapsed().as_secs_f64(),
        failures.len()
    );
    if !failures.is_empty() {
        // Non-zero so a scheduled run shows red; everything that succeeded is on disk
        // and a rerun is incremental, so retrying is cheap.
        anyhow::bail!("{} symbol(s) failed", failures.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeframe_parser_accepts_alpaca_spelling_only() {
        assert_eq!(parse_timeframe("1Day"), Ok(Timeframe::Day1));
        assert_eq!(parse_timeframe("1Hour"), Ok(Timeframe::Hour1));
        assert!(parse_timeframe("1Min").is_err());
        assert!(parse_timeframe("daily").is_err());
    }

    #[test]
    fn midnight_is_utc_and_day_round_trips() {
        let d = NaiveDate::from_ymd_opt(2026, 10, 3).unwrap();
        let ms = midnight_ms(d);
        assert_eq!(ms % 86_400_000, 0, "midnight UTC is a whole number of days");
        assert_eq!(day(ms), "2026-10-03");
    }
}
