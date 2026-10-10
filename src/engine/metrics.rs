//! Backtest accounting (#11): pair fills into per-lot round trips and turn an equity
//! curve into the numbers a methodology review asks for. Pure functions over plain
//! values — no ports, no async — so every rule here is unit-tested on canned input.
//!
//! Trades are **per lot, FIFO**: each buy fill opens its own lot, a sell closes lots
//! oldest-first and emits one `Trade` per lot it touches. Every buy marker on the chart
//! therefore maps to exactly one row, and a sell that closes three lots produces three
//! rows sharing an `exit_ts`. Blending scale-ins into one averaged position would be
//! simpler but loses the marker-to-row link and hides the per-entry outcome.

use std::collections::HashMap;

use crate::domain::dto::{Metrics, Trade};
use crate::domain::types::Side;
use crate::engine::backtest::Fill;

/// Trading sessions per year, for annualising daily Sharpe.
pub const SESSIONS_PER_YEAR: f64 = 252.0;

/// One open lot, consumed FIFO by later sells.
struct Lot {
    entry_ts: i64,
    entry_price: f64,
    /// Quantity the lot opened with; fee is prorated against it.
    opened: f64,
    remaining: f64,
    entry_fee: f64,
    entry_reason: String,
}

/// Pair `fills` (chronological) into round trips. `reasons` maps the decision day
/// (`Fill::decided_ts`) to the decision's reason text. Lots still open at the end are
/// emitted with `exit_* = None` and an **unrealised** `pnl` marked at `last_close`,
/// net of their entry fee, so the sum of `pnl` over all rows equals the strategy's
/// profit at the last close.
pub fn pair_trades(
    symbol: &str,
    fills: &[Fill],
    reasons: &HashMap<i64, String>,
    last_ts: i64,
    last_close: f64,
) -> Vec<Trade> {
    let reason = |ts: i64| reasons.get(&ts).cloned().unwrap_or_default();
    let mut open: Vec<Lot> = Vec::new();
    let mut trades = Vec::new();

    for f in fills {
        match f.side {
            Side::Buy => open.push(Lot {
                entry_ts: f.ts,
                entry_price: f.price,
                opened: f.qty,
                remaining: f.qty,
                entry_fee: f.fee,
                entry_reason: reason(f.decided_ts),
            }),
            Side::Sell => {
                let exit_fee_per_unit = if f.qty > 0.0 { f.fee / f.qty } else { 0.0 };
                let exit_reason = reason(f.decided_ts);
                let mut left = f.qty;
                while left > 1e-12 {
                    let Some(lot) = open.first_mut() else { break };
                    let take = left.min(lot.remaining);
                    let fees = lot.entry_fee * (take / lot.opened) + exit_fee_per_unit * take;
                    let pnl = (f.price - lot.entry_price) * take - fees;
                    trades.push(Trade {
                        symbol: symbol.to_string(),
                        entry_ts: lot.entry_ts,
                        entry_price: lot.entry_price,
                        exit_ts: Some(f.ts),
                        exit_price: Some(f.price),
                        qty: take,
                        pnl,
                        pnl_pct: pct(pnl, lot.entry_price * take),
                        fees,
                        entry_reason: lot.entry_reason.clone(),
                        exit_reason: exit_reason.clone(),
                    });
                    lot.remaining -= take;
                    left -= take;
                    if lot.remaining <= 1e-12 {
                        open.remove(0);
                    }
                }
            }
        }
    }

    for lot in open {
        let fees = lot.entry_fee * (lot.remaining / lot.opened);
        let pnl = (last_close - lot.entry_price) * lot.remaining - fees;
        trades.push(Trade {
            symbol: symbol.to_string(),
            entry_ts: lot.entry_ts,
            entry_price: lot.entry_price,
            exit_ts: None,
            exit_price: None,
            qty: lot.remaining,
            pnl,
            pnl_pct: pct(pnl, lot.entry_price * lot.remaining),
            fees,
            entry_reason: lot.entry_reason,
            exit_reason: format!("open at {last_ts}: marked at the last close"),
        });
    }
    trades
}

fn pct(pnl: f64, basis: f64) -> f64 {
    if basis > 0.0 {
        pnl / basis
    } else {
        0.0
    }
}

/// Worst peak-to-trough drop as a positive fraction (0.3 = a 30% drawdown).
pub fn max_drawdown(equity: &[f64]) -> f64 {
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

/// Annualised Sharpe of the daily returns of `equity`, risk-free rate zero. `0.0`
/// when there are fewer than two returns or no variance, rather than NaN or ±inf:
/// a flat curve is "no evidence", not "infinitely good".
pub fn sharpe(equity: &[f64]) -> f64 {
    let rets: Vec<f64> = equity
        .windows(2)
        .filter(|w| w[0] > 0.0)
        .map(|w| w[1] / w[0] - 1.0)
        .collect();
    if rets.len() < 2 {
        return 0.0;
    }
    let n = rets.len() as f64;
    let mean = rets.iter().sum::<f64>() / n;
    let var = rets.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1.0);
    let sd = var.sqrt();
    if sd <= 0.0 || !sd.is_finite() {
        return 0.0;
    }
    mean / sd * SESSIONS_PER_YEAR.sqrt()
}

/// Everything the summary cards show, from the equity curve, the paired trades and
/// the daily exposure series (position value ÷ equity, one entry per day).
pub fn summarize(
    equity: &[f64],
    starting_cash: f64,
    trades: &[Trade],
    exposure_series: &[f64],
    buy_hold_return: f64,
    total_fees: f64,
) -> Metrics {
    let final_equity = equity.last().copied().unwrap_or(starting_cash);
    let total_return = if starting_cash > 0.0 {
        final_equity / starting_cash - 1.0
    } else {
        0.0
    };

    let closed: Vec<&Trade> = trades.iter().filter(|t| t.exit_ts.is_some()).collect();
    let wins: Vec<f64> = closed
        .iter()
        .filter(|t| t.pnl > 0.0)
        .map(|t| t.pnl)
        .collect();
    let losses: Vec<f64> = closed
        .iter()
        .filter(|t| t.pnl < 0.0)
        .map(|t| t.pnl)
        .collect();
    let mean = |xs: &[f64]| {
        if xs.is_empty() {
            0.0
        } else {
            xs.iter().sum::<f64>() / xs.len() as f64
        }
    };
    let gross_wins: f64 = wins.iter().sum();
    let gross_losses: f64 = -losses.iter().sum::<f64>();
    // `None` rather than +inf: serde writes non-finite floats as null anyway, and the
    // page renders null as ∞ when there were wins.
    let profit_factor = if gross_losses > 0.0 {
        Some(gross_wins / gross_losses)
    } else {
        None
    };

    Metrics {
        total_return,
        max_drawdown: max_drawdown(equity),
        sharpe: sharpe(equity),
        num_trades: closed.len() as u32,
        win_rate: if closed.is_empty() {
            0.0
        } else {
            wins.len() as f64 / closed.len() as f64
        },
        avg_win: mean(&wins),
        avg_loss: mean(&losses),
        profit_factor,
        exposure: mean(exposure_series),
        buy_hold_return,
        total_fees,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill(ts: i64, side: Side, qty: f64, price: f64, fee: f64) -> Fill {
        Fill {
            ts,
            decided_ts: ts - 1,
            side,
            qty,
            price,
            fee,
            reference: price,
        }
    }

    #[test]
    fn fifo_pairs_lots_and_prorates_fees() {
        // Two lots, then one sell that closes the first lot and half the second.
        let fills = vec![
            fill(1, Side::Buy, 1.0, 100.0, 1.0),
            fill(2, Side::Buy, 2.0, 110.0, 2.0),
            fill(3, Side::Sell, 2.0, 120.0, 4.0), // 2 per unit of exit fee
        ];
        let reasons = HashMap::from([(0, "first".to_string()), (2, "sell".to_string())]);
        let trades = pair_trades("X", &fills, &reasons, 9, 130.0);
        assert_eq!(
            trades.len(),
            3,
            "lot 1 closed, lot 2 split: closed half + open half"
        );

        let t0 = &trades[0];
        assert_eq!(
            (t0.entry_price, t0.exit_price, t0.qty),
            (100.0, Some(120.0), 1.0)
        );
        assert_eq!(t0.fees, 1.0 + 2.0, "whole entry fee + 1 unit of exit fee");
        assert_eq!(t0.pnl, 20.0 - 3.0);
        assert_eq!(t0.entry_reason, "first");
        assert_eq!(t0.exit_reason, "sell");

        let t1 = &trades[1];
        assert_eq!((t1.entry_price, t1.qty), (110.0, 1.0));
        assert_eq!(
            t1.fees,
            2.0 * 0.5 + 2.0,
            "half the entry fee + 1 unit of exit fee"
        );
        assert_eq!(t1.pnl, 10.0 - 3.0);

        let open = &trades[2];
        assert_eq!(open.exit_ts, None);
        assert_eq!(open.qty, 1.0);
        assert_eq!(open.fees, 1.0, "the other half of lot 2's entry fee");
        assert_eq!(open.pnl, (130.0 - 110.0) - 1.0, "marked at the last close");
        assert!(open.exit_reason.starts_with("open"));
    }

    #[test]
    fn pnl_sums_to_profit_at_last_close() {
        let fills = vec![
            fill(1, Side::Buy, 3.0, 50.0, 0.3),
            fill(2, Side::Sell, 1.0, 60.0, 0.1),
        ];
        let trades = pair_trades("X", &fills, &HashMap::new(), 5, 55.0);
        let total: f64 = trades.iter().map(|t| t.pnl).sum();
        // cash: -150 -0.3 +60 -0.1 = -90.4; position 2 × 55 = 110; profit = 19.6
        assert!((total - 19.6).abs() < 1e-9);
    }

    #[test]
    fn drawdown_and_sharpe_on_canned_curves() {
        assert_eq!(max_drawdown(&[100.0, 120.0, 90.0, 130.0]), 0.25);
        assert_eq!(max_drawdown(&[100.0, 101.0, 102.0]), 0.0);
        // Flat curve: no variance → 0, not NaN.
        assert_eq!(sharpe(&[100.0, 100.0, 100.0]), 0.0);
        assert_eq!(sharpe(&[100.0]), 0.0);
        // Alternating +1% / -1%: zero mean, 1% sd → a Sharpe of about zero.
        let mut eq = vec![100.0];
        for i in 0..40 {
            let r = if i % 2 == 0 { 1.01 } else { 0.99 };
            eq.push(eq.last().unwrap() * r);
        }
        let s = sharpe(&eq);
        assert!(s.abs() < 0.2, "sharpe={s}");
        // Steady +0.1%/day: positive, large (near-zero variance from float noise is
        // still variance, so the value is finite and clearly positive).
        let steady: Vec<f64> = (0..30).map(|i| 100.0 * 1.001f64.powi(i)).collect();
        let s = sharpe(&steady);
        assert!(s.is_finite() && s > 10.0, "sharpe={s}");
    }

    #[test]
    fn summary_counts_only_closed_trades_and_never_divides_by_zero() {
        let m = summarize(&[100.0, 110.0], 100.0, &[], &[0.0, 0.5], 0.05, 1.5);
        assert_eq!(m.num_trades, 0);
        assert_eq!(m.win_rate, 0.0);
        assert_eq!(m.profit_factor, None);
        assert!((m.total_return - 0.1).abs() < 1e-12);
        assert_eq!(m.exposure, 0.25);
        assert_eq!(m.total_fees, 1.5);

        let fills = vec![
            fill(1, Side::Buy, 1.0, 100.0, 0.0),
            fill(2, Side::Sell, 1.0, 110.0, 0.0),
            fill(3, Side::Buy, 1.0, 100.0, 0.0),
            fill(4, Side::Sell, 1.0, 95.0, 0.0),
            fill(5, Side::Buy, 1.0, 100.0, 0.0),
        ];
        let trades = pair_trades("X", &fills, &HashMap::new(), 6, 100.0);
        let m = summarize(&[100.0, 110.0, 105.0], 100.0, &trades, &[1.0], 0.0, 0.0);
        assert_eq!(m.num_trades, 2, "the open lot is listed but not counted");
        assert_eq!(m.win_rate, 0.5);
        assert_eq!(m.avg_win, 10.0);
        assert_eq!(m.avg_loss, -5.0);
        assert_eq!(m.profit_factor, Some(2.0));
    }
}
