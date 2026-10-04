//! Live smoke test for the range fetch (#30). Read-only, two requests. Ignored by
//! default because it needs network and the keys in `.env`:
//!
//!     cargo test --test live_alpaca_range -- --ignored --nocapture

use std::sync::Arc;

use alpaca_trading_system::domain::config::Config;
use alpaca_trading_system::domain::types::{Symbol, Timeframe};
use alpaca_trading_system::infra::{AlpacaBars, AlpacaClient};
use alpaca_trading_system::traits::BarSource;
use chrono::NaiveDate;

fn bars() -> AlpacaBars {
    let _ = dotenvy::dotenv();
    let cfg = Config::load("config/strategies.toml").expect("config");
    assert!(!cfg.alpaca_key.is_empty(), "ALPACA_KEY missing in .env");
    AlpacaBars::new(Arc::new(AlpacaClient::new(
        cfg.alpaca_key,
        cfg.alpaca_secret,
        cfg.alpaca_trade_base,
        cfg.alpaca_data_base,
    )))
}

fn ms(y: i32, m: u32, d: u32) -> i64 {
    NaiveDate::from_ymd_opt(y, m, d)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis()
}

#[tokio::test]
#[ignore]
async fn september_2026_daily_bars_are_a_clean_half_open_range() {
    let (start, end) = (ms(2026, 9, 1), ms(2026, 10, 1));
    for (sym, expect_min) in [("NVDA", 19usize), ("BTC/USD", 29usize)] {
        let got = bars()
            .bars_range(&Symbol::from(sym), Timeframe::Day1, start, end)
            .await
            .unwrap_or_else(|e| panic!("{sym}: {e}"));
        assert!(got.len() >= expect_min, "{sym}: {} bars", got.len());
        assert!(
            got.iter().all(|b| b.ts >= start && b.ts < end),
            "{sym}: out of range"
        );
        assert!(
            got.windows(2).all(|w| w[0].ts < w[1].ts),
            "{sym}: not strictly ascending"
        );
        println!(
            "{sym}: {} bars, first {} last {}",
            got.len(),
            got[0].ts,
            got[got.len() - 1].ts
        );
    }
}

#[tokio::test]
#[ignore]
async fn empty_or_inverted_range_is_empty_without_a_request() {
    let b = bars();
    let s = Symbol::from("NVDA");
    assert!(b
        .bars_range(&s, Timeframe::Day1, ms(2026, 9, 1), ms(2026, 9, 1))
        .await
        .unwrap()
        .is_empty());
    assert!(b
        .bars_range(&s, Timeframe::Day1, ms(2026, 9, 2), ms(2026, 9, 1))
        .await
        .unwrap()
        .is_empty());
}
