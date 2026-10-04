'use client'

import type { Pick } from '@/bindings/Pick'
import { date, price, signedPct } from '@/format'
import { PriceChart } from './PriceChart'

/** Everything the report knows about one pick: numbers, note, headlines, price. */
export function PickDetail({ pick, asOf, onClose }: { pick: Pick; asOf: number; onClose: () => void }) {
  const s = pick.scored
  return (
    <div className="drawer">
      <div className="row detail-head">
        <strong>
          <code>{s.symbol}</code> {s.name}
        </strong>
        <span className="chip">{s.sector}</span>
        {s.themes.map((t) => (
          <span key={t} className="tag">
            {t}
          </span>
        ))}
        <span className="dim">{s.region}</span>
        <button className="seg-btn right" onClick={onClose}>
          close
        </button>
      </div>

      <div className="two">
        <div>
          <div className="kv">
            <span className="dim">as of</span>
            <span>{date(s.as_of)}</span>
            <span className="dim">score</span>
            <span>
              {s.score.toFixed(3)} · {s.rating}
            </span>
            <span className="dim">RSI(14)</span>
            <span>{s.rsi.toFixed(1)}</span>
            <span className="dim">close</span>
            <span>{price(s.close)}</span>
            <span className="dim">1d / 5d / 20d</span>
            <span>
              {[s.change_1d, s.change_5d, s.change_20d].map((c) => (c == null ? '–' : signedPct(c))).join(' / ')}
            </span>
            <span className="dim">bars</span>
            <span>{s.bars}</span>
            {pick.earnings && (
              <>
                <span className="dim">earnings</span>
                <span>
                  {pick.earnings.date}
                  {pick.earnings.earnings_hour ? ` · ${pick.earnings.earnings_hour}` : ''}
                  {pick.earnings.eps_estimate != null ? ` · EPS est ${pick.earnings.eps_estimate.toFixed(2)}` : ''}
                </span>
              </>
            )}
          </div>

          {pick.commentary ? (
            <div className="note">
              <div className="row">
                <span className="badge">{pick.commentary.watch_reason}</span>
                <span className="dim">{pick.commentary.model}</span>
              </div>
              <p>{pick.commentary.summary}</p>
              <p>
                <span className="dim">risk · </span>
                {pick.commentary.risk}
              </p>
            </div>
          ) : (
            <p className="hint">No note for this pick.</p>
          )}

          <div>
            <div className="dim" style={{ marginBottom: 4 }}>
              headlines, last 3 days ({pick.headlines.length})
            </div>
            {pick.headlines.length === 0 && <div className="dim">none</div>}
            <ul className="headlines">
              {pick.headlines.map((h) => (
                <li key={h.id}>
                  {h.url ? (
                    <a href={h.url} target="_blank" rel="noopener noreferrer">
                      {h.headline}
                    </a>
                  ) : (
                    h.headline
                  )}
                  <span className="dim">
                    {' '}
                    · {h.source} · {date(h.ts)}
                    {h.symbols.length > 3 ? ` · ${h.symbols.length} names` : ''}
                  </span>
                </li>
              ))}
            </ul>
          </div>
        </div>

        <PriceChart symbol={s.symbol} asOf={asOf} />
      </div>
    </div>
  )
}
