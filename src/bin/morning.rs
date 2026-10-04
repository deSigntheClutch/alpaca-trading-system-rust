//! `morning` — the daily report: regime line, screener picks, each pick's earnings
//! date, recent headlines and an LLM note, sector and theme roll-ups (#67, #66). The
//! scorecard arrives with #35, the step-by-step hardening with #68.
//!
//!     cargo run --release --bin morning                 # writes data/reports/<date>.json + latest.json
//!     cargo run --release --bin morning -- --no-llm     # same report, commentary left empty, costs nothing
//!     cargo run --release --bin morning -- --top 10 --json
//!
//! Reads bars through `LocalBars`, so run `backfill --universe config/universe.csv`
//! once first; after that a run is a second or two plus the earnings and news calls.
//! Earnings and news fail soft: a missing key or a vendor error becomes a line in
//! `run.warnings`, never a missing report.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context as _;
use chrono::{Duration, NaiveDate, Utc};
use clap::Parser;
use futures::future::join_all;

use alpaca_trading_system::domain::config::Config;
use alpaca_trading_system::domain::report::{MorningReport, Pick, RunStats};
use alpaca_trading_system::domain::types::{EarningsEvent, Headline, Symbol};
use alpaca_trading_system::domain::universe::Universe;
use alpaca_trading_system::engine::agents::commentator::{self, Commentator};
use alpaca_trading_system::engine::regime;
use alpaca_trading_system::engine::screener::{self, ScreenerConfig};
use alpaca_trading_system::infra::{
    llm, AlpacaBars, AlpacaClient, AlpacaNews, AnthropicClient, BarFileStore, Claude,
    FinnhubClient, LocalBars, ReportStore,
};
use alpaca_trading_system::traits::{FundamentalSource, LlmClient, NewsSource};
use std::sync::atomic::Ordering;

/// Headlines kept per pick, and the look-back. 72 hours, not 24: the report runs
/// before the open, and on a Monday (or the day after a holiday) the last 24 hours
/// hold nothing — the feed showed zero items for every pick on a Saturday morning.
/// Three days always reaches the previous session; "newest first, five per name"
/// keeps a weekday report from filling with stale items.
const HEADLINES_PER_PICK: usize = 5;
const NEWS_LOOKBACK_HOURS: i64 = 72;
/// A headline tagged with more symbols than this is a market wrap, not news about
/// the name; it only fills in when nothing more specific exists.
const SPECIFIC_MAX_SYMBOLS: usize = 3;

#[derive(Parser, Debug)]
#[command(about = "Score the universe and write today's morning report")]
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
    /// Report date (UTC). Default today.
    #[arg(long)]
    date: Option<NaiveDate>,
    #[arg(long, default_value = "data/bars")]
    bars: PathBuf,
    #[arg(long, default_value = "data/reports")]
    out: PathBuf,
    /// Print the report as JSON to stdout as well.
    #[arg(long)]
    json: bool,

    /// Skip the LLM notes. The report is complete otherwise and the run costs nothing.
    #[arg(long)]
    no_llm: bool,
    /// Anthropic model for the notes.
    #[arg(long, default_value = llm::SONNET)]
    model: String,
    /// LLM notes requested at once.
    #[arg(long, default_value_t = 4)]
    llm_concurrency: usize,
    /// On-disk cache so a same-day rerun makes no LLM calls.
    #[arg(long, default_value = "data/llm_cache")]
    llm_cache: PathBuf,
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
    let t0 = Instant::now();
    let mut warnings: Vec<String> = Vec::new();

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
        AlpacaBars::new(alpaca.clone()),
    ));
    let date = args.date.unwrap_or_else(|| Utc::now().date_naive());

    // 1. Regime and screen share nothing: run them together.
    let regime_symbol = Symbol::from(args.regime_symbol.as_str());
    let screener_cfg = ScreenerConfig {
        top_n: args.top,
        bar_window: args.window,
        ..Default::default()
    };
    let (regime, screen) = tokio::join!(
        regime::regime(&*bars, &regime_symbol, regime::DEFAULT_WINDOW),
        screener::screen(bars.clone(), &universe, &screener_cfg),
    );
    let regime = match regime {
        Ok(r) => r,
        Err(e) => {
            warnings.push(format!("regime unavailable: {e}"));
            None
        }
    };

    // 2. Earnings for the whole universe and headlines for the picks, together.
    let in_universe: HashSet<Symbol> = universe.symbols().cloned().collect();
    let news = AlpacaNews::new(alpaca.clone());
    let since = Utc::now().timestamp_millis() - NEWS_LOOKBACK_HOURS * 3_600_000;
    let (earnings, headlines) = tokio::join!(
        earnings_this_week(cfg.finnhub_key.clone(), date, &in_universe),
        join_all(screen.picks.iter().map(|p| news.headlines(
            std::slice::from_ref(&p.symbol),
            since,
            25
        ))),
    );
    let earnings = earnings.unwrap_or_else(|w| {
        warnings.push(w);
        Vec::new()
    });

    let mut picks: Vec<Pick> = screen
        .picks
        .iter()
        .cloned()
        .zip(headlines)
        .map(|(scored, heads)| {
            let heads = match heads {
                Ok(h) => select_headlines(h),
                Err(e) => {
                    warnings.push(format!("news for {}: {e}", scored.symbol));
                    Vec::new()
                }
            };
            Pick {
                earnings: earnings.iter().find(|e| e.symbol == scored.symbol).cloned(),
                headlines: heads,
                commentary: None,
                scored,
            }
        })
        .collect();

    // 3. One LLM note per pick. Soft: no key or a vendor outage leaves `commentary`
    // empty and says so; the rest of the report is unaffected.
    let mut llm_stats = RunStats::default();
    if args.no_llm {
        warnings.push("commentary skipped: --no-llm".into());
    } else if cfg.anthropic_key.trim().is_empty() {
        warnings.push("commentary skipped: ANTHROPIC_API_KEY missing".into());
    } else {
        let claude = Claude::new(
            Arc::new(AnthropicClient::new(cfg.anthropic_key.clone())),
            &args.model,
        );
        let agent = Commentator::new(&claude)
            .with_cache(&args.llm_cache)
            .with_concurrency(args.llm_concurrency);
        let inputs: Vec<commentator::Input<'_>> = picks
            .iter()
            .map(|p| commentator::Input {
                date,
                scored: &p.scored,
                earnings: p.earnings.as_ref(),
                headlines: &p.headlines,
                regime: regime.as_ref(),
            })
            .collect();
        let notes = agent.comment_all(&inputs).await;
        for (pick, note) in picks.iter_mut().zip(notes) {
            pick.commentary = note;
        }
        let u = &agent.usage;
        llm_stats.llm_calls = u.calls.load(Ordering::Relaxed) as u32;
        llm_stats.llm_cached = u.cached.load(Ordering::Relaxed) as u32;
        llm_stats.llm_input_tokens = u.input_tokens.load(Ordering::Relaxed);
        llm_stats.llm_output_tokens = u.output_tokens.load(Ordering::Relaxed);
        llm_stats.llm_cost_usd = llm::cost_usd(
            claude.model_id(),
            llm_stats.llm_input_tokens,
            llm_stats.llm_output_tokens,
        )
        .unwrap_or(0.0);
        let failed = u.failed.load(Ordering::Relaxed);
        if failed > 0 {
            warnings.push(format!("commentary missing for {failed} pick(s); see log"));
        }
    }

    let report = MorningReport {
        date,
        generated_at: Utc::now().timestamp_millis(),
        regime,
        picks,
        earnings_this_week: earnings,
        by_sector: screen.by_sector,
        by_theme: screen.by_theme,
        skipped: screen.skipped,
        universe_size: universe.len(),
        run: RunStats {
            elapsed_ms: t0.elapsed().as_millis() as u64,
            warnings,
            ..llm_stats
        },
    };

    let path = ReportStore::new(&args.out).write(&report)?;
    print_summary(&report, &path);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(())
}

/// Universe names reporting in `[date, date + 7)`, soonest first. `Err` carries the
/// warning text; the caller decides that an empty section beats no report.
async fn earnings_this_week(
    key: String,
    date: NaiveDate,
    in_universe: &HashSet<Symbol>,
) -> Result<Vec<EarningsEvent>, String> {
    let finnhub = FinnhubClient::new(key).map_err(|e| format!("earnings skipped: {e}"))?;
    let mut events: Vec<EarningsEvent> = finnhub
        .earnings_calendar(date, date + Duration::days(7))
        .await
        .map_err(|e| format!("earnings skipped: {e}"))?
        .into_iter()
        .filter(|e| in_universe.contains(&e.symbol))
        .collect();
    events.sort_by(|a, b| {
        a.date
            .cmp(&b.date)
            .then_with(|| a.symbol.as_str().cmp(b.symbol.as_str()))
    });
    Ok(events)
}

/// Prefer items about this name over market wraps that merely tag it, then newest
/// first, at most `HEADLINES_PER_PICK`.
fn select_headlines(mut heads: Vec<Headline>) -> Vec<Headline> {
    heads.sort_by(|a, b| {
        let (sa, sb) = (
            a.symbols.len() <= SPECIFIC_MAX_SYMBOLS,
            b.symbols.len() <= SPECIFIC_MAX_SYMBOLS,
        );
        sb.cmp(&sa).then_with(|| b.ts.cmp(&a.ts))
    });
    heads.truncate(HEADLINES_PER_PICK);
    heads
}

fn print_summary(r: &MorningReport, path: &std::path::Path) {
    match &r.regime {
        Some(g) => println!(
            "regime   {} {} {}d SMA  close {:.2}  sma {:.2}  {:+.1}%",
            g.symbol.as_str(),
            if g.above { "ABOVE" } else { "BELOW" },
            g.window,
            g.close,
            g.sma,
            g.distance_pct
        ),
        None => println!("regime   unavailable"),
    }
    println!(
        "screen   {} picks of {} names · {} skipped · {} earnings this week · {} ms",
        r.picks.len(),
        r.universe_size,
        r.skipped.len(),
        r.earnings_this_week.len(),
        r.run.elapsed_ms
    );
    println!();
    println!(
        "{:<7} {:<22} {:<10} {:>6} {:<9} {:>8} {:>6} {:>6}  {:<10} news",
        "sym", "name", "sector", "score", "rating", "close", "5d", "20d", "earnings"
    );
    for p in &r.picks {
        let s = &p.scored;
        println!(
            "{:<7} {:<22} {:<10} {:>6.2} {:<9} {:>8.2} {:>5.1}% {:>5.1}%  {:<10} {}",
            s.symbol.as_str(),
            trunc(&s.name, 22),
            trunc(&s.sector, 10),
            s.score,
            format!("{:?}", s.rating),
            s.close,
            s.change_5d.map_or(0.0, |c| c * 100.0),
            s.change_20d.map_or(0.0, |c| c * 100.0),
            p.earnings
                .as_ref()
                .map_or("-".to_string(), |e| e.date.to_string()),
            p.headlines.len(),
        );
        if let Some(c) = &p.commentary {
            println!("        [{:?}] {}", c.watch_reason, c.summary);
            println!("        risk: {}", c.risk);
        }
    }
    if r.run.llm_calls + r.run.llm_cached > 0 {
        println!(
            "\nllm      {} calls · {} cached · {} in / {} out tokens · ${:.4}",
            r.run.llm_calls,
            r.run.llm_cached,
            r.run.llm_input_tokens,
            r.run.llm_output_tokens,
            r.run.llm_cost_usd
        );
    }
    if !r.earnings_this_week.is_empty() {
        println!("\nearnings this week");
        for e in r.earnings_this_week.iter().take(12) {
            println!(
                "  {} {:<7} {:?}",
                e.date,
                e.symbol.as_str(),
                e.earnings_hour
            );
        }
    }
    for w in &r.run.warnings {
        println!("warning  {w}");
    }
    println!("\nwrote {}", path.display());
}

fn trunc(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(id: u64, ts: i64, n_syms: usize) -> Headline {
        Headline {
            id,
            ts,
            headline: format!("h{id}"),
            source: "t".into(),
            url: None,
            symbols: (0..n_syms)
                .map(|i| Symbol::from(format!("S{i}").as_str()))
                .collect(),
        }
    }

    #[test]
    fn specific_headlines_beat_market_wraps_then_newest_first() {
        let got = select_headlines(vec![
            h(1, 500, 12),
            h(2, 100, 1),
            h(3, 300, 3),
            h(4, 400, 8),
            h(5, 200, 2),
            h(6, 50, 1),
            h(7, 10, 1),
        ]);
        let ids: Vec<u64> = got.iter().map(|x| x.id).collect();
        assert_eq!(
            ids,
            vec![3, 5, 2, 6, 7],
            "five kept: the specific ones newest first; the wraps (1, 4) dropped"
        );
    }
}
