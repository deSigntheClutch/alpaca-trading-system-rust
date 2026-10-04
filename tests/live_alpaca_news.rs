//! Live smoke test against Alpaca News. Read-only, one or two requests. Ignored by
//! default because it needs network and the keys in `.env`:
//!
//!     cargo test --test live_alpaca_news -- --ignored --nocapture

use std::sync::Arc;

use alpaca_trading_system::domain::config::Config;
use alpaca_trading_system::domain::types::Symbol;
use alpaca_trading_system::infra::{AlpacaClient, AlpacaNews};
use alpaca_trading_system::traits::NewsSource;

fn news() -> AlpacaNews {
    let _ = dotenvy::dotenv();
    let cfg = Config::load("config/strategies.toml").expect("config");
    assert!(!cfg.alpaca_key.is_empty(), "ALPACA_KEY missing in .env");
    AlpacaNews::new(Arc::new(AlpacaClient::new(
        cfg.alpaca_key,
        cfg.alpaca_secret,
        cfg.alpaca_trade_base,
        cfg.alpaca_data_base,
    )))
}

#[tokio::test]
#[ignore]
async fn last_three_days_of_headlines_for_two_names() {
    let asked = [Symbol::from("NVDA"), Symbol::from("MU")];
    let since = chrono::Utc::now().timestamp_millis() - 3 * 86_400_000;
    let got = news()
        .headlines(&asked, since, 10)
        .await
        .expect("news call");
    assert!(!got.is_empty(), "no headlines in three days?");
    assert!(got.len() <= 10);
    assert!(
        got.windows(2).all(|w| w[0].ts >= w[1].ts),
        "not newest first"
    );
    assert!(
        got.iter().all(|h| h.ts >= since),
        "vendor returned an older item"
    );
    assert!(
        got.iter()
            .all(|h| h.symbols.iter().any(|s| asked.contains(s))),
        "an item mentions neither name"
    );
    for h in &got {
        println!(
            "{} [{}] {}",
            h.ts,
            h.symbols
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(","),
            h.headline
        );
    }
}
