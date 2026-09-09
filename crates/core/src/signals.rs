//! Indicators + composite signal. Pure numeric in/out, no network, incrementally
//! updatable, tests below. Ported from the old alpaca/heatmap/src/signal.rs.
//! ponytail: naive educational signal, not investment advice.
//!
//! Sub-signals (all from close prices):
//!   1. Trend    : close vs SMA20 (trend-following)
//!   2. Momentum : SMA5 vs SMA20 crossover direction (trend-following)
//!   3. RSI(14)  : overbought/oversold, contrarian brake

#[derive(Debug, PartialEq)]
pub enum Rating {
    StrongBuy,
    Buy,
    Hold,
    Sell,
    StrongSell,
}

impl Rating {
    // Display labels stay Chinese: they render in the HTML report.
    pub fn label(&self) -> &'static str {
        match self {
            Rating::StrongBuy => "强烈买入",
            Rating::Buy => "买入",
            Rating::Hold => "持有",
            Rating::Sell => "卖出",
            Rating::StrongSell => "强烈卖出",
        }
    }
}

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

/// n-day RSI (simple-average variant): 0..100. None if not enough data.
fn rsi(closes: &[f64], n: usize) -> Option<f64> {
    if closes.len() < n + 1 {
        return None;
    }
    let changes: Vec<f64> = closes.windows(2).map(|w| w[1] - w[0]).collect();
    let recent = &changes[changes.len() - n..];
    let (mut gain, mut loss) = (0.0, 0.0);
    for &c in recent {
        if c >= 0.0 {
            gain += c;
        } else {
            loss -= c;
        }
    }
    let (avg_gain, avg_loss) = (gain / n as f64, loss / n as f64);
    if avg_loss == 0.0 {
        return Some(100.0); // all gains, no losses = max overbought
    }
    let rs = avg_gain / avg_loss;
    Some(100.0 - 100.0 / (1.0 + rs))
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
    Some(Signal { score, rating, rsi: rsi_v })
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
    fn too_short_returns_none() {
        assert!(compute(&[1.0, 2.0, 3.0]).is_none()); // not enough for SMA20
    }
}
