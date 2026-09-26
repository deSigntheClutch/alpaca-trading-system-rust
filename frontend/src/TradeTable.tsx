/**
 * Round trips, with the answer to "did this one make money" in the last two
 * columns. Entry and exit dates are separately clickable, so you can jump the
 * drill-down to either leg and read what the system said at that moment.
 *
 * Sorting defaults to chronological but the P&L columns are sortable, because
 * the first question anyone asks of a trade list is "show me the worst ones".
 */
import { useMemo, useState } from 'react'
import { date, daysBetween, price, qty, signedMoney, signedPct, money } from './format'
import type { Trade } from './api/types'

type SortKey = 'entry_ts' | 'pnl' | 'pnl_pct' | 'held'

type Props = {
  trades: Trade[]
  selectedTs: number | null
  onSelect: (tsMillis: number) => void
  /** Last bar in the window, used to age a position that is still open. */
  lastTs: number
}

export function TradeTable({ trades, selectedTs, onSelect, lastTs }: Props) {
  const [sort, setSort] = useState<SortKey>('entry_ts')
  const [asc, setAsc] = useState(true)

  const held = (t: Trade) => daysBetween(t.entry_ts, t.exit_ts ?? lastTs)

  const rows = useMemo(() => {
    const key = (t: Trade) =>
      sort === 'entry_ts' ? t.entry_ts : sort === 'pnl' ? t.pnl : sort === 'pnl_pct' ? t.pnl_pct : held(t)
    return [...trades].sort((a, b) => (key(a) - key(b)) * (asc ? 1 : -1))
    // `held` closes over lastTs, which is stable for a given result.
  }, [trades, sort, asc, lastTs])

  const toggle = (k: SortKey) => {
    if (k === sort) setAsc((v) => !v)
    else {
      setSort(k)
      setAsc(k === 'entry_ts')
    }
  }

  if (!trades.length) {
    return (
      <div className="pane">
        <h3>Trades</h3>
        <p className="hint">
          No positions were opened in this window. Either the signal never crossed its threshold, or
          every decision was clamped to a Hold.
        </p>
      </div>
    )
  }

  const closed = trades.filter((t) => t.exit_ts != null)
  const net = trades.reduce((a, t) => a + t.pnl, 0)

  return (
    <div className="pane">
      <h3>
        Trades <span className="dim">({closed.length} closed{trades.length > closed.length ? `, ${trades.length - closed.length} open` : ''})</span>
        <span className={`right ${net >= 0 ? 'buy' : 'sell'}`}>{signedMoney(net)} total</span>
      </h3>
      <div className="scroll">
        <table>
          <thead>
            <tr>
              <Th onClick={() => toggle('entry_ts')} on={sort === 'entry_ts'} asc={asc}>Entry</Th>
              <th>In</th>
              <th>Exit</th>
              <th>Out</th>
              <Th onClick={() => toggle('held')} on={sort === 'held'} asc={asc}>Held</Th>
              <th>Qty</th>
              <th>Fees</th>
              <Th onClick={() => toggle('pnl')} on={sort === 'pnl'} asc={asc}>Net P&amp;L</Th>
              <Th onClick={() => toggle('pnl_pct')} on={sort === 'pnl_pct'} asc={asc}>%</Th>
            </tr>
          </thead>
          <tbody>
            {rows.map((t) => {
              const open = t.exit_ts == null
              const on = selectedTs === t.entry_ts || (t.exit_ts != null && selectedTs === t.exit_ts)
              return (
                <tr
                  key={`${t.entry_ts}-${t.exit_ts ?? 'open'}`}
                  className={on ? 'sel' : ''}
                  onClick={() => onSelect(t.entry_ts)}
                >
                  <td>
                    <button className="linkish" onClick={(e) => { e.stopPropagation(); onSelect(t.entry_ts) }}>
                      {date(t.entry_ts)}
                    </button>
                  </td>
                  <td>{price(t.entry_price)}</td>
                  <td>
                    {open ? (
                      <span className="tag open">open</span>
                    ) : (
                      <button className="linkish" onClick={(e) => { e.stopPropagation(); onSelect(t.exit_ts!) }}>
                        {date(t.exit_ts!)}
                      </button>
                    )}
                  </td>
                  <td>{open ? '—' : price(t.exit_price!)}</td>
                  <td>{held(t)}d</td>
                  <td>{qty(t.qty)}</td>
                  <td className="dim">{money(t.fees)}</td>
                  <td className={t.pnl >= 0 ? 'buy' : 'sell'}>{signedMoney(t.pnl)}</td>
                  <td className={t.pnl >= 0 ? 'buy' : 'sell'}>{signedPct(t.pnl_pct)}</td>
                </tr>
              )
            })}
          </tbody>
        </table>
      </div>
      {trades.length > closed.length && (
        <p className="hint">
          An open position is marked to the last close, so its P&amp;L is unrealised and will move.
        </p>
      )}
    </div>
  )
}

function Th({
  children, onClick, on, asc,
}: {
  children: React.ReactNode
  onClick: () => void
  on: boolean
  asc: boolean
}) {
  return (
    <th className={`sortable ${on ? 'on' : ''}`} onClick={onClick}>
      {children}
      <span className="caret">{on ? (asc ? '▲' : '▼') : ''}</span>
    </th>
  )
}
