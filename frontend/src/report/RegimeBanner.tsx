'use client'

import type { Regime } from '@/bindings/Regime'

/** One line: is the index above its long average? The context every pick sits in. */
export function RegimeBanner({ regime }: { regime: Regime | null }) {
  if (!regime) {
    return <div className="warn-box">Regime unavailable: fewer than 200 daily bars for the index.</div>
  }
  const cls = regime.above ? 'buy' : 'sell'
  return (
    <div className="row runline">
      <span className={`badge ${cls}`}>
        {regime.symbol} {regime.above ? 'ABOVE' : 'BELOW'} {regime.window}d SMA
      </span>
      <span>
        close <strong>{regime.close.toFixed(2)}</strong>
      </span>
      <span className="dim">sma {regime.sma.toFixed(2)}</span>
      <span className={regime.distance_pct >= 0 ? 'pos' : 'neg'}>
        {regime.distance_pct >= 0 ? '+' : ''}
        {regime.distance_pct.toFixed(1)}%
      </span>
      <span className="dim">as of {new Date(regime.as_of).toISOString().slice(0, 10)}</span>
    </div>
  )
}
