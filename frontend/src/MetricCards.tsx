/**
 * The summary scorecard.
 *
 * One deliberate choice here: buy-and-hold is shown twice, once raw and once
 * scaled by the strategy's average exposure. A strategy that is in the market a
 * third of the time is not competing with a fully invested benchmark, and
 * printing only the raw number is how a losing strategy gets to look like a
 * winner (or vice versa). The scaled figure is a first-order approximation, not
 * a risk model, and is labelled as such.
 */
import { money, pct, signedPct } from './format'
import type { Metrics } from './api/types'

type Props = { metrics: Metrics }

export function MetricCards({ metrics: m }: Props) {
  const matched = m.buy_hold_return * m.exposure
  const beat = m.total_return - matched

  return (
    <div className="cards">
      <Card
        label="Net return"
        value={signedPct(m.total_return)}
        tone={m.total_return >= 0 ? 'good' : 'bad'}
        note="after fees and slippage"
      />
      <Card
        label="Buy and hold"
        value={signedPct(m.buy_hold_return)}
        tone="flat"
        note="fully invested the whole window"
      />
      <Card
        label="Exposure-matched"
        value={signedPct(matched)}
        tone={beat >= 0 ? 'good' : 'bad'}
        note={`benchmark × ${pct(m.exposure, 0)} exposure · you ${beat >= 0 ? 'beat' : 'trail'} it by ${signedPct(beat)}`}
      />
      <Card
        label="Sharpe"
        value={m.sharpe.toFixed(2)}
        tone={m.sharpe >= 1 ? 'good' : m.sharpe >= 0 ? 'flat' : 'bad'}
        note="annualised, daily returns"
      />
      <Card
        label="Max drawdown"
        value={pct(m.max_drawdown)}
        tone={m.max_drawdown > 0.25 ? 'bad' : 'flat'}
        note="worst peak to trough"
      />
      <Card
        label="Win rate"
        value={m.num_trades ? pct(m.win_rate, 0) : '—'}
        tone="flat"
        note={
          m.num_trades
            ? `${m.num_trades} closed · avg win ${money(m.avg_win)} · avg loss ${money(m.avg_loss)}`
            : 'no closed trades'
        }
      />
      <Card
        label="Profit factor"
        value={profitFactor(m)}
        tone={m.num_trades === 0 ? 'flat' : m.profit_factor == null || m.profit_factor >= 1 ? 'good' : 'bad'}
        note={m.profit_factor == null && m.num_trades > 0 ? 'no losing trades' : 'gross wins ÷ gross losses'}
      />
      <Card
        label="Exposure"
        value={pct(m.exposure, 0)}
        tone="flat"
        note="average share of the window holding a position"
      />
      <Card
        label="Fees paid"
        value={money(m.total_fees)}
        tone={m.total_fees > 0 ? 'bad' : 'flat'}
        note="the cost of turnover"
      />
    </div>
  )
}

/** null means every trade won, so the ratio is unbounded rather than missing. */
function profitFactor(m: Metrics): string {
  if (m.num_trades === 0) return '—'
  return m.profit_factor == null ? '∞' : m.profit_factor.toFixed(2)
}

function Card({
  label, value, tone, note,
}: {
  label: string
  value: string
  tone: 'good' | 'bad' | 'flat'
  note: string
}) {
  return (
    <div className="card">
      <div className="card-label">{label}</div>
      <div className={`card-value ${tone}`}>{value}</div>
      <div className="card-note">{note}</div>
    </div>
  )
}
