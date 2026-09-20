//! Walk-forward backtest that REUSES the live `Pipeline` unchanged — only the ports
//! are swapped. A day at a time, `run_symbol` runs the identical signal→LLM→guards
//! chain as paper; the sim just decides what data it sees and how orders fill. So a
//! backtested edge is the same decision logic that runs live — nothing to distrust.
//!
//! Two swapped ports (both on one `SimMarket`, since a bar source and a broker over
//! the same price series share state) + a `CollectingStore` that captures the score,
//! RSI and analyst text each day so the frontend can show WHY the LLM chose as it did.
//!
//! ponytail: close-to-close fills, no slippage/fees — an optimistic upper bound. Add a
//! fill model when the numbers need to be trusted for real capital. f64 sim money
//! (not `Decimal`): rounding is immaterial to a paper what-if, and it stays simple.

use async_trait::async_trait;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use std::sync::Mutex;

use crate::domain::dto::{BacktestResult, DayRecord};
use crate::domain::error::{BrokerError, MarketError, StoreError};
use crate::domain::types::{Action, Bar, Decision, Order, OrderRequest, Position, Side, Symbol};
use crate::engine::pipeline::{Pipeline, PipelineConfig};
use crate::traits::{BarSource, Broker, LlmClient, Store};

/// Backtest knobs. `days` caps how many recent bars we actually make decisions on
/// (each decision day = 2 LLM calls, so this bounds cost).
pub struct BacktestConfig {
    pub starting_cash: f64,
    pub max_notional: Decimal,
    pub days: usize,
    /// Bull/bear debate rounds per decision day (0 = skip). Mirrors the live pipeline so
    /// a backtested edge uses the same decision chain; adds 2 LLM calls/round/day to cost.
    pub debate_rounds: usize,
}

/// Historical price series + a virtual account, exposing both `BarSource` (what the
/// pipeline may see today) and `Broker` (fills at today's close). Interior mutability
/// because the ports take `&self` and one `&sim` backs both roles in `Pipeline::new`.
struct SimMarket {
    bars: Vec<Bar>,
    cursor: Mutex<usize>, // "today" index; daily_bars never returns bars after it
    cash: Mutex<f64>,
    position: Mutex<f64>,
}

impl SimMarket {
    fn new(bars: Vec<Bar>, starting_cash: f64) -> Self {
        Self {
            bars,
            cursor: Mutex::new(0),
            cash: Mutex::new(starting_cash),
            position: Mutex::new(0.0),
        }
    }
    fn set_cursor(&self, i: usize) {
        *self.cursor.lock().unwrap() = i;
    }
    fn cursor(&self) -> usize {
        *self.cursor.lock().unwrap()
    }
    fn close(&self, i: usize) -> f64 {
        self.bars[i].close
    }
    fn ts(&self, i: usize) -> i64 {
        self.bars[i].ts
    }
    fn cash(&self) -> f64 {
        *self.cash.lock().unwrap()
    }
    fn position(&self) -> f64 {
        *self.position.lock().unwrap()
    }
}

#[async_trait]
impl BarSource for SimMarket {
    async fn daily_bars(&self, _symbol: &Symbol, limit: usize) -> Result<Vec<Bar>, MarketError> {
        // The no-lookahead guarantee: only bars up to and including `cursor`, at most
        // `limit` of them. Nothing after today is ever visible to the signal/LLM.
        let cursor = self.cursor();
        let start = (cursor + 1).saturating_sub(limit);
        Ok(self.bars[start..=cursor].to_vec())
    }
}

#[async_trait]
impl Broker for SimMarket {
    async fn buying_power(&self) -> Result<Decimal, BrokerError> {
        Ok(Decimal::from_f64_retain(self.cash()).unwrap_or_default())
    }

    async fn positions(&self) -> Result<Vec<Position>, BrokerError> {
        let pos = self.position();
        if pos.abs() < 1e-12 {
            return Ok(vec![]); // flat → no position row, like a real broker
        }
        let close = self.close(self.cursor());
        Ok(vec![Position {
            symbol: self.bars[self.cursor()].symbol.clone(),
            qty: Decimal::from_f64_retain(pos).unwrap_or_default(),
            market_value: Decimal::from_f64_retain(pos * close).unwrap_or_default(),
        }])
    }

    async fn submit_order(&self, req: &OrderRequest) -> Result<Order, BrokerError> {
        // Fill at today's close, instantly, in full. (ponytail: no slippage/fees.)
        let close = self.close(self.cursor());
        let qty = req.qty.to_f64().unwrap_or(0.0);
        {
            let mut cash = self.cash.lock().unwrap();
            let mut pos = self.position.lock().unwrap();
            match req.side {
                Side::Buy => {
                    *cash -= qty * close;
                    *pos += qty;
                }
                Side::Sell => {
                    *cash += qty * close;
                    *pos -= qty;
                }
            }
        }
        Ok(Order {
            id: format!("sim-{}", self.cursor()),
            client_order_id: req.client_order_id.clone(),
            symbol: req.symbol.clone(),
            status: "filled".into(),
        })
    }
}

/// A `Store` that keeps nothing — it just plucks the score/RSI (`market_data` step)
/// and analyst text (`analyst` step) out of the events the pipeline persists, so the
/// driver can attach them to the day's record. A fresh one is used per day.
#[derive(Default)]
struct CollectingStore {
    score: Mutex<f64>,
    rsi: Mutex<f64>,
    analyst: Mutex<String>,
}

#[async_trait]
impl Store for CollectingStore {
    async fn append_event(
        &self,
        _strategy_id: &str,
        _symbol: &Symbol,
        _ts: i64,
        step: &str,
        _decision_id: &str,
        payload: &str,
    ) -> Result<(), StoreError> {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(payload) else {
            return Ok(());
        };
        match step {
            "market_data" => {
                if let Some(s) = v.get("score").and_then(|x| x.as_f64()) {
                    *self.score.lock().unwrap() = s;
                }
                if let Some(r) = v.get("rsi").and_then(|x| x.as_f64()) {
                    *self.rsi.lock().unwrap() = r;
                }
            }
            "analyst" => {
                if let Some(a) = v.get("analysis").and_then(|x| x.as_str()) {
                    *self.analyst.lock().unwrap() = a.to_string();
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn write_bars(&self, _bars: &[Bar]) -> Result<(), StoreError> {
        Ok(())
    }

    /// Per-day throwaway store: it only snapshots the current cycle, so there is no
    /// history to read back. Backtest memory recall would need a persistent store.
    async fn read_events(
        &self,
        _strategy_id: &str,
        _symbol: &Symbol,
    ) -> Result<Vec<crate::domain::types::Event>, StoreError> {
        Ok(Vec::new())
    }
}

impl CollectingStore {
    fn snapshot(&self) -> (f64, f64, String) {
        (
            *self.score.lock().unwrap(),
            *self.rsi.lock().unwrap(),
            self.analyst.lock().unwrap().clone(),
        )
    }
}

/// Run the walk-forward backtest for one symbol. `bars` is the full daily series
/// (chronological); decisions are made only on the last `cfg.days` bars (the earlier
/// ones are the signal warmup window). Never propagates errors — a failing day is
/// recorded as a Hold so one bad LLM reply doesn't abort the whole run.
pub async fn run_backtest<L: LlmClient>(
    symbol: &Symbol,
    bars: Vec<Bar>,
    llm: &L,
    cfg: BacktestConfig,
) -> BacktestResult {
    let len = bars.len();
    let starting_cash = cfg.starting_cash;

    // signals::compute needs ≥20 bars; without that, no lookahead-free decision exists.
    if len < 20 {
        return empty_result(symbol, starting_cash);
    }

    let sim = SimMarket::new(bars, starting_cash);
    // First decision day: `days` back from the end, but never before we have 20 bars.
    let start = len.saturating_sub(cfg.days).max(19);
    let first_close = sim.close(start);
    let last_close = sim.close(len - 1);

    let mut days = Vec::new();
    let mut equity_curve = Vec::new();
    let mut num_trades = 0u32;

    for cursor in start..len {
        sim.set_cursor(cursor);
        let store = CollectingStore::default();
        let pipe = Pipeline::new(
            &sim,
            llm,
            &store,
            &sim,
            PipelineConfig {
                strategy_id: "backtest".into(),
                bar_window: 60,
                max_notional: cfg.max_notional,
                debate_rounds: cfg.debate_rounds,
                force_side: None,
            },
        );
        let decision = match pipe.run_symbol(symbol).await {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, cursor, "backtest day failed → recorded as Hold");
                Decision {
                    symbol: symbol.clone(),
                    action: Action::Hold,
                    qty: Decimal::ZERO,
                    score: 0.0,
                    reason: format!("day error: {e}"),
                }
            }
        };

        let close = sim.close(cursor);
        let equity = sim.cash() + sim.position() * close;
        let (score, rsi, analyst_text) = store.snapshot();
        if decision.action != Action::Hold {
            num_trades += 1;
        }
        equity_curve.push(equity);
        days.push(DayRecord {
            ts: sim.ts(cursor),
            close,
            score,
            rsi,
            action: format!("{:?}", decision.action), // "Buy" | "Sell" | "Hold"
            qty: decision.qty.to_f64().unwrap_or(0.0),
            equity,
            decision_reason: decision.reason,
            analyst_text,
        });
    }

    let final_equity = *equity_curve.last().unwrap_or(&starting_cash);
    let total_return = if starting_cash > 0.0 {
        (final_equity - starting_cash) / starting_cash
    } else {
        0.0
    };
    let buy_hold_return = if first_close > 0.0 {
        (last_close - first_close) / first_close
    } else {
        0.0
    };

    BacktestResult {
        symbol: symbol.to_string(),
        starting_cash,
        days,
        total_return,
        max_drawdown: max_drawdown(&equity_curve),
        num_trades,
        buy_hold_return,
    }
}

fn empty_result(symbol: &Symbol, starting_cash: f64) -> BacktestResult {
    BacktestResult {
        symbol: symbol.to_string(),
        starting_cash,
        days: vec![],
        total_return: 0.0,
        max_drawdown: 0.0,
        num_trades: 0,
        buy_hold_return: 0.0,
    }
}

/// Worst peak-to-trough drop as a positive fraction (0.3 = a 30% drawdown).
fn max_drawdown(equity: &[f64]) -> f64 {
    let mut peak = f64::MIN;
    let mut max_dd = 0.0;
    for &e in equity {
        if e > peak {
            peak = e;
        }
        if peak > 0.0 {
            let dd = (peak - e) / peak;
            if dd > max_dd {
                max_dd = dd;
            }
        }
    }
    max_dd
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::LlmError;
    use rust_decimal_macros::dec;

    fn rising_bars(n: usize) -> Vec<Bar> {
        (0..n)
            .map(|i| {
                let p = 100.0 + i as f64;
                Bar {
                    symbol: Symbol::from("BTC/USD"),
                    ts: i as i64 * 86_400_000,
                    open: p,
                    high: p,
                    low: p,
                    close: p,
                    volume: 1.0,
                }
            })
            .collect()
    }

    // Analyst call gets prose; decision call (system prompt mentions JSON) gets a BUY.
    struct FakeLlm;
    #[async_trait]
    impl LlmClient for FakeLlm {
        async fn message(&self, system: &str, _user: &str) -> Result<String, LlmError> {
            if system.contains("JSON") {
                Ok(r#"{"action":"BUY","qty":0.01,"score":0.6,"reason":"fake buy"}"#.into())
            } else {
                Ok("fake analysis text".into())
            }
        }
    }

    #[tokio::test]
    async fn sim_market_never_looks_ahead() {
        let bars = rising_bars(40);
        let sim = SimMarket::new(bars.clone(), 10_000.0);
        sim.set_cursor(25);
        let got = sim
            .daily_bars(&Symbol::from("BTC/USD"), 60)
            .await
            .unwrap();
        // The window ends exactly at "today" and contains nothing after it.
        assert_eq!(got.last().unwrap().ts, bars[25].ts);
        assert!(got.iter().all(|b| b.ts <= bars[25].ts), "no future bars leaked");
        assert!(got.len() <= 26);
    }

    #[tokio::test]
    async fn run_backtest_produces_days_and_finite_metrics() {
        let bars = rising_bars(40);
        let result = run_backtest(
            &Symbol::from("BTC/USD"),
            bars,
            &FakeLlm,
            BacktestConfig {
                starting_cash: 10_000.0,
                max_notional: dec!(1000),
                days: 10,
                debate_rounds: 0,
            },
        )
        .await;

        assert_eq!(result.days.len(), 10, "10 decision days over the last 10 bars");
        assert!(result.total_return.is_finite());
        assert!(result.max_drawdown.is_finite() && result.max_drawdown >= 0.0);
        assert!(result.num_trades >= 1, "FakeLlm always buys → trades happen");
        // Every recorded day used a real bar (no fabricated prices) and carried the
        // captured signal + analyst text the frontend needs.
        assert!(result
            .days
            .iter()
            .all(|d| d.close > 0.0 && d.equity.is_finite() && !d.analyst_text.is_empty()));
    }
}
