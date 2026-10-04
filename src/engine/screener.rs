//! The screener: score every name in the universe with the existing composite signal,
//! rank, take the top N non-ETF names as the day's **buy candidates**, and roll the
//! scores up by sector and theme. This is the morning report's core (#64).
//!
//! The signal is `domain::signals::compute`, unchanged on purpose: the MVP ships on
//! the current rule and measures it (#35); improving the rule is #21/#59/#60.
//!
//! Concurrency: one tokio task per symbol in a `JoinSet`, a `Semaphore` capping how
//! many read bars at once. Reads come from `LocalBars` normally, so "concurrent" here
//! means file reads and JSON parsing; the cap matters on a cold store, where every
//! miss becomes an HTTP request. A symbol that fails never aborts the run: it lands
//! in `skipped` with a reason and the report says so.

use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use ts_rs::TS;

use crate::domain::signals::{self, Rating};
use crate::domain::types::{Symbol, Timeframe};
use crate::domain::universe::{Instrument, Universe};
use crate::traits::BarSource;

/// Fewest bars the signal needs (SMA20 plus the RSI warm-up).
pub const MIN_BARS: usize = 21;

#[derive(Debug, Clone)]
pub struct ScreenerConfig {
    /// How many non-ETF names make the picks list.
    pub top_n: usize,
    /// Bars handed to the signal per symbol.
    pub bar_window: usize,
    pub timeframe: Timeframe,
    /// Symbols read at the same time.
    pub concurrency: usize,
}

impl Default for ScreenerConfig {
    fn default() -> Self {
        Self {
            top_n: 8,
            bar_window: 60,
            timeframe: Timeframe::Day1,
            concurrency: 8,
        }
    }
}

/// One scored name: the instrument's tags plus the signal and a few price facts the
/// report shows next to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub struct Scored {
    #[ts(type = "string")]
    pub symbol: Symbol,
    pub name: String,
    pub sector: String,
    pub themes: Vec<String>,
    pub region: String,
    pub is_etf: bool,
    pub score: f64,
    pub rating: Rating,
    pub rsi: f64,
    pub close: f64,
    /// Fractional changes over the last 1, 5 and 20 bars; `None` when the series is
    /// too short for that look-back.
    pub change_1d: Option<f64>,
    pub change_5d: Option<f64>,
    pub change_20d: Option<f64>,
    /// Timestamp of the bar the score was computed on, epoch millis.
    pub as_of: i64,
    pub bars: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub struct Skipped {
    #[ts(type = "string")]
    pub symbol: Symbol,
    pub reason: String,
}

/// Mean score of a group, with its size so a mean of one name reads as what it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub struct Aggregate {
    pub key: String,
    pub count: usize,
    pub mean_score: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub struct Screen {
    /// Newest bar timestamp across all scored names, epoch millis.
    pub as_of: i64,
    /// Every scored name, ETFs included, best score first; ties broken by symbol.
    pub ranked: Vec<Scored>,
    /// Top `top_n` of `ranked` excluding ETFs: the buy candidates.
    pub picks: Vec<Scored>,
    pub by_sector: Vec<Aggregate>,
    pub by_theme: Vec<Aggregate>,
    pub skipped: Vec<Skipped>,
    pub elapsed_ms: u64,
}

/// Score the universe. Never fails as a whole: per-symbol problems go to `skipped`.
pub async fn screen<B>(bars: Arc<B>, universe: &Universe, cfg: &ScreenerConfig) -> Screen
where
    B: BarSource + 'static,
{
    let t0 = Instant::now();
    let gate = Arc::new(Semaphore::new(cfg.concurrency.max(1)));
    let mut set: JoinSet<Result<Scored, Skipped>> = JoinSet::new();

    for inst in universe.iter().cloned() {
        let (bars, gate) = (bars.clone(), gate.clone());
        let (tf, window) = (cfg.timeframe, cfg.bar_window);
        set.spawn(async move {
            let _permit = gate.acquire().await.expect("semaphore open");
            let series = bars
                .bars(&inst.symbol, tf, window)
                .await
                .map_err(|e| Skipped {
                    symbol: inst.symbol.clone(),
                    reason: e.to_string(),
                })?;
            score_one(
                &inst,
                &series.iter().map(|b| (b.ts, b.close)).collect::<Vec<_>>(),
            )
        });
    }

    let (mut scored, mut skipped) = (Vec::new(), Vec::new());
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(s)) => scored.push(s),
            Ok(Err(k)) => skipped.push(k),
            Err(e) => skipped.push(Skipped {
                symbol: Symbol::from("?"),
                reason: format!("task panicked: {e}"),
            }),
        }
    }
    skipped.sort_by(|a, b| a.symbol.as_str().cmp(b.symbol.as_str()));

    let mut out = rank(scored, cfg.top_n);
    out.skipped = skipped;
    out.elapsed_ms = t0.elapsed().as_millis() as u64;
    out
}

/// Pure: signal + price facts for one name, or the reason it cannot be scored.
pub fn score_one(inst: &Instrument, closes: &[(i64, f64)]) -> Result<Scored, Skipped> {
    let skip = |reason: String| Skipped {
        symbol: inst.symbol.clone(),
        reason,
    };
    if closes.len() < MIN_BARS {
        return Err(skip(format!("{} bars, need {MIN_BARS}", closes.len())));
    }
    let values: Vec<f64> = closes.iter().map(|(_, c)| *c).collect();
    let sig = signals::compute(&values).ok_or_else(|| skip("signal returned nothing".into()))?;
    let (as_of, close) = *closes.last().expect("non-empty");
    let change = |back: usize| -> Option<f64> {
        let prev = values.get(values.len().checked_sub(back + 1)?)?;
        (*prev > 0.0).then(|| close / prev - 1.0)
    };
    Ok(Scored {
        symbol: inst.symbol.clone(),
        name: inst.name.clone(),
        sector: inst.sector.clone(),
        themes: inst.themes.clone(),
        region: inst.region.clone(),
        is_etf: inst.is_etf(),
        score: sig.score,
        rating: sig.rating,
        rsi: sig.rsi,
        close,
        change_1d: change(1),
        change_5d: change(5),
        change_20d: change(20),
        as_of,
        bars: closes.len(),
    })
}

/// Pure: order, pick, aggregate. Deterministic — same inputs, same `Screen`.
pub fn rank(mut scored: Vec<Scored>, top_n: usize) -> Screen {
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.symbol.as_str().cmp(b.symbol.as_str()))
    });
    let picks = scored
        .iter()
        .filter(|s| !s.is_etf)
        .take(top_n)
        .cloned()
        .collect();
    let as_of = scored.iter().map(|s| s.as_of).max().unwrap_or(0);
    let by_sector = aggregate(scored.iter().map(|s| (s.sector.as_str(), s.score)));
    let by_theme = aggregate(
        scored
            .iter()
            .flat_map(|s| s.themes.iter().map(move |t| (t.as_str(), s.score))),
    );
    Screen {
        as_of,
        ranked: scored,
        picks,
        by_sector,
        by_theme,
        skipped: Vec::new(),
        elapsed_ms: 0,
    }
}

/// Mean score per key, strongest group first, ties by key.
fn aggregate<'a>(pairs: impl Iterator<Item = (&'a str, f64)>) -> Vec<Aggregate> {
    let mut acc: std::collections::BTreeMap<&str, (usize, f64)> = Default::default();
    for (k, v) in pairs {
        let e = acc.entry(k).or_insert((0, 0.0));
        e.0 += 1;
        e.1 += v;
    }
    let mut out: Vec<Aggregate> = acc
        .into_iter()
        .map(|(k, (n, sum))| Aggregate {
            key: k.to_string(),
            count: n,
            mean_score: sum / n as f64,
        })
        .collect();
    out.sort_by(|a, b| {
        b.mean_score
            .partial_cmp(&a.mean_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.key.cmp(&b.key))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::MarketError;
    use crate::domain::types::Bar;
    use async_trait::async_trait;

    const CSV: &str = "symbol,name,sector,themes,region\n\
        UP,Up Inc,semis,ai|datacenter,US\n\
        DOWN,Down Inc,retail,e-commerce,US\n\
        FLAT,Flat Inc,semis,ai,US\n\
        SPY,SPDR S&P 500,etf,sp500,US\n\
        NEW,New IPO,software,ai,US\n";

    fn universe() -> Universe {
        Universe::from_reader(CSV.as_bytes()).unwrap()
    }

    /// Series per symbol: UP rises, DOWN falls, FLAT is flat, SPY rises faster than
    /// anyone, NEW has only 5 bars.
    struct Fake;

    fn series(symbol: &Symbol, n: usize) -> Vec<Bar> {
        let f = |i: usize| -> f64 {
            match symbol.as_str() {
                "UP" => 100.0 + i as f64,
                "DOWN" => 200.0 - i as f64,
                "SPY" => 100.0 + 2.0 * i as f64,
                _ => 100.0,
            }
        };
        (0..n)
            .map(|i| Bar {
                symbol: symbol.clone(),
                ts: i as i64 * 86_400_000,
                open: f(i),
                high: f(i),
                low: f(i),
                close: f(i),
                volume: 1.0,
            })
            .collect()
    }

    #[async_trait]
    impl BarSource for Fake {
        async fn bars(
            &self,
            symbol: &Symbol,
            _: Timeframe,
            limit: usize,
        ) -> Result<Vec<Bar>, MarketError> {
            Ok(match symbol.as_str() {
                "NEW" => series(symbol, 5),
                _ => series(symbol, limit),
            })
        }
        async fn bars_range(
            &self,
            symbol: &Symbol,
            _: Timeframe,
            _: i64,
            _: i64,
        ) -> Result<Vec<Bar>, MarketError> {
            Ok(series(symbol, 60))
        }
    }

    #[tokio::test]
    async fn ranks_picks_aggregates_and_skips() {
        let cfg = ScreenerConfig {
            top_n: 2,
            ..Default::default()
        };
        let s = screen(Arc::new(Fake), &universe(), &cfg).await;

        assert_eq!(s.ranked.len(), 4, "NEW is too short to score");
        assert_eq!(s.skipped.len(), 1);
        assert_eq!(s.skipped[0].symbol.as_str(), "NEW");
        assert!(s.skipped[0].reason.contains("5 bars"));

        let order: Vec<&str> = s.ranked.iter().map(|x| x.symbol.as_str()).collect();
        assert_eq!(order[0], "SPY", "steepest rise scores highest");
        assert_eq!(order[3], "DOWN");

        let picks: Vec<&str> = s.picks.iter().map(|x| x.symbol.as_str()).collect();
        assert_eq!(picks, vec!["UP", "FLAT"], "ETF excluded, top 2 non-ETF");

        let semis = s.by_sector.iter().find(|a| a.key == "semis").unwrap();
        assert_eq!(semis.count, 2);
        let ai = s.by_theme.iter().find(|a| a.key == "ai").unwrap();
        assert_eq!(ai.count, 2, "UP and FLAT carry ai; NEW was skipped");
        assert_eq!(s.as_of, 59 * 86_400_000);
        assert_eq!(s.ranked[0].bars, 60);
    }

    #[test]
    fn score_one_reports_price_changes() {
        let inst = universe().get(&Symbol::from("UP")).unwrap().clone();
        let closes: Vec<(i64, f64)> = (0..30).map(|i| (i as i64, 100.0 + i as f64)).collect();
        let s = score_one(&inst, &closes).unwrap();
        assert_eq!(s.close, 129.0);
        assert!((s.change_1d.unwrap() - (129.0 / 128.0 - 1.0)).abs() < 1e-12);
        assert!((s.change_20d.unwrap() - (129.0 / 109.0 - 1.0)).abs() < 1e-12);
        assert!(matches!(s.rating, Rating::Buy | Rating::StrongBuy));
        let short: Vec<(i64, f64)> = closes[..10].to_vec();
        assert!(score_one(&inst, &short).is_err());
    }

    #[test]
    fn rank_is_deterministic_with_ties() {
        let mk = |sym: &str, score: f64| Scored {
            symbol: Symbol::from(sym),
            name: sym.into(),
            sector: "x".into(),
            themes: vec![],
            region: "US".into(),
            is_etf: false,
            score,
            rating: Rating::Hold,
            rsi: 50.0,
            close: 1.0,
            change_1d: None,
            change_5d: None,
            change_20d: None,
            as_of: 1,
            bars: 30,
        };
        let a = rank(vec![mk("B", 0.5), mk("A", 0.5), mk("C", 0.9)], 10);
        let b = rank(vec![mk("C", 0.9), mk("A", 0.5), mk("B", 0.5)], 10);
        let order = |s: &Screen| {
            s.ranked
                .iter()
                .map(|x| x.symbol.as_str().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(order(&a), vec!["C", "A", "B"]);
        assert_eq!(order(&a), order(&b));
    }
}
