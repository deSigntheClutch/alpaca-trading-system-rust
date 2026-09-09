//! Heatmap data model + timeframes + mock data (MVP).
//! Pure and dependency-free -> compiles for both native and wasm, and these
//! tests actually run (`cargo test -p alpaca-core`).
//!
//! The slider picks a `Timeframe`; each stock tile is colored by its %change
//! over that timeframe's lookback and sized (in the UI) by `market_cap`. All
//! timeframes are derived from ONE close series per stock — so dragging the
//! slider is instant, no refetch.
//!
//! What's hardcoded vs synthetic here (MVP):
//!   - symbol + market_cap : hand-entered, roughly realistic ($B). Alpaca has no
//!     market cap, so this is placeholder until we source it (later: shares × price).
//!   - closes              : synthetic (`synth`/`stock`), just so tiles differ.
//! Real data later: a native fetch crate produces this same shape (as JSON).

/// Timeframes the slider exposes, in order (slider index 0..=4).
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

    /// Approx trading-day lookback used for the %change.
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

/// One stock: daily closes (chronological, >=252 to cover the longest timeframe)
/// plus a market cap ($B) used to size its tile.
pub struct Stock {
    pub symbol: &'static str,
    pub market_cap: f64,
    pub closes: Vec<f64>,
}

/// A sector groups stocks. Robinhood-style: one tile per stock, grouped by sector.
pub struct Sector {
    pub name: &'static str,
    pub stocks: Vec<Stock>,
}

impl Sector {
    /// Sector %change = mean of member stocks' %change over `tf`. None if no
    /// stock has enough data. (Handy for a sector-level summary if you want it.)
    pub fn pct_over(&self, tf: Timeframe) -> Option<f64> {
        let lb = tf.lookback_days();
        let vals: Vec<f64> = self
            .stocks
            .iter()
            .filter_map(|s| pct_change(&s.closes, lb))
            .collect();
        if vals.is_empty() {
            None
        } else {
            Some(vals.iter().sum::<f64>() / vals.len() as f64)
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

/// %change -> HSL background color. Green (up) / red (down); brighter with
/// magnitude, saturating at ±3%. Same idea as the old heatmap.
pub fn color(pct: f64) -> String {
    let t = (pct.abs() / 3.0).min(1.0); // 0..1, |pct| >= 3% is full
    let light = 20.0 + t * 28.0; // lightness 20% (flat) -> 48% (strong)
    let hue = if pct >= 0.0 { 145 } else { 355 }; // green / red
    format!("hsl({hue}, 68%, {light:.0}%)")
}

/// Deterministic synthetic close series: `start` price with a per-bar `drift`
/// and a little sine wobble. No RNG dependency.
fn synth(start: f64, drift: f64, n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| start + drift * i as f64 + (i as f64 * 0.3).sin() * start * 0.02)
        .collect()
}

/// Build a mock stock: symbol + market cap are the "real" hardcoded bits; the
/// close series is synthesized with a deterministic start/drift derived from the
/// ticker, so every stock trends differently (some up, some down).
fn stock(symbol: &'static str, market_cap: f64) -> Stock {
    let seed: f64 = symbol.bytes().map(|b| b as f64).sum();
    let start = 40.0 + (seed % 300.0); // ~40..340
    let drift = ((seed % 11.0) - 5.0) / 40.0; // ~ -0.125..0.125 per bar
    Stock { symbol, market_cap, closes: synth(start, drift, 260) }
}

/// 10 sectors x 10 stocks. Market caps are approximate ($B); relative sizes are
/// what matter for the treemap. TODO: replace with fetched/sourced data.
pub fn mock_sectors() -> Vec<Sector> {
    let s = |name, stocks| Sector { name, stocks };
    vec![
        s("科技", vec![
            stock("AAPL", 3400.0), stock("MSFT", 3100.0), stock("NVDA", 3000.0),
            stock("AVGO", 1100.0), stock("ORCL", 500.0), stock("CRM", 320.0),
            stock("AMD", 260.0), stock("ADBE", 240.0), stock("CSCO", 250.0),
            stock("INTC", 130.0),
        ]),
        s("通讯", vec![
            stock("GOOGL", 2100.0), stock("META", 1300.0), stock("NFLX", 300.0),
            stock("TMUS", 260.0), stock("DIS", 200.0), stock("VZ", 180.0),
            stock("CMCSA", 170.0), stock("T", 150.0), stock("CHTR", 60.0),
            stock("EA", 40.0),
        ]),
        s("可选消费", vec![
            stock("AMZN", 2000.0), stock("TSLA", 900.0), stock("HD", 400.0),
            stock("MCD", 210.0), stock("LOW", 150.0), stock("BKNG", 130.0),
            stock("TJX", 130.0), stock("NKE", 120.0), stock("SBUX", 110.0),
            stock("ABNB", 90.0),
        ]),
        s("必需消费", vec![
            stock("WMT", 600.0), stock("PG", 390.0), stock("COST", 380.0),
            stock("KO", 280.0), stock("PEP", 230.0), stock("MO", 100.0),
            stock("MDLZ", 90.0), stock("CL", 70.0), stock("TGT", 70.0),
            stock("KMB", 45.0),
        ]),
        s("金融", vec![
            stock("JPM", 640.0), stock("V", 560.0), stock("MA", 420.0),
            stock("BAC", 300.0), stock("WFC", 220.0), stock("AXP", 180.0),
            stock("GS", 160.0), stock("MS", 160.0), stock("BLK", 140.0),
            stock("C", 120.0),
        ]),
        s("医疗", vec![
            stock("LLY", 750.0), stock("UNH", 500.0), stock("JNJ", 380.0),
            stock("ABBV", 320.0), stock("MRK", 300.0), stock("TMO", 210.0),
            stock("ABT", 200.0), stock("DHR", 190.0), stock("PFE", 160.0),
            stock("BMY", 100.0),
        ]),
        s("能源", vec![
            stock("XOM", 480.0), stock("CVX", 290.0), stock("COP", 130.0),
            stock("EOG", 70.0), stock("SLB", 60.0), stock("WMB", 60.0),
            stock("MPC", 55.0), stock("PSX", 55.0), stock("VLO", 45.0),
            stock("OXY", 45.0),
        ]),
        s("工业", vec![
            stock("GE", 190.0), stock("CAT", 180.0), stock("RTX", 160.0),
            stock("HON", 140.0), stock("UNP", 140.0), stock("DE", 120.0),
            stock("BA", 130.0), stock("LMT", 110.0), stock("ETN", 110.0),
            stock("UPS", 110.0),
        ]),
        s("公用事业", vec![
            stock("NEE", 160.0), stock("SO", 100.0), stock("DUK", 90.0),
            stock("SRE", 55.0), stock("AEP", 55.0), stock("D", 45.0),
            stock("PEG", 45.0), stock("EXC", 40.0), stock("XEL", 40.0),
            stock("ED", 35.0),
        ]),
        s("房地产", vec![
            stock("PLD", 110.0), stock("AMT", 100.0), stock("EQIX", 90.0),
            stock("WELL", 90.0), stock("SPG", 60.0), stock("PSA", 60.0),
            stock("DLR", 60.0), stock("O", 55.0), stock("CCI", 45.0),
            stock("VICI", 35.0),
        ]),
    ]
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
    fn color_sign_and_cap() {
        assert_eq!(color(0.0), "hsl(145, 68%, 20%)"); // flat = darkest green
        assert_eq!(color(3.0), "hsl(145, 68%, 48%)"); // +3% = brightest green
        assert_eq!(color(9.0), "hsl(145, 68%, 48%)"); // capped past 3%
        assert_eq!(color(-3.0), "hsl(355, 68%, 48%)"); // -3% = brightest red
    }

    #[test]
    fn longer_timeframe_needs_more_data() {
        let s = Sector {
            name: "t",
            stocks: vec![Stock { symbol: "X", market_cap: 1.0, closes: synth(100.0, 0.1, 30) }],
        };
        assert!(s.pct_over(Timeframe::Week).is_some()); // 5 <= 30
        assert!(s.pct_over(Timeframe::Year).is_none()); // 252 > 30
    }

    #[test]
    fn mock_is_10x10_with_caps() {
        let sectors = mock_sectors();
        assert_eq!(sectors.len(), 10);
        for sec in &sectors {
            assert_eq!(sec.stocks.len(), 10, "sector {} not 10 stocks", sec.name);
            for st in &sec.stocks {
                assert!(st.market_cap > 0.0);
                assert!(st.closes.len() >= 252); // enough for the 1Y timeframe
            }
        }
    }
}
