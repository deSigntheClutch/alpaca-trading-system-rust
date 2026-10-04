//! Live smoke test for `LocalBars` over the real Alpaca adapter: a cold read fetches,
//! a warm read does not. Writes into a temp directory. Ignored by default:
//!
//!     cargo test --test live_local_bars -- --ignored --nocapture

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use alpaca_trading_system::domain::config::Config;
use alpaca_trading_system::domain::error::MarketError;
use alpaca_trading_system::domain::types::{Bar, Symbol, Timeframe};
use alpaca_trading_system::infra::{AlpacaBars, AlpacaClient, BarFileStore, LocalBars};
use alpaca_trading_system::traits::BarSource;
use async_trait::async_trait;

/// Counts upstream calls around the real adapter.
struct Counting {
    inner: AlpacaBars,
    calls: AtomicUsize,
}

#[async_trait]
impl BarSource for Counting {
    async fn bars(&self, s: &Symbol, t: Timeframe, l: usize) -> Result<Vec<Bar>, MarketError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.bars(s, t, l).await
    }
    async fn bars_range(
        &self,
        s: &Symbol,
        t: Timeframe,
        a: i64,
        b: i64,
    ) -> Result<Vec<Bar>, MarketError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.bars_range(s, t, a, b).await
    }
}

#[tokio::test]
#[ignore]
async fn cold_then_warm() {
    let _ = dotenvy::dotenv();
    let cfg = Config::load("config/strategies.toml").expect("config");
    assert!(!cfg.alpaca_key.is_empty(), "ALPACA_KEY missing in .env");
    let client = Arc::new(AlpacaClient::new(
        cfg.alpaca_key,
        cfg.alpaca_secret,
        cfg.alpaca_trade_base,
        cfg.alpaca_data_base,
    ));
    let dir = std::env::temp_dir().join(format!("live_local_bars_{}", std::process::id()));
    let store = Arc::new(BarFileStore::new(&dir));
    let upstream = Arc::new(Counting {
        inner: AlpacaBars::new(client),
        calls: AtomicUsize::new(0),
    });
    let local = LocalBars::new(store.clone(), upstream.clone());

    for sym in ["NVDA", "BTC/USD"] {
        let s = Symbol::from(sym);
        let cold = local
            .bars(&s, Timeframe::Day1, 60)
            .await
            .unwrap_or_else(|e| panic!("{sym}: {e}"));
        let warm = local.bars(&s, Timeframe::Day1, 60).await.unwrap();
        assert_eq!(cold.len(), 60, "{sym}");
        assert_eq!(cold.len(), warm.len());
        assert_eq!(cold.last().unwrap().ts, warm.last().unwrap().ts);
        println!(
            "{sym}: {} bars, last {}, file {}",
            warm.len(),
            warm.last().unwrap().ts,
            store.path(&s, Timeframe::Day1).display()
        );
    }
    assert_eq!(
        upstream.calls.load(Ordering::SeqCst),
        2,
        "one upstream call per symbol, none on the warm read"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
