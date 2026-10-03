//! Live smoke test against Finnhub. Read-only, one call. Ignored by default
//! because it needs network and `FINNHUB_KEY` in `.env`:
//!
//!     cargo test --test live_finnhub -- --ignored --nocapture

use alpaca_trading_system::domain::config::Config;
use alpaca_trading_system::infra::FinnhubClient;
use alpaca_trading_system::traits::FundamentalSource;
use chrono::{Duration, Utc};

fn client() -> FinnhubClient {
    let _ = dotenvy::dotenv();
    let cfg = Config::load("config/strategies.toml").expect("config");
    FinnhubClient::new(cfg.finnhub_key).expect("FINNHUB_KEY missing in .env")
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
