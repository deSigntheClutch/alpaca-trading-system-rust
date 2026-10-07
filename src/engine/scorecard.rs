//! The scorecard (#35): did the rule work? Every past report's picks are scored at
//! fixed horizons against what actually happened and against the benchmark over the
//! same sessions. This is the one number the whole project exists to produce, and the
//! reason replayed reports exist: with `morning --replay-from` the history is there
//! on day one instead of in a month.
//!
//! Entry is the **open** of the first session on or after the report date — the
//! report is read before the open, so that is the first price a reader could get.
//! Exit is the **close** of the `h`-th session counting that one as the first. The
//! benchmark gets the same two prices on its own sessions. A report is included at a
//! horizon only when its `h` sessions have all completed.

use chrono::NaiveDate;

use crate::domain::report::{HorizonStats, MorningReport, PickOutcome, ReportOutcome, Scorecard};
use crate::domain::types::{Bar, Symbol, Timeframe};
use crate::traits::BarSource;

/// Sessions after the report date. Five is "did it move this week", twenty is "did it
/// move this month"; the signal is built from 5- and 20-day averages, so these are the
/// horizons it implicitly claims.
pub const HORIZONS: [usize; 2] = [5, 20];

const MS_PER_DAY: i64 = 86_400_000;

/// One report's price material: its date, the benchmark's sessions from that date,
/// and each pick's sessions from that date.
type ReportBars = (NaiveDate, Vec<Bar>, Vec<(Symbol, Vec<Bar>)>);

/// What the number does and does not mean, printed next to it.
pub const NOTE: &str = "Picks bought at the next open and sold at the close h sessions \
later, compared with the benchmark over the same sessions. Replayed reports use today's \
universe and adjusted prices (survivorship bias). No fees, no slippage, equal weight.";

/// Score `reports` (any order; those dated on or after `as_of` are ignored) using
/// `bars` for prices. One `bars_range` per pick per report plus one for the
/// benchmark per report; from the local store that is milliseconds.
pub async fn scorecard<B: BarSource>(
    bars: &B,
    reports: &[MorningReport],
    benchmark: &Symbol,
    as_of: NaiveDate,
) -> Scorecard {
    let max_h = HORIZONS.iter().copied().max().unwrap_or(1);
    let span_days = (max_h as i64) * 2 + 15;

    // Collect per-report series once, then score each horizon from the same bars.
    let mut per_report: Vec<ReportBars> = Vec::new();
    let mut sorted: Vec<&MorningReport> = reports.iter().filter(|r| r.date < as_of).collect();
    sorted.sort_by_key(|r| r.date);
    for r in sorted {
        let from = midnight_ms(r.date);
        let to = from + span_days * MS_PER_DAY;
        let bench = match bars.bars_range(benchmark, Timeframe::Day1, from, to).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(date = %r.date, error = %e, "scorecard: no benchmark bars");
                continue;
            }
        };
        let mut picks = Vec::with_capacity(r.picks.len());
        for p in &r.picks {
            match bars
                .bars_range(&p.scored.symbol, Timeframe::Day1, from, to)
                .await
            {
                Ok(b) => picks.push((p.scored.symbol.clone(), b)),
                Err(e) => {
                    tracing::warn!(date = %r.date, symbol = %p.scored.symbol, error = %e, "scorecard: no bars")
                }
            }
        }
        per_report.push((r.date, bench, picks));
    }

    let horizons = HORIZONS
        .iter()
        .map(|&h| {
            let by_report: Vec<ReportOutcome> = per_report
                .iter()
                .filter_map(|(date, bench, picks)| {
                    let outs: Vec<PickOutcome> = picks
                        .iter()
                        .filter_map(|(sym, pb)| outcome(sym, pb, bench, h))
                        .collect();
                    (!outs.is_empty()).then(|| summarise(*date, outs))
                })
                .collect();
            let all: Vec<&PickOutcome> = by_report.iter().flat_map(|r| r.picks.iter()).collect();
            let n = all.len().max(1) as f64;
            HorizonStats {
                horizon_days: h,
                reports: by_report.len(),
                picks: all.len(),
                mean_return: all.iter().map(|o| o.ret).sum::<f64>() / n,
                mean_benchmark_return: all.iter().map(|o| o.benchmark_ret).sum::<f64>() / n,
                mean_excess: all.iter().map(|o| o.excess).sum::<f64>() / n,
                hit_rate: all.iter().filter(|o| o.excess > 0.0).count() as f64 / n,
                by_report,
            }
        })
        .collect();

    Scorecard {
        as_of,
        benchmark: benchmark.clone(),
        horizons,
        note: NOTE.to_string(),
    }
}

/// One pick at one horizon, or `None` when either leg lacks `h` completed sessions
/// or has a non-positive entry price.
pub fn outcome(symbol: &Symbol, pick: &[Bar], bench: &[Bar], h: usize) -> Option<PickOutcome> {
    if h == 0 || pick.len() < h || bench.len() < h {
        return None;
    }
    let (p_in, p_out) = (&pick[0], &pick[h - 1]);
    let (b_in, b_out) = (&bench[0], &bench[h - 1]);
    if p_in.open <= 0.0 || b_in.open <= 0.0 {
        return None;
    }
    let ret = p_out.close / p_in.open - 1.0;
    let benchmark_ret = b_out.close / b_in.open - 1.0;
    Some(PickOutcome {
        symbol: symbol.clone(),
        entry_ts: p_in.ts,
        entry: p_in.open,
        exit_ts: p_out.ts,
        exit: p_out.close,
        ret,
        benchmark_ret,
        excess: ret - benchmark_ret,
    })
}

fn summarise(date: NaiveDate, picks: Vec<PickOutcome>) -> ReportOutcome {
    let n = picks.len().max(1) as f64;
    ReportOutcome {
        date,
        mean_return: picks.iter().map(|o| o.ret).sum::<f64>() / n,
        mean_excess: picks.iter().map(|o| o.excess).sum::<f64>() / n,
        hit_rate: picks.iter().filter(|o| o.excess > 0.0).count() as f64 / n,
        picks,
    }
}

fn midnight_ms(d: NaiveDate) -> i64 {
    d.and_hms_opt(0, 0, 0)
        .expect("midnight")
        .and_utc()
        .timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::MarketError;
    use crate::domain::report::{Pick, RunStats, Scored};
    use crate::domain::signals::Rating;
    use async_trait::async_trait;

    /// Daily bars from `start` (midnight) for `n` days: open = f(i), close = f(i) * (1 + drift).
    fn bars(symbol: &Symbol, start: i64, n: usize, level: f64, drift: f64) -> Vec<Bar> {
        (0..n)
            .map(|i| {
                let open = level * (1.0 + drift).powi(i as i32);
                Bar {
                    symbol: symbol.clone(),
                    ts: start + i as i64 * MS_PER_DAY,
                    open,
                    high: open,
                    low: open,
                    close: open * (1.0 + drift),
                    volume: 1.0,
                }
            })
            .collect()
    }

    #[test]
    fn outcome_uses_next_open_and_hth_close_against_the_benchmark() {
        let up = Symbol::from("UP");
        let spy = Symbol::from("SPY");
        let p = bars(&up, 0, 10, 100.0, 0.01); // +1% a day
        let b = bars(&spy, 0, 10, 500.0, 0.0); // flat
        let o = outcome(&up, &p, &b, 5).unwrap();
        assert_eq!(o.entry, 100.0);
        assert_eq!(o.entry_ts, 0);
        assert_eq!(
            o.exit_ts,
            4 * MS_PER_DAY,
            "fifth session, counting the entry session as the first"
        );
        assert!((o.ret - (1.01f64.powi(5) - 1.0)).abs() < 1e-12);
        assert_eq!(o.benchmark_ret, 0.0);
        assert!((o.excess - o.ret).abs() < 1e-12);
        assert!(
            outcome(&up, &p, &b, 11).is_none(),
            "horizon not yet complete"
        );
        assert!(
            outcome(&up, &p, &b[..3], 5).is_none(),
            "benchmark too short"
        );
    }

    /// Serves `UP` rising, `DOWN` falling, `SPY` flat, all from day 0 for 40 days.
    struct Fake;
    #[async_trait]
    impl BarSource for Fake {
        async fn bars(&self, s: &Symbol, _: Timeframe, l: usize) -> Result<Vec<Bar>, MarketError> {
            self.bars_range(s, Timeframe::Day1, 0, l as i64 * MS_PER_DAY)
                .await
        }
        async fn bars_range(
            &self,
            s: &Symbol,
            _: Timeframe,
            start: i64,
            end: i64,
        ) -> Result<Vec<Bar>, MarketError> {
            let (level, drift) = match s.as_str() {
                "UP" => (100.0, 0.01),
                "DOWN" => (100.0, -0.01),
                _ => (500.0, 0.0),
            };
            Ok(bars(s, 0, 40, level, drift)
                .into_iter()
                .filter(|b| b.ts >= start && b.ts < end)
                .collect())
        }
    }

    fn report(day: u32, picks: &[&str]) -> MorningReport {
        let scored = |sym: &str| Scored {
            symbol: Symbol::from(sym),
            name: sym.into(),
            sector: "x".into(),
            themes: vec![],
            region: "US".into(),
            is_etf: false,
            score: 0.5,
            rating: Rating::Buy,
            rsi: 60.0,
            close: 1.0,
            change_1d: None,
            change_5d: None,
            change_20d: None,
            as_of: 0,
            bars: 60,
        };
        MorningReport {
            date: NaiveDate::from_ymd_opt(1970, 1, day).unwrap(),
            generated_at: 0,
            regime: None,
            picks: picks
                .iter()
                .map(|s| Pick {
                    scored: scored(s),
                    earnings: None,
                    headlines: vec![],
                    commentary: None,
                })
                .collect(),
            watchlist: vec![],
            earnings_this_week: vec![],
            by_sector: vec![],
            by_theme: vec![],
            skipped: vec![],
            universe_size: 3,
            scorecard: None,
            run: RunStats::default(),
        }
    }

    #[tokio::test]
    async fn scores_only_completed_horizons_and_ignores_reports_from_as_of_onward() {
        let spy = Symbol::from("SPY");
        // Day 2: UP and DOWN. Day 30: UP only (20-day horizon incomplete: only 11 bars left).
        // Day 45: after as_of (day 41) — ignored even though it exists.
        let reports = vec![
            report(2, &["UP", "DOWN"]),
            report(30, &["UP"]),
            report(31, &["UP"]),
        ];
        let as_of = NaiveDate::from_ymd_opt(1970, 1, 31).unwrap();
        let sc = scorecard(&Fake, &reports, &spy, as_of).await;
        assert_eq!(sc.benchmark, spy);
        let h5 = &sc.horizons[0];
        assert_eq!(h5.horizon_days, 5);
        assert_eq!(
            h5.reports, 2,
            "day 2 and day 30 both have 5 sessions of data"
        );
        assert_eq!(h5.picks, 3);
        assert!(
            (h5.hit_rate - 2.0 / 3.0).abs() < 1e-12,
            "UP wins twice, DOWN loses once"
        );
        assert!(h5.mean_excess > 0.0);
        let h20 = &sc.horizons[1];
        assert_eq!(h20.reports, 1, "only day 2 has 20 completed sessions");
        assert_eq!(h20.picks, 2);
        assert!((h20.hit_rate - 0.5).abs() < 1e-12);
        assert_eq!(
            h20.by_report[0].date,
            NaiveDate::from_ymd_opt(1970, 1, 2).unwrap()
        );
    }
}
