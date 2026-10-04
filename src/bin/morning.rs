//! `morning` — the daily report: regime line, screener picks, each pick's earnings
//! date, recent headlines and an LLM note, sector and theme roll-ups, and the
//! scorecard of every past report (#64, #61, #67, #66, #35, #79).
//!
//!     cargo run --release --bin morning                 # writes data/reports/<date>.json + latest.json
//!     cargo run --release --bin morning -- --no-llm     # same report, commentary left empty, costs nothing
//!     cargo run --release --bin morning -- --date 2026-06-02 --no-llm   # replay that morning from the bar files
//!     cargo run --release --bin morning -- --replay-from 2026-07-01     # one report per session since then, then today's
//!     cargo run --release --bin morning -- --top 10 --json
//!
//! `--date` is a true replay: the screen and the regime see only bars before that
//! day, headlines come from the three days before it, and the earnings calendar —
//! which is forward-only — is skipped with a warning for past dates. Replays use
//! today's universe and today's adjusted prices (survivorship bias; see #79).
//!
//! `--replay-from` writes one report per trading session from that date to
//! yesterday (no LLM, no news: the scorecard needs picks, not prose), then today's
//! report, whose scorecard now has months of history behind it on day one.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context as _;
use chrono::{Duration, NaiveDate, Utc};
use clap::Parser;
use futures::future::join_all;

use alpaca_trading_system::domain::config::Config;
use alpaca_trading_system::domain::report::{
    MorningReport, Pick, RunStats, Scorecard, Scored, WatchItem,
};
use alpaca_trading_system::domain::types::{EarningsEvent, Headline, Symbol, Timeframe};
use alpaca_trading_system::domain::universe::{Instrument, Universe};
use alpaca_trading_system::engine::agents::commentator::{self, Commentator};
use alpaca_trading_system::engine::screener::{self, ScreenerConfig};
use alpaca_trading_system::engine::{regime, scorecard};
use alpaca_trading_system::infra::{
    llm, AlpacaBars, AlpacaClient, AlpacaNews, AnthropicClient, BarFileStore, Claude,
    FinnhubClient, LocalBars, ReportStore,
};
use alpaca_trading_system::traits::{BarSource, FundamentalSource, LlmClient, NewsSource};

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
    /// Regime index and scorecard benchmark.
    #[arg(long, default_value = regime::DEFAULT_SYMBOL)]
    regime_symbol: String,
    /// The morning to produce the report for (UTC). Default today. A past date replays
    /// it from the bar files.
    #[arg(long)]
    date: Option<NaiveDate>,
    /// Write one report per trading session from this date to yesterday (no LLM, no
    /// news), then today's report with the scorecard over all of them.
    #[arg(long)]
    replay_from: Option<NaiveDate>,
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
    /// Skip the headlines.
    #[arg(long)]
    no_news: bool,
    /// Skip the scorecard.
    #[arg(long)]
    no_scorecard: bool,
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

/// Everything a report needs that does not change between dates.
struct Ctx {
    args: Args,
    universe: Universe,
    in_universe: HashSet<Symbol>,
    alpaca: Arc<AlpacaClient>,
    bars: Arc<LocalBars<AlpacaBars>>,
    finnhub_key: String,
    anthropic_key: String,
    today: NaiveDate,
}

/// What to leave out of one report.
#[derive(Clone, Copy)]
struct Skip {
    llm: bool,
    news: bool,
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
        AlpacaBars::new(alpaca.clone()),
    ));
    let ctx = Ctx {
        in_universe: universe.symbols().cloned().collect(),
        universe,
        alpaca,
        bars,
        finnhub_key: cfg.finnhub_key,
        anthropic_key: cfg.anthropic_key,
        today: Utc::now().date_naive(),
        args,
    };
    let store = ReportStore::new(&ctx.args.out);

    // Batch replay first: one report per session, picks only.
    if let Some(from) = ctx.args.replay_from {
        let sessions = trading_days(
            &*ctx.bars,
            &Symbol::from(ctx.args.regime_symbol.as_str()),
            from,
            ctx.today,
        )
        .await?;
        println!(
            "replay   {} sessions {} .. {} (no LLM, no news)",
            sessions.len(),
            from,
            ctx.today
        );
        let t0 = Instant::now();
        for day in &sessions {
            let r = build_report(
                &ctx,
                *day,
                Skip {
                    llm: true,
                    news: true,
                },
            )
            .await;
            store.write(&r)?;
            println!(
                "  {}  {:<60} {} skipped",
                r.date,
                r.picks
                    .iter()
                    .map(|p| p.scored.symbol.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
                r.skipped.len()
            );
        }
        println!("replay   done in {:.1}s\n", t0.elapsed().as_secs_f64());
    }

    // The report asked for (today unless --date), with the scorecard over the past.
    let date = ctx.args.date.unwrap_or(ctx.today);
    let mut report = build_report(
        &ctx,
        date,
        Skip {
            llm: ctx.args.no_llm,
            news: ctx.args.no_news,
        },
    )
    .await;
    if !ctx.args.no_scorecard {
        let past = load_reports(&store, date);
        if past.is_empty() {
            report
                .run
                .warnings
                .push("scorecard: no earlier reports to score".into());
        } else {
            let bench = Symbol::from(ctx.args.regime_symbol.as_str());
            report.scorecard = Some(scorecard::scorecard(&*ctx.bars, &past, &bench, date).await);
        }
    }
    let path = store.write(&report)?;
    print_summary(&report, &path);
    if ctx.args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(())
}

/// One report for the morning of `date`, from bars strictly before it.
async fn build_report(ctx: &Ctx, date: NaiveDate, skip: Skip) -> MorningReport {
    let t0 = Instant::now();
    let mut warnings: Vec<String> = Vec::new();
    let replay = date < ctx.today;
    // Exclusive end: bars strictly before this morning. For today that is "through
    // yesterday's close", the same rule LocalBars applies; for a past date it is the
    // replay boundary.
    let as_of = midnight_ms(date);
    if replay {
        warnings.push(format!(
            "replay of {date}: today's universe and adjusted prices (survivorship bias); earnings skipped"
        ));
    }

    // 1. Regime and screen share nothing: run them together.
    let regime_symbol = Symbol::from(ctx.args.regime_symbol.as_str());
    let screener_cfg = ScreenerConfig {
        top_n: ctx.args.top,
        bar_window: ctx.args.window,
        as_of: Some(as_of),
        ..Default::default()
    };
    let (regime, screen) = tokio::join!(
        regime::regime(
            &*ctx.bars,
            &regime_symbol,
            regime::DEFAULT_WINDOW,
            Some(as_of)
        ),
        screener::screen(ctx.bars.clone(), &ctx.universe, &screener_cfg),
    );
    let regime = regime.unwrap_or_else(|e| {
        warnings.push(format!("regime unavailable: {e}"));
        None
    });

    // Who gets headlines and a note: the rule's picks plus the owner's watchlist,
    // each name once. The watchlist is read from the day's full ranking so it also
    // carries where the rule put the name.
    let ranked_pos: HashMap<Symbol, usize> = screen
        .ranked
        .iter()
        .enumerate()
        .map(|(i, s)| (s.symbol.clone(), i))
        .collect();
    let watch_insts: Vec<&Instrument> = ctx.universe.watchlist().collect();
    let mut covered: Vec<Scored> = screen.picks.clone();
    for inst in &watch_insts {
        if let Some(&i) = ranked_pos.get(&inst.symbol) {
            if !covered.iter().any(|s| s.symbol == inst.symbol) {
                covered.push(screen.ranked[i].clone());
            }
        }
    }

    // 2. Earnings for the whole universe and headlines for every covered name,
    // together. Both windows hang off `as_of`, so a replay reads the news of that
    // morning.
    let news = AlpacaNews::new(ctx.alpaca.clone());
    let since = as_of - NEWS_LOOKBACK_HOURS * 3_600_000;
    let (earnings, headlines) = tokio::join!(
        async {
            if replay {
                Ok(Vec::new())
            } else {
                earnings_this_week(ctx.finnhub_key.clone(), date, &ctx.in_universe).await
            }
        },
        async {
            if skip.news {
                Vec::new()
            } else {
                join_all(covered.iter().map(|s| {
                    news.headlines_between(std::slice::from_ref(&s.symbol), since, as_of, 25)
                }))
                .await
            }
        },
    );
    let earnings = earnings.unwrap_or_else(|w| {
        warnings.push(w);
        Vec::new()
    });
    if skip.news {
        warnings.push("news skipped".into());
    }

    let mut heads_iter = headlines.into_iter();
    let mut by_symbol: HashMap<Symbol, Pick> = HashMap::new();
    let order: Vec<Symbol> = covered.iter().map(|s| s.symbol.clone()).collect();
    for scored in covered {
        let heads = match heads_iter.next() {
            Some(Ok(h)) => select_headlines(h),
            Some(Err(e)) => {
                warnings.push(format!("news for {}: {e}", scored.symbol));
                Vec::new()
            }
            None => Vec::new(),
        };
        by_symbol.insert(
            scored.symbol.clone(),
            Pick {
                earnings: earnings.iter().find(|e| e.symbol == scored.symbol).cloned(),
                headlines: heads,
                commentary: None,
                scored,
            },
        );
    }

    // 3. One LLM note per covered name. Soft: no key or a vendor outage leaves
    // `commentary` empty and says so; the rest of the report is unaffected.
    let mut llm_stats = RunStats::default();
    if skip.llm {
        warnings.push("commentary skipped".into());
    } else if ctx.anthropic_key.trim().is_empty() {
        warnings.push("commentary skipped: ANTHROPIC_API_KEY missing".into());
    } else {
        let claude = Claude::new(
            Arc::new(AnthropicClient::new(ctx.anthropic_key.clone())),
            &ctx.args.model,
        );
        let agent = Commentator::new(&claude)
            .with_cache(&ctx.args.llm_cache)
            .with_concurrency(ctx.args.llm_concurrency);
        let notes = {
            let inputs: Vec<commentator::Input<'_>> = order
                .iter()
                .map(|sym| &by_symbol[sym])
                .map(|p| commentator::Input {
                    date,
                    scored: &p.scored,
                    earnings: p.earnings.as_ref(),
                    headlines: &p.headlines,
                    regime: regime.as_ref(),
                })
                .collect();
            agent.comment_all(&inputs).await
        };
        for (sym, note) in order.iter().zip(notes) {
            if let Some(p) = by_symbol.get_mut(sym) {
                p.commentary = note;
            }
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
            warnings.push(format!("commentary missing for {failed} name(s); see log"));
        }
    }

    let picks: Vec<Pick> = screen
        .picks
        .iter()
        .filter_map(|s| by_symbol.get(&s.symbol).cloned())
        .collect();
    let watchlist: Vec<WatchItem> = watch_insts
        .iter()
        .map(|inst| match ranked_pos.get(&inst.symbol) {
            Some(&i) => WatchItem {
                symbol: inst.symbol.clone(),
                name: inst.name.clone(),
                rank: Some(i + 1),
                of: screen.ranked.len(),
                pick: by_symbol.get(&inst.symbol).cloned(),
                unscored: None,
            },
            None => WatchItem {
                symbol: inst.symbol.clone(),
                name: inst.name.clone(),
                rank: None,
                of: screen.ranked.len(),
                pick: None,
                unscored: Some(
                    screen
                        .skipped
                        .iter()
                        .find(|k| k.symbol == inst.symbol)
                        .map(|k| k.reason.clone())
                        .unwrap_or_else(|| "not in the ranking".into()),
                ),
            },
        })
        .collect();

    MorningReport {
        date,
        generated_at: Utc::now().timestamp_millis(),
        regime,
        picks,
        watchlist,
        earnings_this_week: earnings,
        by_sector: screen.by_sector,
        by_theme: screen.by_theme,
        skipped: screen.skipped,
        universe_size: ctx.universe.len(),
        scorecard: None,
        run: RunStats {
            elapsed_ms: t0.elapsed().as_millis() as u64,
            warnings,
            ..llm_stats
        },
    }
}

/// Trading sessions in `[from, to)`: the benchmark's bar dates, since the index trades
/// every session and nothing else. A report for session `d` sees bars before `d`.
async fn trading_days<B: BarSource>(
    bars: &B,
    benchmark: &Symbol,
    from: NaiveDate,
    to: NaiveDate,
) -> anyhow::Result<Vec<NaiveDate>> {
    let series = bars
        .bars_range(
            benchmark,
            Timeframe::Day1,
            midnight_ms(from),
            midnight_ms(to),
        )
        .await
        .with_context(|| format!("sessions from {benchmark}"))?;
    Ok(series
        .iter()
        .filter_map(|b| chrono::DateTime::from_timestamp_millis(b.ts).map(|t| t.date_naive()))
        .collect())
}

/// Every stored report dated before `date`.
fn load_reports(store: &ReportStore, date: NaiveDate) -> Vec<MorningReport> {
    store
        .dates()
        .unwrap_or_default()
        .into_iter()
        .filter(|d| *d < date)
        .filter_map(|d| store.read(d).ok().flatten())
        .collect()
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

fn midnight_ms(d: NaiveDate) -> i64 {
    d.and_hms_opt(0, 0, 0)
        .expect("midnight")
        .and_utc()
        .timestamp_millis()
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
    if let Some(sc) = &r.scorecard {
        print_scorecard(sc);
    }
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
    if !r.watchlist.is_empty() {
        println!(
            "\nwatchlist (rank of {})",
            r.watchlist.first().map_or(0, |w| w.of)
        );
        for w in &r.watchlist {
            match (&w.pick, w.rank) {
                (Some(p), Some(rank)) => {
                    let s = &p.scored;
                    println!(
                        "  #{:<4} {:<7} {:<22} {:>6.2} {:<9} {:>8.2} {:>5.1}% {:>5.1}%  news {}",
                        rank,
                        s.symbol.as_str(),
                        trunc(&s.name, 22),
                        s.score,
                        format!("{:?}", s.rating),
                        s.close,
                        s.change_5d.map_or(0.0, |c| c * 100.0),
                        s.change_20d.map_or(0.0, |c| c * 100.0),
                        p.headlines.len()
                    );
                    if let Some(c) = &p.commentary {
                        println!("        [{:?}] {}", c.watch_reason, c.summary);
                    }
                }
                _ => println!(
                    "  -     {:<7} {}",
                    w.symbol.as_str(),
                    w.unscored.as_deref().unwrap_or("unscored")
                ),
            }
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

fn print_scorecard(sc: &Scorecard) {
    let span = sc
        .horizons
        .iter()
        .flat_map(|h| h.by_report.iter().map(|r| r.date))
        .fold(None::<(NaiveDate, NaiveDate)>, |acc, d| match acc {
            None => Some((d, d)),
            Some((a, b)) => Some((a.min(d), b.max(d))),
        });
    match span {
        Some((a, b)) => println!("scorecard vs {}  reports {a} .. {b}", sc.benchmark.as_str()),
        None => println!(
            "scorecard vs {}  (no completed horizons yet)",
            sc.benchmark.as_str()
        ),
    }
    for h in &sc.horizons {
        println!(
            "  {:>3}d  reports {:>3}  picks {:>4}  mean {:>+6.2}%  bench {:>+6.2}%  excess {:>+6.2}%  hit {:>4.0}%",
            h.horizon_days,
            h.reports,
            h.picks,
            h.mean_return * 100.0,
            h.mean_benchmark_return * 100.0,
            h.mean_excess * 100.0,
            h.hit_rate * 100.0
        );
    }
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
