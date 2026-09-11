import { useEffect, useMemo, useState } from 'react'
import type { BacktestResult } from './bindings/BacktestResult'
import { Chart } from './Chart'

const pct = (x: number) => `${(x * 100).toFixed(2)}%`
const day = (ms: number) => new Date(ms).toISOString().slice(0, 10)

export function App() {
  const [results, setResults] = useState<BacktestResult[]>([])
  const [symbol, setSymbol] = useState('')
  const [selectedTs, setSelectedTs] = useState<number | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    fetch('/api/backtest')
      .then((r) => {
        if (!r.ok) throw new Error(`HTTP ${r.status}`)
        return r.json()
      })
      .then((data: BacktestResult[]) => {
        setResults(data)
        if (data.length) setSymbol(data[0].symbol)
      })
      .catch((e) => setError(String(e)))
  }, [])

  const current = useMemo(() => results.find((r) => r.symbol === symbol) ?? null, [results, symbol])
  const selected = useMemo(() => {
    if (!current || selectedTs == null) return null
    return current.days.find((d) => Number(d.ts) === selectedTs) ?? null
  }, [current, selectedTs])
  const trades = useMemo(() => (current?.days ?? []).filter((d) => d.action !== 'Hold'), [current])

  if (error)
    return (
      <div className="pad">
        Failed to load <code>/api/backtest</code>: {error}. Is the web server running, and is{' '}
        <code>backtest.jsonl</code> present?
      </div>
    )
  if (!results.length)
    return (
      <div className="pad">
        No backtest results yet — run <code>BACKTEST=1 cargo run</code> to produce{' '}
        <code>backtest.jsonl</code>, then start <code>cargo run --bin web</code>.
      </div>
    )

  return (
    <div className="pad">
      <header className="row">
        <h1>Trading Decisions</h1>
        <select
          value={symbol}
          onChange={(e) => {
            setSymbol(e.target.value)
            setSelectedTs(null)
          }}
        >
          {results.map((r) => (
            <option key={r.symbol} value={r.symbol}>
              {r.symbol}
            </option>
          ))}
        </select>
      </header>

      {current && (
        <>
          <div className="cards">
            <Stat label="Total return" value={pct(current.total_return)} good={current.total_return >= 0} />
            <Stat label="Buy & hold" value={pct(current.buy_hold_return)} good={current.buy_hold_return >= 0} />
            <Stat label="Max drawdown" value={pct(current.max_drawdown)} good={false} />
            <Stat label="Trades" value={String(current.num_trades)} />
          </div>

          <p className="hint">
            Blue = price (▲ buy / ▼ sell). Purple dashed = equity (dotted line = starting cash). Click a marker
            or a row to see the signal and the LLM's reasoning.
          </p>

          <Chart days={current.days} startingCash={current.starting_cash} onSelect={setSelectedTs} />

          <div className="grid">
            <div>
              <h3>Decisions ({trades.length})</h3>
              <table>
                <thead>
                  <tr>
                    <th>Date</th>
                    <th>Action</th>
                    <th>Qty</th>
                    <th>Score</th>
                    <th>RSI</th>
                    <th>Close</th>
                  </tr>
                </thead>
                <tbody>
                  {trades.map((d) => {
                    const ms = Number(d.ts)
                    return (
                      <tr key={ms} className={selectedTs === ms ? 'sel' : ''} onClick={() => setSelectedTs(ms)}>
                        <td>{day(ms)}</td>
                        <td className={d.action === 'Buy' ? 'buy' : 'sell'}>{d.action}</td>
                        <td>{d.qty}</td>
                        <td>{d.score.toFixed(3)}</td>
                        <td>{d.rsi.toFixed(1)}</td>
                        <td>{d.close.toFixed(2)}</td>
                      </tr>
                    )
                  })}
                </tbody>
              </table>
            </div>

            <div>
              <h3>Why this decision</h3>
              {selected ? (
                <div className="detail">
                  <div className="row">
                    <span className={`tag ${selected.action.toLowerCase()}`}>{selected.action}</span>
                    <span>{day(Number(selected.ts))}</span>
                  </div>
                  <p>
                    <b>Signal</b> — score {selected.score.toFixed(3)} · RSI {selected.rsi.toFixed(1)} · close{' '}
                    {selected.close.toFixed(2)} · qty {selected.qty}
                  </p>
                  <p>
                    <b>Decision:</b> {selected.decision_reason || '—'}
                  </p>
                  <p>
                    <b>Analyst:</b>
                  </p>
                  <pre>{selected.analyst_text || '—'}</pre>
                </div>
              ) : (
                <p className="hint">Select a point to see the LLM's analysis and decision.</p>
              )}
            </div>
          </div>
        </>
      )}
    </div>
  )
}

function Stat({ label, value, good }: { label: string; value: string; good?: boolean }) {
  return (
    <div className="card">
      <div className="card-label">{label}</div>
      <div className="card-value" style={good == null ? undefined : { color: good ? '#1a7f37' : '#cf222e' }}>
        {value}
      </div>
    </div>
  )
}
