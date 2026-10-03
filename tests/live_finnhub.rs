//! Live smoke test against Finnhub. Read-only, one call. Ignored by default
//! because it needs network and `FINNHUB_KEY` in `.env`:
//!
//!     cargo test --test live_finnhub -- --ignored --nocapture

use alpaca_trading_system::domain::config::Config;
use alpaca_trading_system::domain::types::Symbol;
use alpaca_trading_system::infra::FinnhubClient;
use alpaca_trading_system::traits::FundamentalSource;
use chrono::{Duration, Utc};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

fn client() -> FinnhubClient {
    let _ = dotenvy::dotenv();
    let cfg = Config::load("config/strategies.toml").expect("config");
    FinnhubClient::new(cfg.finnhub_key).expect("FINNHUB_KEY missing in .env")
}

fn human(d: Decimal) -> String {
    let n = d.to_f64().unwrap_or(0.0);
    match n.abs() {
        x if x >= 1e12 => format!("{:.2}T", n / 1e12),
        x if x >= 1e9 => format!("{:.2}B", n / 1e9),
        x if x >= 1e6 => format!("{:.2}M", n / 1e6),
        x if x >= 1e3 => format!("{:.2}K", n / 1e3),
        _ => format!("{n:.0}"),
    }
}

#[tokio::test]
#[ignore]
async fn next_seven_days_has_reporters() {
    let today = Utc::now().date_naive();
    let events = client()
        .earnings_calendar(today, today + Duration::days(7))
        .await
        .expect("earnings call");
    assert!(!events.is_empty(), "no earnings in the coming week?");
    assert!(
        events.iter().all(|e| e.date >= today),
        "vendor returned past dates"
    );
    for e in events.iter().take(10) {
        println!(
            "{} {} {:?} eps={:?}",
            e.date, e.symbol.0, e.earnings_hour, e.eps_estimate
        );
    }
}

#[tokio::test]
#[ignore]
async fn get_market_map() {
    let stock_symbols = ["AMZN", "SNDK", "HOOD", "SHOP", "PDD"];

    for sym in stock_symbols {
        let cap = client()
            .market_cap(&Symbol::from(sym))
            .await
            .unwrap()
            .unwrap();

        println!("{sym} market cap {}", human(cap));
    }
}
