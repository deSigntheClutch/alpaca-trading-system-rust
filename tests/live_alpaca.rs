//! Live smoke tests against Alpaca's paper endpoints. Read-only: bars, account,
//! positions. Never places an order. Ignored by default because they need network and
//! the keys in `.env`:
//!
//!     cargo test --test live_alpaca -- --ignored --nocapture

use std::sync::Arc;

use alpaca_trading_system::domain::config::Config;
use alpaca_trading_system::domain::types::{Symbol, Timeframe};
use alpaca_trading_system::infra::{AlpacaBars, AlpacaBroker, AlpacaClient};
use alpaca_trading_system::traits::{BarSource, Broker};

fn client() -> Arc<AlpacaClient> {
    let _ = dotenvy::dotenv();
    let cfg = Config::load("config/strategies.toml").expect("config");
    assert!(!cfg.alpaca_key.is_empty(), "ALPACA_KEY missing in .env");
    Arc::new(AlpacaClient::new(
        cfg.alpaca_key,
        cfg.alpaca_secret,
        cfg.alpaca_trade_base,
        cfg.alpaca_data_base,
    ))
}

#[tokio::test]
#[ignore]
async fn stock_and_crypto_bars_come_back_chronological() {
    let bars = AlpacaBars::new(client());
    for sym in ["NVDA", "BTC/USD"] {
        let got = bars
            .bars(&Symbol::from(sym), Timeframe::Day1, 5)
            .await
            .unwrap_or_else(|e| panic!("{sym}: {e}"));
        assert_eq!(got.len(), 5, "{sym}: expected 5 bars, got {}", got.len());
        assert!(got.windows(2).all(|w| w[0].ts < w[1].ts), "{sym}: not ascending");
        assert!(got.iter().all(|b| b.close > 0.0 && b.high >= b.low), "{sym}: bad OHLC");
        println!("{sym}: last close {} at {}", got[4].close, got[4].ts);
    }
}

#[tokio::test]
#[ignore]
async fn paper_account_is_readable() {
    let broker = AlpacaBroker::new(client());
    let bp = broker.buying_power().await.expect("buying_power");
    let positions = broker.positions().await.expect("positions");
    println!("buying power {bp}, {} open positions", positions.len());
    assert!(bp >= rust_decimal::Decimal::ZERO);
}
