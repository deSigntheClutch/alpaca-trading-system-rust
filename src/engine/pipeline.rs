//! Agent pipeline: bars → signal (pure) → analyst (LLM) → bull/bear debate (LLM, optional)
//! → trader (LLM) → guards → execute → persist every step (WAL — see `execute` for the
//! write ordering). Generic over the four ports, so the whole flow is testable with fakes
//! — no network, no database, no live orders.

use rust_decimal::Decimal;
use serde_json::{json, Value};

use crate::domain::dto::Arm;
use crate::domain::error::PipelineError;
use crate::domain::signals::{self, Rating, Signal};
use crate::domain::types::{Action, Decision, OrderRequest, Position, Side, Symbol};
use crate::engine::agents::{
    analyst::Analyst,
    researcher::Researcher,
    trader::{Trader, TraderInput},
};
use crate::engine::guards::{self, GuardConfig};
use crate::traits::{BarSource, Broker, LlmClient, Store};

pub struct PipelineConfig {
    pub strategy_id: String,
    /// `Rule` maps the signal's rating to an action with no model call; `Llm` runs
    /// analyst → trader; `LlmDebate` adds `debate_rounds` bull/bear rounds in between.
    pub arm: Arm,
    /// How many daily bars to pull for the signal window (needs ≥20).
    pub bar_window: usize,
    pub max_notional: Decimal,
    /// Bull/bear debate rounds between the analyst and the trader. `0` skips the debate
    /// entirely (analyst → trader, as before). Each round is 2 LLM calls, so this bounds
    /// cost; the trader reads the full transcript before deciding.
    pub debate_rounds: usize,
    /// Verification only (`FORCE_SIDE=buy|sell`): skip the signal + LLM and force this
    /// side's order, so the execute + Alpaca paper path can be confirmed end-to-end.
    /// Still clamped by the guards to `max_notional` (set `MAX_NOTIONAL=1` for ~$1).
    /// `None` = normal signal-driven flow.
    pub force_side: Option<Side>,
}

/// Orchestrates one decision cycle for one symbol over the four ports. Holds the agents
/// it runs (analyst → trader); the ports are the seams, the agents are the LLM roles.
pub struct Pipeline<B, L, S, K> {
    bars: B,
    llm: L,
    store: S,
    broker: K,
    cfg: PipelineConfig,
    analyst: Analyst,
    researchers: [Researcher; 2],
    trader: Trader,
}

impl<B: BarSource, L: LlmClient, S: Store, K: Broker> Pipeline<B, L, S, K> {
    pub fn new(bars: B, llm: L, store: S, broker: K, cfg: PipelineConfig) -> Self {
        Self {
            bars,
            llm,
            store,
            broker,
            cfg,
            analyst: Analyst,
            researchers: [Researcher::bull(), Researcher::bear()],
            trader: Trader,
        }
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

        // Verification short-circuit (see `PipelineConfig::force_side`).
        if let Some(side) = self.cfg.force_side {
            let positions = self.broker.positions().await?;
            let position_qty = position_qty_for(&positions, symbol);
            let buying_power = self.broker.buying_power().await?;
            let action = if side == Side::Buy {
                Action::Buy
            } else {
                Action::Sell
            };
            let forced = Decision {
                symbol: symbol.clone(),
                action,
                qty: Decimal::from(1_000_000_000u64), // huge → guard clamps to max_notional/price
                score: 0.0,
                reason: format!("FORCED {action:?} — verification, bypasses signal/LLM"),
            };
            self.append(
                symbol,
                ts,
                "decision",
                &decision_id,
                &json!({"forced": format!("{action:?}")}),
            )
            .await;
            let guarded = guards::apply(
                forced,
                position_qty,
                buying_power,
                price,
                &GuardConfig {
                    max_notional: self.cfg.max_notional,
                },
            );
            self.append(symbol, ts, "guard", &decision_id, &guard_payload(&guarded))
                .await;
            return self.execute(symbol, ts, &decision_id, guarded).await;
        }

        // 2. signal — no signal ⇒ Hold, cycle over.
        let Some(sig) = signals::compute(&closes) else {
            let d = hold(symbol, "insufficient bars for signal");
            self.append(symbol, ts, "guard", &decision_id, &guard_payload(&d))
                .await;
            return Ok(d);
        };
        self.append(
            symbol,
            ts,
            "market_data",
            &decision_id,
            &json!({"last_close": last_close, "score": sig.score, "rsi": sig.rsi}),
        )
        .await;

        // Rule arm (#32): rating → action, no model. Same guards, same WAL steps minus
        // the agent ones, so the control arm differs from the LLM arm by exactly the
        // decision function and nothing else.
        if self.cfg.arm == Arm::Rule {
            let positions = self.broker.positions().await?;
            let position_qty = position_qty_for(&positions, symbol);
            let buying_power = self.broker.buying_power().await?;
            let mut decision = rule_decision(symbol, &sig);
            // Size here, not in the guards: a buy is one max_notional clip, a sell is
            // the whole position. The guards still clamp to buying power.
            decision.qty = match decision.action {
                Action::Buy if price > Decimal::ZERO => self.cfg.max_notional / price,
                Action::Sell => position_qty,
                _ => Decimal::ZERO,
            };
            self.append(
                symbol,
                ts,
                "decision",
                &decision_id,
                &json!({"rule": sig.rating, "score": sig.score, "rsi": sig.rsi}),
            )
            .await;
            let guarded = guards::apply(
                decision,
                position_qty,
                buying_power,
                price,
                &GuardConfig {
                    max_notional: self.cfg.max_notional,
                },
            );
            self.append(symbol, ts, "guard", &decision_id, &guard_payload(&guarded))
                .await;
            return self.execute(symbol, ts, &decision_id, guarded).await;
        }

        // 3. analyst agent.
        let analysis = self
            .analyst
            .run(&self.llm, symbol, last_close, &sig, tail(&closes, 10))
            .await?;
        self.append(
            symbol,
            ts,
            "analyst",
            &decision_id,
            &json!({"analysis": analysis}),
        )
        .await;

        // 4. bull/bear debate (optional). Each round both sides argue, reacting to the
        // transcript so far; every turn is a `debate` WAL step so the frontend can show
        // it. Empty when debate_rounds == 0 ⇒ the trader prompt is unchanged.
        let mut debate = String::new();
        let rounds = match self.cfg.arm {
            Arm::LlmDebate => self.cfg.debate_rounds,
            Arm::Rule | Arm::Llm => 0,
        };
        for _round in 0..rounds {
            for r in &self.researchers {
                let turn = r.argue(&self.llm, symbol, &analysis, &sig, &debate).await?;
                self.append(
                    symbol,
                    ts,
                    "debate",
                    &decision_id,
                    &json!({"stance": r.stance(), "text": turn}),
                )
                .await;
                debate.push_str(&format!("[{}] {turn}\n", r.stance()));
            }
        }

        // 5. broker truth (positions + cash) for the decision + guards.
        let positions = self.broker.positions().await?;
        let position_qty = position_qty_for(&positions, symbol);
        let buying_power = self.broker.buying_power().await?;

        // 6. trader agent → strict JSON (parse failure ⇒ Hold, inside the agent).
        let (decision, raw) = self
            .trader
            .run(
                &self.llm,
                TraderInput {
                    symbol,
                    analysis: &analysis,
                    debate: &debate,
                    signal_score: sig.score,
                    position_qty,
                    buying_power,
                },
            )
            .await?;
        self.append(symbol, ts, "decision", &decision_id, &json!({"raw": raw}))
            .await;

        // 7. guards — the last clamp before a real order.
        let guarded = guards::apply(
            decision,
            position_qty,
            buying_power,
            price,
            &GuardConfig {
                max_notional: self.cfg.max_notional,
            },
        );
        self.append(symbol, ts, "guard", &decision_id, &guard_payload(&guarded))
            .await;

        // 8. execute.
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
            let side = if guarded.action == Action::Buy {
                Side::Buy
            } else {
                Side::Sell
            };
            let req = OrderRequest {
                symbol: symbol.clone(),
                qty: guarded.qty,
                side,
                client_order_id: decision_id.to_string(),
            };
            self.append(
                symbol,
                ts,
                "order_intent",
                decision_id,
                &json!({"qty": req.qty.to_string(), "side": side}),
            )
            .await;
            let order = self.broker.submit_order(&req).await?;
            self.append(
                symbol,
                ts,
                "order_result",
                decision_id,
                &json!({"id": order.id, "status": order.status}),
            )
            .await;
        }
        Ok(guarded)
    }

    async fn append(
        &self,
        symbol: &Symbol,
        ts: i64,
        step: &str,
        decision_id: &str,
        payload: &Value,
    ) {
        if let Err(e) = self
            .store
            .append_event(
                &self.cfg.strategy_id,
                symbol,
                ts,
                step,
                decision_id,
                &payload.to_string(),
            )
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

/// The control arm's whole decision function: rating → action. The quantity is a
/// placeholder; the pipeline sizes it (one `max_notional` clip for a buy, the whole
/// position for a sell) and the guards clamp it to buying power.
pub fn rule_decision(symbol: &Symbol, sig: &Signal) -> Decision {
    let action = match sig.rating {
        Rating::StrongBuy | Rating::Buy => Action::Buy,
        Rating::Hold => Action::Hold,
        Rating::Sell | Rating::StrongSell => Action::Sell,
    };
    Decision {
        symbol: symbol.clone(),
        action,
        qty: if action == Action::Hold {
            Decimal::ZERO
        } else {
            Decimal::from(1_000_000_000u64)
        },
        score: sig.score,
        reason: format!(
            "rule {:?}: score {:.3}, RSI {:.1}",
            sig.rating, sig.score, sig.rsi
        ),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::{BrokerError, LlmError, MarketError, StoreError};
    use crate::domain::types::{Bar, Order, Timeframe};
    use async_trait::async_trait;
    use rust_decimal_macros::dec;
    use std::sync::Mutex;

    // Rising series ending at 50 → bullish signal is Some (≥20 bars).
    struct FakeBars;
    #[async_trait]
    impl BarSource for FakeBars {
        async fn bars(
            &self,
            symbol: &Symbol,
            _timeframe: Timeframe,
            _limit: usize,
        ) -> Result<Vec<Bar>, MarketError> {
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

        async fn bars_range(
            &self,
            symbol: &Symbol,
            timeframe: Timeframe,
            start: i64,
            end: i64,
        ) -> Result<Vec<Bar>, MarketError> {
            let mut bars = self.bars(symbol, timeframe, 0).await?;
            bars.retain(|b| b.ts >= start && b.ts < end);
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
        async fn append_event(
            &self,
            _s: &str,
            _sym: &Symbol,
            _ts: i64,
            step: &str,
            _id: &str,
            _p: &str,
        ) -> Result<(), StoreError> {
            self.steps.lock().unwrap().push(step.to_string());
            Ok(())
        }
        async fn write_bars(&self, _bars: &[Bar]) -> Result<(), StoreError> {
            Ok(())
        }
        async fn read_events(
            &self,
            _s: &str,
            _sym: &Symbol,
        ) -> Result<Vec<crate::domain::types::Event>, StoreError> {
            Ok(Vec::new())
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
            PipelineConfig {
                strategy_id: "test".into(),
                arm: Arm::Llm,
                bar_window: 60,
                max_notional: dec!(100),
                debate_rounds: 0,
                force_side: None,
            },
        );

        let d = pipe.run_symbol(&Symbol::from("BTC/USD")).await.unwrap();

        // max_notional 100 @ price 50 → qty clamped from 10 to 2.
        assert_eq!(d.action, Action::Buy);
        assert_eq!(d.qty, dec!(2));
        assert_eq!(*pipe.broker.submitted_qty.lock().unwrap(), Some(dec!(2)));

        let steps = pipe.store.steps.lock().unwrap();
        let intent = steps.iter().position(|s| s == "order_intent").unwrap();
        let result = steps.iter().position(|s| s == "order_result").unwrap();
        assert!(
            intent < result,
            "WAL: intent must precede result — {steps:?}"
        );
    }

    #[tokio::test]
    async fn rule_arm_never_calls_the_model_and_still_trades_through_guards() {
        let store = FakeStore::default();
        let broker = FakeBroker::default();
        let pipe = Pipeline::new(
            FakeBars,
            NeverLlm,
            store,
            broker,
            PipelineConfig {
                strategy_id: "test".into(),
                arm: Arm::Rule,
                bar_window: 60,
                max_notional: dec!(100),
                debate_rounds: 1,
                force_side: None,
            },
        );
        // FakeBars is a steady uptrend → the rule rates it a buy.
        let d = pipe.run_symbol(&Symbol::from("NVDA")).await.unwrap();
        assert_eq!(d.action, Action::Buy);
        assert_eq!(d.qty, dec!(2), "guards size it: 100 notional @ 50");
        assert!(d.reason.starts_with("rule "), "{}", d.reason);
        let steps = pipe.store.steps.lock().unwrap();
        assert!(!steps.iter().any(|s| s == "analyst" || s == "debate"));
        assert!(steps.iter().any(|s| s == "decision"));
        let intent = steps.iter().position(|s| s == "order_intent");
        let result = steps.iter().position(|s| s == "order_result");
        assert!(intent.is_some() && intent < result);
    }

    #[test]
    fn rule_maps_every_rating_to_an_action() {
        let sym = Symbol::from("X");
        let sig = |rating| Signal {
            score: 0.0,
            rating,
            rsi: 50.0,
        };
        assert_eq!(
            rule_decision(&sym, &sig(Rating::StrongBuy)).action,
            Action::Buy
        );
        assert_eq!(rule_decision(&sym, &sig(Rating::Buy)).action, Action::Buy);
        assert_eq!(rule_decision(&sym, &sig(Rating::Hold)).action, Action::Hold);
        assert_eq!(rule_decision(&sym, &sig(Rating::Sell)).action, Action::Sell);
        assert_eq!(
            rule_decision(&sym, &sig(Rating::StrongSell)).action,
            Action::Sell
        );
        assert_eq!(rule_decision(&sym, &sig(Rating::Hold)).qty, Decimal::ZERO);
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
                arm: Arm::Llm,
                bar_window: 60,
                max_notional: dec!(100),
                debate_rounds: 0,
                force_side: Some(Side::Buy),
            },
        );

        let d = pipe.run_symbol(&Symbol::from("BTC/USD")).await.unwrap();

        // Forced buy, clamped to max_notional 100 @ price 50 → qty 2, no LLM call.
        assert_eq!(d.action, Action::Buy);
        assert_eq!(d.qty, dec!(2));
        assert_eq!(*pipe.broker.submitted_qty.lock().unwrap(), Some(dec!(2)));
    }

    // Records every system prompt seen, and the user message the trader (JSON) call got.
    // Prose for the analyst/researchers, strict JSON for the trader.
    #[derive(Default)]
    struct RecordingLlm {
        trader_user: Mutex<String>,
    }
    #[async_trait]
    impl LlmClient for RecordingLlm {
        async fn message(&self, system: &str, user: &str) -> Result<String, LlmError> {
            if system.contains("JSON") {
                *self.trader_user.lock().unwrap() = user.to_string();
                Ok(r#"{"action":"BUY","qty":10,"score":0.8,"reason":"test"}"#.into())
            } else {
                Ok("prose".into())
            }
        }
    }

    #[tokio::test]
    async fn debate_runs_both_sides_and_trader_sees_transcript() {
        let pipe = Pipeline::new(
            FakeBars,
            RecordingLlm::default(),
            FakeStore::default(),
            FakeBroker::default(),
            PipelineConfig {
                strategy_id: "test".into(),
                arm: Arm::LlmDebate,
                bar_window: 60,
                max_notional: dec!(100),
                debate_rounds: 1,
                force_side: None,
            },
        );

        let d = pipe.run_symbol(&Symbol::from("BTC/USD")).await.unwrap();
        assert_eq!(d.action, Action::Buy);

        // One round ⇒ exactly two debate turns persisted (bull + bear).
        let debates = pipe
            .store
            .steps
            .lock()
            .unwrap()
            .iter()
            .filter(|s| *s == "debate")
            .count();
        assert_eq!(debates, 2, "one round = bull + bear");

        // The trader's prompt carried the full transcript.
        let trader_user = pipe.llm.trader_user.lock().unwrap();
        assert!(
            trader_user.contains("Bull/bear debate:")
                && trader_user.contains("[bull]")
                && trader_user.contains("[bear]"),
            "trader must see the debate transcript — {trader_user}"
        );
    }
}
