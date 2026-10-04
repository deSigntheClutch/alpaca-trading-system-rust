//! Market regime: is the index above its 200-day moving average? One line at the
//! top of the morning report, and later the gate on new longs (#61 phase 3). Faber
//! (2007) style: it does not add return, it keeps you out of broad sell-offs.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::domain::error::MarketError;
use crate::domain::types::{Symbol, Timeframe};
use crate::traits::BarSource;

/// Default index and window. SPY because it is what "the market" means to a US
/// equity screener; 200 days because that is the number everybody else uses, which
/// is the point of a regime line.
pub const DEFAULT_SYMBOL: &str = "SPY";
pub const DEFAULT_WINDOW: usize = 200;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub struct Regime {
    #[ts(type = "string")]
    pub symbol: Symbol,
    /// Timestamp of the close used, epoch millis.
    pub as_of: i64,
    pub close: f64,
    pub sma: f64,
    pub window: usize,
    pub above: bool,
    /// `(close / sma − 1) × 100`; negative when below.
    pub distance_pct: f64,
}

/// `None` when fewer than `window` bars exist — a new store, or a bad symbol — so the
/// report shows "regime: unavailable" rather than a number computed on half a window.
pub async fn regime<B: BarSource>(
    bars: &B,
    symbol: &Symbol,
    window: usize,
) -> Result<Option<Regime>, MarketError> {
    // A few extra so a short window of holidays does not drop us under `window`.
    let series = bars.bars(symbol, Timeframe::Day1, window + 10).await?;
    Ok(compute(
        symbol,
        &series.iter().map(|b| (b.ts, b.close)).collect::<Vec<_>>(),
        window,
    ))
}

/// Pure part: `(ts, close)` oldest→newest.
pub fn compute(symbol: &Symbol, closes: &[(i64, f64)], window: usize) -> Option<Regime> {
    if window == 0 || closes.len() < window {
        return None;
    }
    let tail = &closes[closes.len() - window..];
    let sma = tail.iter().map(|(_, c)| c).sum::<f64>() / window as f64;
    let (as_of, close) = *closes.last()?;
    Some(Regime {
        symbol: symbol.clone(),
        as_of,
        close,
        sma,
        window,
        above: close >= sma,
        distance_pct: (close / sma - 1.0) * 100.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(n: usize, f: impl Fn(usize) -> f64) -> Vec<(i64, f64)> {
        (0..n).map(|i| (i as i64 * 86_400_000, f(i))).collect()
    }

    #[test]
    fn too_short_is_none() {
        let spy = Symbol::from("SPY");
        assert!(compute(&spy, &series(199, |_| 100.0), 200).is_none());
        assert!(compute(&spy, &series(200, |_| 100.0), 200).is_some());
    }

    #[test]
    fn rising_market_is_above_with_positive_distance() {
        let spy = Symbol::from("SPY");
        let r = compute(&spy, &series(250, |i| 100.0 + i as f64), 200).unwrap();
        assert!(r.above);
        assert!(r.distance_pct > 0.0);
        assert_eq!(r.close, 349.0);
        assert_eq!(r.as_of, 249 * 86_400_000);
        // SMA of the last 200 of 100..=349 is the mean of 150..=349 = 249.5
        assert!((r.sma - 249.5).abs() < 1e-9);
    }

    #[test]
    fn falling_market_is_below() {
        let spy = Symbol::from("SPY");
        let r = compute(&spy, &series(250, |i| 500.0 - i as f64), 200).unwrap();
        assert!(!r.above);
        assert!(r.distance_pct < 0.0);
    }
}
