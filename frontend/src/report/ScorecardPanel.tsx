'use client'

import type { Scorecard } from '@/bindings/Scorecard'
import { signedPct } from '@/format'

const cls = (x: number) => (x > 0 ? 'pos' : x < 0 ? 'neg' : '')

/**
 * Did the rule work? One row per horizon; the per-report series folds out below it
 * so a bad month and a bad rule look different.
 */
export function ScorecardPanel({ scorecard }: { scorecard: Scorecard }) {
  return (
    <div className="drawer">
      <div className="row head">
        <strong>Scorecard</strong>
        <span className="dim">vs {scorecard.benchmark} · as of {scorecard.as_of}</span>
      </div>
      <div className="scroll">
        <table className="table">
          <thead>
            <tr>
              <th>horizon</th>
              <th className="num">reports</th>
              <th className="num">picks</th>
              <th className="num">mean return</th>
              <th className="num">{scorecard.benchmark}</th>
              <th className="num">excess</th>
              <th className="num">hit rate</th>
            </tr>
          </thead>
          <tbody>
            {scorecard.horizons.map((h) => (
              <tr key={h.horizon_days}>
                <td>{h.horizon_days} sessions</td>
                <td className="num">{h.reports}</td>
                <td className="num">{h.picks}</td>
                <td className={`num ${cls(h.mean_return)}`}>{signedPct(h.mean_return)}</td>
                <td className={`num ${cls(h.mean_benchmark_return)}`}>{signedPct(h.mean_benchmark_return)}</td>
                <td className={`num ${cls(h.mean_excess)}`}>
                  <strong>{signedPct(h.mean_excess)}</strong>
                </td>
                <td className="num">{(h.hit_rate * 100).toFixed(0)}%</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p className="hint">{scorecard.note}</p>
      {scorecard.horizons.map((h) => (
        <details key={h.horizon_days}>
          <summary className="dim">
            {h.horizon_days}-session outcome per report ({h.by_report.length})
          </summary>
          <div className="scroll">
            <table className="table">
              <thead>
                <tr>
                  <th>report</th>
                  <th className="num">picks</th>
                  <th className="num">mean return</th>
                  <th className="num">excess</th>
                  <th className="num">hit</th>
                  <th>best / worst</th>
                </tr>
              </thead>
              <tbody>
                {[...h.by_report].reverse().map((r) => {
                  const sorted = [...r.picks].sort((a, b) => b.excess - a.excess)
                  const best = sorted[0]
                  const worst = sorted[sorted.length - 1]
                  return (
                    <tr key={r.date}>
                      <td>{r.date}</td>
                      <td className="num">{r.picks.length}</td>
                      <td className={`num ${cls(r.mean_return)}`}>{signedPct(r.mean_return)}</td>
                      <td className={`num ${cls(r.mean_excess)}`}>{signedPct(r.mean_excess)}</td>
                      <td className="num">{(r.hit_rate * 100).toFixed(0)}%</td>
                      <td className="dim">
                        {best && (
                          <>
                            <code>{best.symbol}</code> {signedPct(best.excess)}
                          </>
                        )}
                        {worst && worst !== best && (
                          <>
                            {' / '}
                            <code>{worst.symbol}</code> {signedPct(worst.excess)}
                          </>
                        )}
                      </td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          </div>
        </details>
      ))}
    </div>
  )
}
