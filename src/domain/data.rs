//! Lookback timeframes + %change. Pure and dependency-free. Used to decide how
//! many bars to fetch and to give the agents a plain price-move number.

/// Lookback windows for %change / bar fetches.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Timeframe {
    Week,
    Month,
    ThreeMonths,
    SixMonths,
    Year,
}

impl Timeframe {
    pub const ALL: [Timeframe; 5] = [
        Timeframe::Week,
        Timeframe::Month,
        Timeframe::ThreeMonths,
        Timeframe::SixMonths,
        Timeframe::Year,
    ];

    /// Approx trading-day lookback.
    pub fn lookback_days(self) -> usize {
        match self {
            Timeframe::Week => 5,
            Timeframe::Month => 21,
            Timeframe::ThreeMonths => 63,
            Timeframe::SixMonths => 126,
            Timeframe::Year => 252,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Timeframe::Week => "1W",
            Timeframe::Month => "1M",
            Timeframe::ThreeMonths => "3M",
            Timeframe::SixMonths => "6M",
            Timeframe::Year => "1Y",
        }
    }
}

/// %change of `closes` over `lookback` bars: latest vs `lookback` bars ago.
/// None if the series is too short.
pub fn pct_change(closes: &[f64], lookback: usize) -> Option<f64> {
    let n = closes.len();
    if n <= lookback {
        return None;
    }
    let now = closes[n - 1];
    let then = closes[n - 1 - lookback];
    Some((now - then) / then * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pct_change_basic() {
        let closes = [100.0, 110.0, 121.0]; // +10% then +10%
        assert_eq!(pct_change(&closes, 1), Some(10.0)); // last vs prev
        assert_eq!(pct_change(&closes, 2), Some(21.0)); // last vs first
        assert_eq!(pct_change(&closes, 3), None); // too short
    }

    #[test]
    fn lookback_days_ordered() {
        let d: Vec<usize> = Timeframe::ALL.iter().map(|t| t.lookback_days()).collect();
        assert!(d.windows(2).all(|w| w[0] < w[1]), "lookbacks must increase: {d:?}");
    }
}
