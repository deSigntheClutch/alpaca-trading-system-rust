//! Deterministic safety guards — the only thing between a bad LLM output and a real
//! (paper) order, so pure + unit-tested per rule. Every doubt collapses to `Hold`.
//!
//! Rules:
//!   - Hold / non-positive qty or price          → Hold
//!   - BUY notional capped at `max_notional`      → clamp qty
//!   - BUY beyond buying power                     → clamp qty (→ Hold if it hits 0)
//!   - SELL bounded by the held position           → clamp qty (→ Hold if flat; no shorting)
//!
//! Crypto allows fractional qty, so there's no whole-share rounding.

use rust_decimal::Decimal;

use crate::domain::types::{Action, Decision};

/// Tunables for the guards. `max_notional` is the hard ceiling on any single order.
pub struct GuardConfig {
    pub max_notional: Decimal,
}

/// Apply every guard to a raw decision, returning a safe-to-execute decision.
/// `price` is the reference price used to convert qty↔notional.
pub fn apply(
    decision: Decision,
    position_qty: Decimal,
    buying_power: Decimal,
    price: Decimal,
    cfg: &GuardConfig,
) -> Decision {
    let zero = Decimal::ZERO;

    if decision.action == Action::Hold {
        return decision; // already a no-op
    }
    if decision.qty <= zero || price <= zero {
        return hold(decision, "non-positive qty or price");
    }

    match decision.action {
        Action::Buy => {
            let cap_notional = cfg.max_notional / price;
            let cap_bp = if buying_power > zero {
                buying_power / price
            } else {
                zero
            };
            let capped = decision.qty.min(cap_notional).min(cap_bp);
            if capped <= zero {
                return hold(decision, "buy blocked: no buying power");
            }
            clamp_qty(decision, capped)
        }
        Action::Sell => {
            if position_qty <= zero {
                return hold(decision, "sell blocked: no position (no shorting)");
            }
            let capped = decision.qty.min(position_qty);
            clamp_qty(decision, capped)
        }
        Action::Hold => unreachable!("handled above"),
    }
}

fn clamp_qty(mut d: Decision, capped: Decimal) -> Decision {
    if capped < d.qty {
        d.reason = format!("{} [guard: qty {}→{}]", d.reason, d.qty, capped);
        d.qty = capped;
    }
    d
}

fn hold(mut d: Decision, why: &str) -> Decision {
    d.reason = format!("{} [guard: {} → HOLD]", d.reason, why);
    d.action = Action::Hold;
    d.qty = Decimal::ZERO;
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::Symbol;
    use rust_decimal_macros::dec;

    fn cfg() -> GuardConfig {
        GuardConfig {
            max_notional: dec!(100),
        }
    }

    fn decision(action: Action, qty: Decimal) -> Decision {
        Decision {
            symbol: Symbol::from("BTC/USD"),
            action,
            qty,
            score: 0.5,
            reason: "test".into(),
        }
    }

    #[test]
    fn buy_over_notional_is_clamped() {
        // qty 10 @ price 50 = 500 notional, cap 100 → qty 2.
        let d = apply(
            decision(Action::Buy, dec!(10)),
            dec!(0),
            dec!(1_000_000),
            dec!(50),
            &cfg(),
        );
        assert_eq!(d.action, Action::Buy);
        assert_eq!(d.qty, dec!(2));
    }

    #[test]
    fn buy_with_no_buying_power_becomes_hold() {
        let d = apply(
            decision(Action::Buy, dec!(1)),
            dec!(0),
            dec!(0),
            dec!(50),
            &cfg(),
        );
        assert_eq!(d.action, Action::Hold);
        assert_eq!(d.qty, dec!(0));
    }

    #[test]
    fn sell_is_bounded_by_position() {
        // want to sell 5, only hold 1.5 → clamp to 1.5.
        let d = apply(
            decision(Action::Sell, dec!(5)),
            dec!(1.5),
            dec!(0),
            dec!(50),
            &cfg(),
        );
        assert_eq!(d.action, Action::Sell);
        assert_eq!(d.qty, dec!(1.5));
    }

    #[test]
    fn sell_with_no_position_becomes_hold() {
        let d = apply(
            decision(Action::Sell, dec!(5)),
            dec!(0),
            dec!(0),
            dec!(50),
            &cfg(),
        );
        assert_eq!(d.action, Action::Hold);
    }

    #[test]
    fn hold_and_nonpositive_qty_pass_through_as_hold() {
        assert_eq!(
            apply(
                decision(Action::Hold, dec!(0)),
                dec!(0),
                dec!(0),
                dec!(50),
                &cfg()
            )
            .action,
            Action::Hold
        );
        assert_eq!(
            apply(
                decision(Action::Buy, dec!(0)),
                dec!(0),
                dec!(100),
                dec!(50),
                &cfg()
            )
            .action,
            Action::Hold
        );
    }
}
