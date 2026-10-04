'use client'

import type { Pick } from '@/bindings/Pick'
import { price, signedPct } from '@/format'

const cls = (x: number | null) => (x == null ? '' : x > 0 ? 'pos' : x < 0 ? 'neg' : '')

/** The day's buy candidates. Click a row for the detail drawer. */
export function PicksTable({
  picks,
  selected,
  onSelect,
}: {
  picks: Pick[]
  selected: string | null
  onSelect: (symbol: string | null) => void
}) {
  if (picks.length === 0) return <p className="hint">No picks: nothing in the universe scored.</p>
  return (
    <div className="scroll">
      <table className="table picks">
        <thead>
          <tr>
            <th>sym</th>
            <th>name</th>
            <th>sector · themes</th>
            <th className="num">score</th>
            <th>rating</th>
            <th className="num">close</th>
            <th className="num">5d</th>
            <th className="num">20d</th>
            <th>earnings</th>
            <th className="num">news</th>
            <th>note</th>
          </tr>
        </thead>
        <tbody>
          {picks.map((p) => {
            const s = p.scored
            const on = s.symbol === selected
            return (
              <tr
                key={s.symbol}
                className={on ? 'sel' : ''}
                onClick={() => onSelect(on ? null : s.symbol)}
                tabIndex={0}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' || e.key === ' ') onSelect(on ? null : s.symbol)
                }}
              >
                <td>
                  <code>{s.symbol}</code>
                </td>
                <td>{s.name}</td>
                <td>
                  <span className="chip-row">
                    <span className="chip">{s.sector}</span>
                    {s.themes.slice(0, 3).map((t) => (
                      <span key={t} className="tag">
                        {t}
                      </span>
                    ))}
                  </span>
                </td>
                <td className="num">{s.score.toFixed(2)}</td>
                <td>{s.rating}</td>
                <td className="num">{price(s.close)}</td>
                <td className={`num ${cls(s.change_5d)}`}>{s.change_5d == null ? '–' : signedPct(s.change_5d)}</td>
                <td className={`num ${cls(s.change_20d)}`}>{s.change_20d == null ? '–' : signedPct(s.change_20d)}</td>
                <td>{p.earnings ? p.earnings.date : <span className="dim">–</span>}</td>
                <td className="num">{p.headlines.length}</td>
                <td>{p.commentary ? <span className="badge">{p.commentary.watch_reason}</span> : <span className="dim">–</span>}</td>
              </tr>
            )
          })}
        </tbody>
      </table>
    </div>
  )
}
