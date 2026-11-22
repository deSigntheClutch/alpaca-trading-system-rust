//! Indicators + composite signal. Pure numeric in/out, no network, incrementally
//! updatable, tests below. Ported from the old alpaca/heatmap/src/signal.rs.
//! ponytail: naive educational signal, not investment advice.
//!
//! Sub-signals (all from close prices):
//!   1. Trend    : close vs SMA20 (trend-following)
//!   2. Momentum : SMA5 vs SMA20 crossover direction (trend-following)
//!   3. RSI(14)  : overbought/oversold, contrarian brake

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "frontend/src/bindings/")]
pub enum Rating {
    StrongBuy,
    Buy,
    Hold,
    Sell,
    StrongSell,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Signal {
    pub score: f64, // [-1, 1]: >0 bullish, <0 bearish
    pub rating: Rating,
    pub rsi: f64, // raw RSI 0..100, for display
}

/// Simple mean of the last n closes; None if not enough data.
fn sma(closes: &[f64], n: usize) -> Option<f64> {
    if closes.len() < n {
        return None;
    }
    let tail = &closes[closes.len() - n..];
    Some(tail.iter().sum::<f64>() / n as f64)
}

/// n-day RSI with Wilder smoothing (TA-Lib seeding): 0..100. None if not enough data.
fn rsi(closes: &[f64], n: usize) -> Option<f64> {
    if closes.len() < n + 1 {
        return None;
    }
    let v = *fin_primitives::signals::batch_f64::rsi(closes, n).last()?;
    v.is_finite().then_some(v)
}

/// Close series (chronological, >=20 bars) -> composite signal. None if too short.
pub fn compute(closes: &[f64]) -> Option<Signal> {
    let last = *closes.last()?;
    let sma20 = sma(closes, 20)?;
    let sma5 = sma(closes, 5)?;
    let rsi_v = rsi(closes, 14)?;

    // Normalize each sub-signal to [-1,1]: divide by a "full-scale" threshold, then clamp.
    let trend = ((last - sma20) / sma20 / 0.08).clamp(-1.0, 1.0); // ±8% off SMA20 = full
    let momentum = ((sma5 - sma20) / sma20 / 0.05).clamp(-1.0, 1.0); // ±5% fast-slow gap = full
    let rsi_score = ((50.0 - rsi_v) / 30.0).clamp(-1.0, 1.0); // contrarian: oversold (low RSI) = buy

    // Weights sum to 1, so score stays in [-1,1]; trend+momentum lead, RSI brakes.
    let score = 0.4 * trend + 0.35 * momentum + 0.25 * rsi_score;

    let rating = if score >= 0.5 {
        Rating::StrongBuy
    } else if score >= 0.2 {
        Rating::Buy
    } else if score > -0.2 {
        Rating::Hold
    } else if score > -0.5 {
        Rating::Sell
    } else {
        Rating::StrongSell
    };
    Some(Signal {
        score,
        rating,
        rsi: rsi_v,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rising_series_is_bullish() {
        let closes: Vec<f64> = (0..40).map(|i| 100.0 + i as f64).collect(); // steady uptrend
        let s = compute(&closes).unwrap();
        assert!(s.score > 0.0, "score={}", s.score);
        assert!(matches!(s.rating, Rating::Buy | Rating::StrongBuy));
    }

    #[test]
    fn falling_series_is_bearish() {
        let closes: Vec<f64> = (0..40).map(|i| 200.0 - i as f64).collect(); // steady downtrend
        let s = compute(&closes).unwrap();
        assert!(s.score < 0.0, "score={}", s.score);
        assert!(matches!(s.rating, Rating::Sell | Rating::StrongSell));
    }

    #[test]
    fn rsi_is_wilder_smoothed() {
        // Wilder's 14-period RSI over this series (seed = mean of the first 14 changes,
        // then avg = (prev * 13 + x) / 14) ends at 56.945932803034346.
        // The old simple-average RSI read 47.40 here.
        let closes = [
            44.34, 44.09, 44.15, 43.61, 44.33, 44.83, 45.10, 45.42, 45.84, 46.08, 45.89, 46.03,
            45.61, 46.28, 46.28, 46.00, 46.03, 46.41, 46.22, 45.64, 46.21, 46.25, 45.71, 46.45,
            45.78, 45.35, 44.03, 44.17, 45.01, 44.89, 45.43, 46.03,
        ];
        let r = rsi(&closes, 14).unwrap();
        assert!((r - 56.945932803034346).abs() < 1e-6, "rsi={r}");
    }

    #[test]
    fn too_short_returns_none() {
        assert!(compute(&[1.0, 2.0, 3.0]).is_none()); // not enough for SMA20
    }
}
