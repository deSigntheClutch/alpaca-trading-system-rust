//! Walk-forward backtest that REUSES the live `Pipeline` unchanged — only the ports
//! are swapped. A day at a time, `run_symbol` runs the identical signal→LLM→guards
//! chain as paper; the sim just decides what data it sees and how orders fill. So a
//! backtested edge is the same decision logic that runs live — nothing to distrust.
//!
//! Two swapped ports (both on one `SimMarket`, since a bar source and a broker over
//! the same price series share state) + a `CollectingStore` that captures the score,
//! RSI and analyst text each day so the frontend can show WHY the LLM chose as it did.
//!
//! Fill model (#12): an order decided on day t fills at day t+1's **open**, moved
//! against the trader by `slippage_bps`, and pays `fee_bps` of notional per side. The
//! decision only ever sees prices through the close it was made on, so a backtest
//! cannot trade at a price it learned at that same close. f64 sim money (not
//! `Decimal`): rounding is immaterial to a paper what-if, and it stays simple.

use async_trait::async_trait;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use std::sync::Mutex;

use std::collections::HashMap;

use crate::domain::dto::{BacktestResult, DayRecord, Metrics};
use crate::domain::error::{BrokerError, MarketError, StoreError};
use crate::domain::types::{
    Action, Bar, Decision, Order, OrderRequest, Position, Side, Symbol, Timeframe,
};
use crate::engine::metrics;
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
    /// Commission per side, in basis points of notional (25 = 0.25% of each fill).
    pub fee_bps: f64,
    /// Fill offset against the trader, in basis points of the fill's reference price:
    /// buys fill that much above the open, sells that much below.
    pub slippage_bps: f64,
}

/// One simulated execution. `reference` is the close the decision was sized on, so
/// slippage versus the decision price (`price / reference - 1`) is on record.
#[derive(Debug, Clone, PartialEq)]
pub struct Fill {
    /// Bar the fill happened on (epoch millis).
    pub ts: i64,
    /// Bar the decision was made on — joins the fill back to its `DayRecord`.
    pub decided_ts: i64,
    pub side: Side,
    pub qty: f64,
    pub price: f64,
    pub fee: f64,
    pub reference: f64,
}

/// An order accepted on day t, waiting for day t+1's open.
#[derive(Debug, Clone)]
struct Pending {
    side: Side,
    qty: f64,
    reference: f64,
    decided_ts: i64,
}

/// Historical price series + a virtual account, exposing both `BarSource` (what the
/// pipeline may see today) and `Broker` (orders fill at the next open). Interior
/// mutability because the ports take `&self` and one `&sim` backs both roles in
/// `Pipeline::new`.
struct SimMarket {
    bars: Vec<Bar>,
    cursor: Mutex<usize>, // "today" index; daily_bars never returns bars after it
    cash: Mutex<f64>,
    position: Mutex<f64>,
    fee_bps: f64,
    slippage_bps: f64,
    /// The order placed today, filled when the cursor moves to tomorrow.
    pending: Mutex<Option<Pending>>,
    fills: Mutex<Vec<Fill>>,
    fees_paid: Mutex<f64>,
}

impl SimMarket {
    fn new(bars: Vec<Bar>, starting_cash: f64, fee_bps: f64, slippage_bps: f64) -> Self {
        Self {
            bars,
            cursor: Mutex::new(0),
            cash: Mutex::new(starting_cash),
            position: Mutex::new(0.0),
            fee_bps,
            slippage_bps,
            pending: Mutex::new(None),
            fills: Mutex::new(Vec::new()),
            fees_paid: Mutex::new(0.0),
        }
    }

    /// Move "today" to bar `i`. Whatever was ordered on the previous day fills first,
    /// at bar `i`'s open, so the fill is the first thing the new day sees.
    fn set_cursor(&self, i: usize) {
        if let Some(p) = self.pending.lock().unwrap().take() {
            self.fill(p, i, self.bars[i].open);
        }
        *self.cursor.lock().unwrap() = i;
    }

    /// Execute `p` on bar `i` at `base` (an open, or the last day's close), with
    /// slippage against the trader and the per-side fee. A buy that no longer fits
    /// the cash at that price is cut down to what the cash buys; a sell never
    /// exceeds the position. Nothing is ever filled at a price the decision saw.
    fn fill(&self, p: Pending, i: usize, base: f64) {
        let slip = self.slippage_bps / 10_000.0;
        let fee_rate = self.fee_bps / 10_000.0;
        let mut cash = self.cash.lock().unwrap();
        let mut pos = self.position.lock().unwrap();
        let (qty, price) = match p.side {
            Side::Buy => {
                let price = base * (1.0 + slip);
                let affordable = (*cash / (price * (1.0 + fee_rate))).max(0.0);
                (p.qty.min(affordable), price)
            }
            Side::Sell => (p.qty.min(*pos), base * (1.0 - slip)),
        };
        if qty <= 0.0 {
            return;
        }
        let fee = qty * price * fee_rate;
        match p.side {
            Side::Buy => {
                *cash -= qty * price + fee;
                *pos += qty;
            }
            Side::Sell => {
                *cash += qty * price - fee;
                *pos -= qty;
            }
        }
        *self.fees_paid.lock().unwrap() += fee;
        self.fills.lock().unwrap().push(Fill {
            ts: self.bars[i].ts,
            decided_ts: p.decided_ts,
            side: p.side,
            qty,
            price,
            fee,
            reference: p.reference,
        });
    }

    fn fees_paid(&self) -> f64 {
        *self.fees_paid.lock().unwrap()
    }

    fn fills(&self) -> Vec<Fill> {
        self.fills.lock().unwrap().clone()
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
    /// The sim holds one daily series; `timeframe` is accepted for the port and
    /// ignored (intraday replay is out of scope for this phase, see #30).
    async fn bars(
        &self,
        _symbol: &Symbol,
        _timeframe: Timeframe,
        limit: usize,
    ) -> Result<Vec<Bar>, MarketError> {
        // The no-lookahead guarantee: only bars up to and including `cursor`, at most
        // `limit` of them. Nothing after today is ever visible to the signal/LLM.
        let cursor = self.cursor();
        let start = (cursor + 1).saturating_sub(limit);
        Ok(self.bars[start..=cursor].to_vec())
    }

    /// Same no-lookahead rule as `bars`: the range is clipped at the cursor.
    async fn bars_range(
        &self,
        _symbol: &Symbol,
        _timeframe: Timeframe,
        start: i64,
        end: i64,
    ) -> Result<Vec<Bar>, MarketError> {
        let cursor = self.cursor();
        Ok(self.bars[..=cursor]
            .iter()
            .filter(|b| b.ts >= start && b.ts < end)
            .cloned()
            .collect())
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

    /// Accept the order for the next session. The decision was sized on today's
    /// close; the fill waits for tomorrow's open (see `set_cursor`). On the very last
    /// bar there is no tomorrow, so the order fills at that close — still a price the
    /// decision already knew, so no lookahead.
    async fn submit_order(&self, req: &OrderRequest) -> Result<Order, BrokerError> {
        let cursor = self.cursor();
        let pending = Pending {
            side: req.side,
            qty: req.qty.to_f64().unwrap_or(0.0),
            reference: self.close(cursor),
            decided_ts: self.ts(cursor),
        };
        let status = if cursor + 1 < self.bars.len() {
            *self.pending.lock().unwrap() = Some(pending);
            "accepted"
        } else {
            self.fill(pending, cursor, self.close(cursor));
            "filled"
        };
        Ok(Order {
            id: format!("sim-{cursor}"),
            client_order_id: req.client_order_id.clone(),
            symbol: req.symbol.clone(),
            status: status.into(),
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

    let sim = SimMarket::new(bars, starting_cash, cfg.fee_bps, cfg.slippage_bps);
    // First decision day: `days` back from the end, but never before we have 20 bars.
    let start = len.saturating_sub(cfg.days).max(19);
    let first_close = sim.close(start);
    let last_close = sim.close(len - 1);

    let mut days = Vec::new();
    let mut equity_curve = Vec::new();
    let mut exposure = Vec::new();
    let mut reasons: HashMap<i64, String> = HashMap::new();

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
            reasons.insert(sim.ts(cursor), decision.reason.clone());
        }
        equity_curve.push(equity);
        exposure.push(if equity > 0.0 {
            sim.position() * close / equity
        } else {
            0.0
        });
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

    let buy_hold_return = if first_close > 0.0 {
        (last_close - first_close) / first_close
    } else {
        0.0
    };
    let fills = sim.fills();
    let trades = metrics::pair_trades(
        symbol.as_str(),
        &fills,
        &reasons,
        sim.ts(len - 1),
        last_close,
    );
    let summary = metrics::summarize(
        &equity_curve,
        starting_cash,
        &trades,
        &exposure,
        buy_hold_return,
        sim.fees_paid(),
    );
    tracing::info!(
        symbol = %symbol,
        fills = fills.len(),
        trades = summary.num_trades,
        fees = summary.total_fees,
        "backtest done"
    );

    BacktestResult {
        symbol: symbol.to_string(),
        starting_cash,
        days,
        trades,
        metrics: summary,
    }
}

fn empty_result(symbol: &Symbol, starting_cash: f64) -> BacktestResult {
    BacktestResult {
        symbol: symbol.to_string(),
        starting_cash,
        days: vec![],
        trades: vec![],
        metrics: Metrics {
            total_return: 0.0,
            max_drawdown: 0.0,
            sharpe: 0.0,
            num_trades: 0,
            win_rate: 0.0,
            avg_win: 0.0,
            avg_loss: 0.0,
            profit_factor: None,
            exposure: 0.0,
            buy_hold_return: 0.0,
            total_fees: 0.0,
        },
    }
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

    /// Bars whose open differs from the previous close, so "next open" is distinguishable
    /// from "this close": close = 100 + i, next open = close + 1.
    fn gapping_bars(n: usize) -> Vec<Bar> {
        (0..n)
            .map(|i| {
                let close = 100.0 + i as f64;
                Bar {
                    symbol: Symbol::from("NVDA"),
                    ts: i as i64 * 86_400_000,
                    open: close - 1.0 + 2.0, // previous close + 2 → a visible gap
                    high: close + 2.0,
                    low: close - 2.0,
                    close,
                    volume: 1.0,
                }
            })
            .collect()
    }

    fn order(side: Side, qty: f64) -> OrderRequest {
        OrderRequest {
            symbol: Symbol::from("NVDA"),
            qty: Decimal::from_f64_retain(qty).unwrap(),
            side,
            client_order_id: "t".into(),
        }
    }

    #[tokio::test]
    async fn order_fills_at_next_open_with_slippage_and_fee() {
        let bars = gapping_bars(5);
        let sim = SimMarket::new(bars.clone(), 10_000.0, 100.0, 50.0); // 1% fee, 0.5% slip
        sim.set_cursor(2);
        let o = sim.submit_order(&order(Side::Buy, 10.0)).await.unwrap();
        assert_eq!(o.status, "accepted", "nothing fills on the decision day");
        assert_eq!(sim.position(), 0.0);
        assert_eq!(sim.cash(), 10_000.0);

        sim.set_cursor(3);
        let fills = sim.fills();
        assert_eq!(fills.len(), 1);
        let f = &fills[0];
        let expect_price = bars[3].open * 1.005;
        assert!(
            (f.price - expect_price).abs() < 1e-9,
            "buy fills at next open + slippage"
        );
        assert_eq!(
            f.reference, bars[2].close,
            "reference is the close the decision saw"
        );
        assert!((f.fee - 10.0 * expect_price * 0.01).abs() < 1e-9);
        assert_eq!(sim.position(), 10.0);
        assert!((sim.cash() - (10_000.0 - 10.0 * expect_price - f.fee)).abs() < 1e-9);
    }

    #[tokio::test]
    async fn round_trips_cost_exactly_the_fees() {
        // Flat prices: every open and close is 100, so the only thing that moves
        // equity over N round trips is 2N fee legs. 1% per side, no slippage.
        let bars: Vec<Bar> = (0..12)
            .map(|i| Bar {
                symbol: Symbol::from("NVDA"),
                ts: i as i64 * 86_400_000,
                open: 100.0,
                high: 100.0,
                low: 100.0,
                close: 100.0,
                volume: 1.0,
            })
            .collect();
        let sim = SimMarket::new(bars, 10_000.0, 100.0, 0.0);
        let n = 5;
        for k in 0..n {
            sim.set_cursor(2 * k);
            sim.submit_order(&order(Side::Buy, 1.0)).await.unwrap();
            sim.set_cursor(2 * k + 1);
            sim.submit_order(&order(Side::Sell, 1.0)).await.unwrap();
        }
        sim.set_cursor(2 * n); // fills the last sell
        assert_eq!(sim.position(), 0.0);
        let expected_fees = n as f64 * 2.0 * 100.0 * 0.01;
        assert!((sim.fees_paid() - expected_fees).abs() < 1e-9);
        assert!((sim.cash() - (10_000.0 - expected_fees)).abs() < 1e-9);
    }

    #[tokio::test]
    async fn last_day_order_fills_at_that_close() {
        let bars = gapping_bars(4);
        let sim = SimMarket::new(bars.clone(), 10_000.0, 0.0, 0.0);
        sim.set_cursor(3);
        let o = sim.submit_order(&order(Side::Buy, 1.0)).await.unwrap();
        assert_eq!(o.status, "filled");
        assert_eq!(sim.fills()[0].price, bars[3].close);
    }

    #[tokio::test]
    async fn buy_is_cut_to_what_cash_affords_at_the_open() {
        let bars = gapping_bars(4);
        let sim = SimMarket::new(bars.clone(), 1_000.0, 0.0, 0.0);
        sim.set_cursor(1);
        sim.submit_order(&order(Side::Buy, 50.0)).await.unwrap(); // 50 × ~103 > 1000
        sim.set_cursor(2);
        let f = &sim.fills()[0];
        assert!(
            (f.qty * f.price - 1_000.0).abs() < 1e-6,
            "spent exactly the cash"
        );
        assert!(sim.cash().abs() < 1e-6);
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
        let sim = SimMarket::new(bars.clone(), 10_000.0, 0.0, 0.0);
        sim.set_cursor(25);
        let got = sim.daily_bars(&Symbol::from("BTC/USD"), 60).await.unwrap();
        // The window ends exactly at "today" and contains nothing after it.
        assert_eq!(got.last().unwrap().ts, bars[25].ts);
        assert!(
            got.iter().all(|b| b.ts <= bars[25].ts),
            "no future bars leaked"
        );
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
                fee_bps: 25.0,
                slippage_bps: 5.0,
            },
        )
        .await;

        assert_eq!(
            result.days.len(),
            10,
            "10 decision days over the last 10 bars"
        );
        let m = &result.metrics;
        assert!(m.total_return.is_finite());
        assert!(m.max_drawdown.is_finite() && m.max_drawdown >= 0.0);
        assert!(!result.trades.is_empty(), "FakeLlm always buys → lots open");
        assert!(m.total_fees > 0.0, "25 bps fees are charged");
        assert!(m.exposure > 0.0 && m.exposure <= 1.0);
        // Every recorded day used a real bar (no fabricated prices) and carried the
        // captured signal + analyst text the frontend needs.
        assert!(result
            .days
            .iter()
            .all(|d| d.close > 0.0 && d.equity.is_finite() && !d.analyst_text.is_empty()));
    }
}
