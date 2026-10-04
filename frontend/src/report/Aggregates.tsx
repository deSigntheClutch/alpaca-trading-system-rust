'use client'

import type { Aggregate } from '@/bindings/Aggregate'

const cls = (x: number) => (x > 0 ? 'pos' : x < 0 ? 'neg' : '')

function Table({ title, rows }: { title: string; rows: Aggregate[] }) {
  return (
    <div>
      <div className="dim" style={{ marginBottom: 4 }}>
        {title} · mean score (n)
      </div>
      <table className="table">
        <tbody>
          {rows.slice(0, 10).map((a) => (
            <tr key={a.key}>
              <td>{a.key}</td>
              <td className={`num ${cls(a.mean_score)}`}>
                {a.mean_score >= 0 ? '+' : ''}
                {a.mean_score.toFixed(2)}
              </td>
              <td className="num dim">{a.count}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

/**
 * Sector and theme roll-ups. With ~160 names most themes have one or two members,
 * which is why `n` is always next to the mean: a theme of one is just that stock.
 */
export function Aggregates({ bySector, byTheme }: { bySector: Aggregate[]; byTheme: Aggregate[] }) {
  return (
    <div className="two" style={{ marginTop: 16 }}>
      <Table title="by sector" rows={bySector} />
      <Table title="by theme" rows={byTheme} />
    </div>
  )
}
