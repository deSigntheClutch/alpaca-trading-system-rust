//! Agent pipeline: bars → signal (pure) → analyst (LLM) → decision (LLM) → guards →
//! execute → persist every step (WAL). Generic over the four ports, so the whole
//! flow is testable with fakes — no network, no database, no live orders.
//!
//! Write ordering is load-bearing: the `order_intent` row is persisted BEFORE the
//! broker call and `order_result` AFTER — a crash between the two leaves an intent
//! with no result (recoverable), never an order with no record.

use rust_decimal::Decimal;
use serde_json::{json, Value};

use crate::domain::error::PipelineError;
use crate::domain::signals;
use crate::domain::types::{Action, Decision, OrderRequest, Position, Side, Symbol};
use crate::engine::guards::{self, GuardConfig};
use crate::traits::{BarSource, Broker, LlmClient, Store};

const ANALYST_SYS: &str = "You are a crypto market analyst. Given a technical signal \
and recent prices, write 2-3 sentences on the setup. Be concise and non-committal about \
risk. This is educational, not investment advice.";

const DECIDE_SYS: &str = "You are a disciplined crypto trader. Given an analysis and \
signal, decide an action. Respond with ONLY a JSON object: \
{\"action\":\"BUY|SELL|HOLD\",\"qty\":<number>,\"score\":<-1..1>,\"reason\":\"...\"}. \
qty is in units of the base asset. Prefer HOLD when uncertain.";

pub struct PipelineConfig {
    pub strategy_id: String,
    /// How many daily bars to pull for the signal window (needs ≥20).
    pub bar_window: usize,
    pub max_notional: Decimal,
    /// Verification only: when set, skip the signal + LLM and force this side's order
    /// (still clamped by the guards to `max_notional`). `None` = normal signal-driven flow.
    pub force_side: Option<Side>,
}

/// Orchestrates one decision cycle for one symbol over the four ports.
pub struct Pipeline<B, L, S, K> {
    bars: B,
    llm: L,
    store: S,
    broker: K,
    cfg: PipelineConfig,
}

impl<B: BarSource, L: LlmClient, S: Store, K: Broker> Pipeline<B, L, S, K> {
    pub fn new(bars: B, llm: L, store: S, broker: K, cfg: PipelineConfig) -> Self {
        Self { bars, llm, store, broker, cfg }
    }

    /// Run the full cycle for one symbol, returning the guarded decision. Every step
    /// is persisted; store failures are logged, not fatal (the fallback file catches
    /// the safety-critical order events inside the Store impl).
    pub async fn run_symbol(&self, symbol: &Symbol) -> Result<Decision, PipelineError> {
        let now = chrono::Utc::now();
        let ts = now.timestamp_millis();
        let day = now.format("%Y-%m-%d").to_string();
        let decision_id = format!("{}:{}:{}", self.cfg.strategy_id, symbol, day);

        // 1. market data — fetch + persist (persistence is best-effort).
        let bars = self.bars.daily_bars(symbol, self.cfg.bar_window).await?;
        if let Err(e) = self.store.write_bars(&bars).await {
            tracing::warn!(error = %e, "write_bars failed (non-fatal)");
        }
        let closes: Vec<f64> = bars.iter().map(|b| b.close).collect();
        let last_close = closes.last().copied().unwrap_or(0.0);
        let price = Decimal::from_f64_retain(last_close).unwrap_or_default();

        // Verification short-circuit: FORCE_SIDE skips signal + both LLM calls and goes
        // straight to a guarded order, so you can confirm the execute + Alpaca paper path
        // end-to-end. The guards still clamp to max_notional — set MAX_NOTIONAL=1 for ~$1.
        if let Some(side) = self.cfg.force_side {
            let positions = self.broker.positions().await?;
            let position_qty = position_qty_for(&positions, symbol);
            let buying_power = self.broker.buying_power().await?;
            let action = if side == Side::Buy { Action::Buy } else { Action::Sell };
            let forced = Decision {
                symbol: symbol.clone(),
                action,
                qty: Decimal::from(1_000_000_000u64), // huge → guard clamps to max_notional/price
                score: 0.0,
                reason: format!("FORCED {action:?} — verification, bypasses signal/LLM"),
            };
            self.append(symbol, ts, "decision", &decision_id, &json!({"forced": format!("{action:?}")})).await;
            let guarded = guards::apply(
                forced, position_qty, buying_power, price,
                &GuardConfig { max_notional: self.cfg.max_notional },
            );
            self.append(symbol, ts, "guard", &decision_id, &guard_payload(&guarded)).await;
            return self.execute(symbol, ts, &decision_id, guarded).await;
        }

        // 2. signal — no signal ⇒ Hold, cycle over.
        let Some(sig) = signals::compute(&closes) else {
            let d = hold(symbol, "insufficient bars for signal");
            self.append(symbol, ts, "guard", &decision_id, &guard_payload(&d)).await;
            return Ok(d);
        };
        self.append(
            symbol, ts, "market_data", &decision_id,
            &json!({"last_close": last_close, "score": sig.score, "rsi": sig.rsi}),
        ).await;

        // 3. analyst agent.
        let analyst_user = format!(
            "Symbol {symbol}\nLast close: {last_close}\nSignal score: {:.3} (RSI {:.1})\nRecent closes: {:?}",
            sig.score, sig.rsi, tail(&closes, 10),
        );
        let analysis = self.llm.message(ANALYST_SYS, &analyst_user).await?;
        self.append(symbol, ts, "analyst", &decision_id, &json!({"analysis": analysis})).await;

        // 4. broker truth (positions + cash) for the decision + guards.
        let positions = self.broker.positions().await?;
        let position_qty = position_qty_for(&positions, symbol);
        let buying_power = self.broker.buying_power().await?;

        // 5. decision agent → strict JSON (parse failure ⇒ Hold).
        let decide_user = format!(
            "Analysis:\n{analysis}\n\nSignal score: {:.3}\nPosition qty: {position_qty}\nBuying power: {buying_power}\nRespond with ONLY the JSON object.",
            sig.score,
        );
        let raw = self.llm.message(DECIDE_SYS, &decide_user).await?;
        let decision = parse_decision(symbol, &raw);
        self.append(symbol, ts, "decision", &decision_id, &json!({"raw": raw})).await;

        // 6. guards — the last clamp before a real order.
        let guarded = guards::apply(
            decision, position_qty, buying_power, price,
            &GuardConfig { max_notional: self.cfg.max_notional },
        );
        self.append(symbol, ts, "guard", &decision_id, &guard_payload(&guarded)).await;

        // 7. execute.
        self.execute(symbol, ts, &decision_id, guarded).await
    }

    /// Execute a guarded decision. WAL: `order_intent` is persisted BEFORE the broker
    /// call, `order_result` AFTER — a crash between the two leaves a recoverable intent,
    /// never an order with no record. Hold ⇒ no-op. Returns the decision unchanged.
    async fn execute(
        &self,
        symbol: &Symbol,
        ts: i64,
        decision_id: &str,
        guarded: Decision,
    ) -> Result<Decision, PipelineError> {
        if guarded.action != Action::Hold {
            let side = if guarded.action == Action::Buy { Side::Buy } else { Side::Sell };
            let req = OrderRequest {
                symbol: symbol.clone(),
                qty: guarded.qty,
                side,
                client_order_id: decision_id.to_string(),
            };
            self.append(
                symbol, ts, "order_intent", decision_id,
                &json!({"qty": req.qty.to_string(), "side": side}),
            ).await;
            let order = self.broker.submit_order(&req).await?;
            self.append(
                symbol, ts, "order_result", decision_id,
                &json!({"id": order.id, "status": order.status}),
            ).await;
        }
        Ok(guarded)
    }

    async fn append(&self, symbol: &Symbol, ts: i64, step: &str, decision_id: &str, payload: &Value) {
        if let Err(e) = self
            .store
            .append_event(&self.cfg.strategy_id, symbol, ts, step, decision_id, &payload.to_string())
            .await
        {
            tracing::error!(error = %e, step, "append_event failed");
        }
    }
}

fn hold(symbol: &Symbol, why: &str) -> Decision {
    Decision {
        symbol: symbol.clone(),
        action: Action::Hold,
        qty: Decimal::ZERO,
        score: 0.0,
        reason: why.to_string(),
    }
}

fn guard_payload(d: &Decision) -> Value {
    json!({"action": d.action, "qty": d.qty.to_string(), "score": d.score, "reason": d.reason})
}

fn tail<T>(v: &[T], n: usize) -> &[T] {
    &v[v.len().saturating_sub(n)..]
}

fn position_qty_for(positions: &[Position], symbol: &Symbol) -> Decimal {
    // Alpaca crypto positions come back as "BTCUSD"; our symbols are "BTC/USD".
    let want = norm(symbol.as_str());
    positions
        .iter()
        .find(|p| norm(p.symbol.as_str()) == want)
        .map(|p| p.qty)
        .unwrap_or_default()
}

fn norm(s: &str) -> String {
    s.replace('/', "").to_ascii_uppercase()
}

/// Parse the decision agent's reply. Tolerant of prose around the JSON; ANY failure
/// (bad JSON, missing/unknown action) collapses to Hold — never a rogue order.
fn parse_decision(symbol: &Symbol, raw: &str) -> Decision {
    let v: Value = match serde_json::from_str(extract_json(raw)) {
        Ok(v) => v,
        Err(_) => return hold(symbol, "decision JSON parse failed"),
    };
    let action = match v.get("action").and_then(Value::as_str).map(|s| s.to_ascii_uppercase()).as_deref() {
        Some("BUY") => Action::Buy,
        Some("SELL") => Action::Sell,
        Some("HOLD") => Action::Hold,
        _ => return hold(symbol, "missing/invalid action"),
    };
    Decision {
        symbol: symbol.clone(),
        action,
        qty: parse_decimal(v.get("qty")),
        score: v.get("score").and_then(Value::as_f64).unwrap_or(0.0),
        reason: v.get("reason").and_then(Value::as_str).unwrap_or("").to_string(),
    }
}

/// Slice from the first `{` to the last `}` — strips code fences / prose.
fn extract_json(s: &str) -> &str {
    match (s.find('{'), s.rfind('}')) {
        (Some(a), Some(b)) if b > a => &s[a..=b],
        _ => s,
    }
}

fn parse_decimal(v: Option<&Value>) -> Decimal {
    match v {
        Some(Value::String(s)) => s.parse().unwrap_or_default(),
        Some(Value::Number(n)) => n.as_f64().and_then(Decimal::from_f64_retain).unwrap_or_default(),
        _ => Decimal::ZERO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::{BrokerError, LlmError, MarketError, StoreError};
    use crate::domain::types::{Bar, Order};
    use async_trait::async_trait;
    use rust_decimal_macros::dec;
    use std::sync::Mutex;

    // Rising series ending at 50 → bullish signal is Some (≥20 bars).
    struct FakeBars;
    #[async_trait]
    impl BarSource for FakeBars {
        async fn daily_bars(&self, symbol: &Symbol, _limit: usize) -> Result<Vec<Bar>, MarketError> {
            let bars = (10..=50)
                .map(|i| Bar {
                    symbol: symbol.clone(),
                    ts: i as i64 * 86_400_000,
                    open: i as f64,
                    high: i as f64,
                    low: i as f64,
                    close: i as f64,
                    volume: 1.0,
                })
                .collect();
            Ok(bars)
        }
    }

    // Always says BUY 10 units — huge, so the guard MUST clamp it.
    struct FakeLlm;
    #[async_trait]
    impl LlmClient for FakeLlm {
        async fn message(&self, _system: &str, _user: &str) -> Result<String, LlmError> {
            Ok(r#"{"action":"BUY","qty":10,"score":0.8,"reason":"test"}"#.into())
        }
    }

    #[derive(Default)]
    struct FakeStore {
        steps: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl Store for FakeStore {
        async fn append_event(&self, _s: &str, _sym: &Symbol, _ts: i64, step: &str, _id: &str, _p: &str) -> Result<(), StoreError> {
            self.steps.lock().unwrap().push(step.to_string());
            Ok(())
        }
        async fn write_bars(&self, _bars: &[Bar]) -> Result<(), StoreError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeBroker {
        submitted_qty: Mutex<Option<Decimal>>,
    }
    #[async_trait]
    impl Broker for FakeBroker {
        async fn buying_power(&self) -> Result<Decimal, BrokerError> {
            Ok(dec!(1_000_000))
        }
        async fn positions(&self) -> Result<Vec<Position>, BrokerError> {
            Ok(vec![])
        }
        async fn submit_order(&self, req: &OrderRequest) -> Result<Order, BrokerError> {
            *self.submitted_qty.lock().unwrap() = Some(req.qty);
            Ok(Order {
                id: "ord_1".into(),
                client_order_id: req.client_order_id.clone(),
                symbol: req.symbol.clone(),
                status: "accepted".into(),
            })
        }
    }

    #[tokio::test]
    async fn buy_is_clamped_and_wal_order_is_intent_before_result() {
        let store = FakeStore::default();
        let broker = FakeBroker::default();
        let pipe = Pipeline::new(
            FakeBars,
            FakeLlm,
            store,
            broker,
            PipelineConfig { strategy_id: "test".into(), bar_window: 60, max_notional: dec!(100), force_side: None },
        );

        let d = pipe.run_symbol(&Symbol::from("BTC/USD")).await.unwrap();

        // max_notional 100 @ price 50 → qty clamped from 10 to 2.
        assert_eq!(d.action, Action::Buy);
        assert_eq!(d.qty, dec!(2));
        assert_eq!(*pipe.broker.submitted_qty.lock().unwrap(), Some(dec!(2)));

        let steps = pipe.store.steps.lock().unwrap();
        let intent = steps.iter().position(|s| s == "order_intent").unwrap();
        let result = steps.iter().position(|s| s == "order_result").unwrap();
        assert!(intent < result, "WAL: intent must precede result — {steps:?}");
    }

    // LLM that MUST NOT be called on the forced path — errors if it is.
    struct NeverLlm;
    #[async_trait]
    impl LlmClient for NeverLlm {
        async fn message(&self, _system: &str, _user: &str) -> Result<String, LlmError> {
            Err(LlmError::Request("LLM called on forced path".into()))
        }
    }

    #[tokio::test]
    async fn force_side_bypasses_llm_and_clamps() {
        let pipe = Pipeline::new(
            FakeBars,
            NeverLlm,
            FakeStore::default(),
            FakeBroker::default(),
            PipelineConfig {
                strategy_id: "test".into(),
                bar_window: 60,
                max_notional: dec!(100),
                force_side: Some(Side::Buy),
            },
        );

        let d = pipe.run_symbol(&Symbol::from("BTC/USD")).await.unwrap();

        // Forced buy, clamped to max_notional 100 @ price 50 → qty 2, no LLM call.
        assert_eq!(d.action, Action::Buy);
        assert_eq!(d.qty, dec!(2));
        assert_eq!(*pipe.broker.submitted_qty.lock().unwrap(), Some(dec!(2)));
    }
}
