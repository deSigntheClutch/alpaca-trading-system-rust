'use client'

import type { WatchItem } from '@/bindings/WatchItem'
import { price, signedPct } from '@/format'

const cls = (x: number | null) => (x == null ? '' : x > 0 ? 'pos' : x < 0 ? 'neg' : '')

/**
 * The owner's always-covered names with where the rule ranked them today. Same
 * columns as the picks plus the rank; click a row for the same detail drawer.
 */
export function WatchlistTable({
  items,
  selected,
  onSelect,
}: {
  items: WatchItem[]
  selected: string | null
  onSelect: (symbol: string | null) => void
}) {
  if (items.length === 0) {
    return <p className="hint">No watchlist: mark names with a 1 in the `watch` column of config/universe.csv.</p>
  }
  return (
    <div className="scroll">
      <table className="table picks">
        <thead>
          <tr>
            <th className="num">rank</th>
            <th>sym</th>
            <th>name</th>
            <th className="num">score</th>
            <th>rating</th>
            <th className="num">close</th>
            <th className="num">5d</th>
            <th className="num">20d</th>
            <th className="num">news</th>
            <th>note</th>
          </tr>
        </thead>
        <tbody>
          {items.map((w) => {
            const on = w.symbol === selected
            const p = w.pick
            const s = p?.scored
            const toggle = () => p && onSelect(on ? null : w.symbol)
            return (
              <tr
                key={w.symbol}
                className={on ? 'sel' : ''}
                onClick={toggle}
                tabIndex={0}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' || e.key === ' ') toggle()
                }}
              >
                <td className="num">{w.rank != null ? `#${w.rank} / ${w.of}` : <span className="dim">–</span>}</td>
                <td>
                  <code>{w.symbol}</code>
                </td>
                <td>{w.name}</td>
                {s ? (
                  <>
                    <td className="num">{s.score.toFixed(2)}</td>
                    <td>{s.rating}</td>
                    <td className="num">{price(s.close)}</td>
                    <td className={`num ${cls(s.change_5d)}`}>{s.change_5d == null ? '–' : signedPct(s.change_5d)}</td>
                    <td className={`num ${cls(s.change_20d)}`}>{s.change_20d == null ? '–' : signedPct(s.change_20d)}</td>
                    <td className="num">{p?.headlines.length ?? 0}</td>
                    <td>{p?.commentary ? <span className="badge">{p.commentary.watch_reason}</span> : <span className="dim">–</span>}</td>
                  </>
                ) : (
                  <td colSpan={7} className="dim">
                    not scored: {w.unscored ?? 'unknown'}
                  </td>
                )}
              </tr>
            )
          })}
        </tbody>
      </table>
    </div>
  )
}
