/**
 * What the system saw on one day, what it decided, and how that turned out.
 *
 * The outcome line is the part that matters. A reason on its own is a story; a
 * reason next to the realised P&L of the trade it opened is evidence. Whenever
 * the selected day is an entry or an exit, that link is drawn here.
 */
import { date, daysBetween, price, qty, signedMoney, signedPct } from './format'
import type { DayRecord, Trade } from './api/types'

type Props = {
  day: DayRecord | null
  trades: Trade[]
  lastTs: number
}

export function DecisionDetail({ day, trades, lastTs }: Props) {
  if (!day) {
    return (
      <div className="pane">
        <h3>Why this decision</h3>
        <p className="hint">
          Click a marker on the chart, or a row in the trade table, to see the signal the system read
          that day and the words it used to justify the trade.
        </p>
      </div>
    )
  }

  // A buy opens one lot, but a sell can close several at once, so both sides
  // are lists. Showing only the first would quietly hide the rest of the P&L.
  const asEntry = trades.filter((t) => t.entry_ts === day.ts)
  const asExit = trades.filter((t) => t.exit_ts === day.ts)

  return (
    <div className="pane">
      <h3>Why this decision</h3>

      <div className="row detail-head">
        <span className={`tag ${day.action.toLowerCase()}`}>{day.action}</span>
        <strong>{date(day.ts)}</strong>
        <span className="dim">close {price(day.close)}</span>
        {day.qty > 0 && <span className="dim">qty {qty(day.qty)}</span>}
      </div>

      <ScoreBar score={day.score} rsi={day.rsi} />

      {asEntry.map((t) => (
        <Outcome key={`in-${t.entry_ts}`} trade={t} leg="entry" lastTs={lastTs} />
      ))}
      {asExit.length > 1 && (
        <ClosedTogether trades={asExit} />
      )}
      {asExit.map((t) => (
        <Outcome key={`out-${t.entry_ts}`} trade={t} leg="exit" lastTs={lastTs} />
      ))}

      <div className="field">
        <div className="field-label">Decision</div>
        <p>{day.decision_reason || '—'}</p>
      </div>

      <div className="field">
        <div className="field-label">Analyst</div>
        {day.analyst_text ? <pre>{day.analyst_text}</pre> : <p className="hint">Nothing recorded for this day.</p>}
      </div>
    </div>
  )
}

/** One sell can close several lots; lead with the combined number. */
function ClosedTogether({ trades }: { trades: Trade[] }) {
  const net = trades.reduce((a, t) => a + t.pnl, 0)
  return (
    <div className={`outcome ${net >= 0 ? 'good' : 'bad'}`}>
      <strong>Outcome</strong> This sell closed {trades.length} lots at once, {signedMoney(net)} net in
      total. Each leg is broken out below.
    </div>
  )
}

/** Ties a decision to the money it made or lost. */
function Outcome({ trade, leg, lastTs }: { trade: Trade; leg: 'entry' | 'exit'; lastTs: number }) {
  const open = trade.exit_ts == null
  const held = daysBetween(trade.entry_ts, trade.exit_ts ?? lastTs)
  const good = trade.pnl >= 0

  const text = open
    ? `Still open after ${held} days, marked at ${signedMoney(trade.pnl)} (${signedPct(trade.pnl_pct)}) unrealised.`
    : leg === 'entry'
      ? `Closed ${held} days later on ${date(trade.exit_ts!)} for ${signedMoney(trade.pnl)} (${signedPct(trade.pnl_pct)}) net of fees.`
      : `Closed a position opened ${held} days earlier on ${date(trade.entry_ts)}, for ${signedMoney(trade.pnl)} (${signedPct(trade.pnl_pct)}) net of fees.`

  return (
    <div className={`outcome ${open ? 'flat' : good ? 'good' : 'bad'}`}>
      <strong>Outcome</strong> {text}
    </div>
  )
}

/**
 * Where the composite score sits in [-1, 1], with the ±0.2 rule thresholds
 * drawn in, so "score 0.31" reads as "past the buy line" at a glance.
 */
function ScoreBar({ score, rsi }: { score: number; rsi: number }) {
  const left = ((Math.max(-1, Math.min(1, score)) + 1) / 2) * 100
  return (
    <div className="field">
      <div className="field-label">
        Signal <span className="dim">score {score.toFixed(3)} · RSI {rsi.toFixed(1)}</span>
      </div>
      <div className="scorebar">
        <div className="zone sell-zone" />
        <div className="zone hold-zone" />
        <div className="zone buy-zone" />
        <div className="needle" style={{ left: `${left}%` }} />
      </div>
      <div className="scale">
        <span>-1 sell</span>
        <span>hold</span>
        <span>buy +1</span>
      </div>
    </div>
  )
}
