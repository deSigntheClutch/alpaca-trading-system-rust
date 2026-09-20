/**
 * FE-1 (#51): run controls, the price chart with buy and sell markers, and the
 * equity curve.
 *
 * The per-trade table, the decision drill-down and the full metric cards are
 * FE-2 (#56), so the area below the chart is deliberately thin for now. The
 * selection state and the `onSelect` wiring already exist here because FE-2
 * binds the table and the chart to the same record.
 */
import { useCallback, useEffect, useState } from 'react'
import { Chart } from './Chart'
import { RunControls } from './RunControls'
import { runBacktest, usingFixtures } from './api/client'
import type { BacktestResult, RunRequest, RunStatus } from './api/types'

const DEFAULT_REQUEST: RunRequest = {
  symbols: ['NVDA'],
  context_symbols: ['MU', 'SPY'],
  days: 90,
  starting_cash: 10_000,
  arm: 'rule',
  fee_bps: 25,
  slippage_bps: 5,
}

const pct = (x: number) => `${(x * 100).toFixed(2)}%`

export function App() {
  const [request, setRequest] = useState<RunRequest>(DEFAULT_REQUEST)
  const [results, setResults] = useState<BacktestResult[]>([])
  const [symbol, setSymbol] = useState('')
  const [selectedTs, setSelectedTs] = useState<number | null>(null)
  const [status, setStatus] = useState<RunStatus | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [running, setRunning] = useState(false)

  const run = useCallback(async (req: RunRequest) => {
    setRunning(true)
    setError(null)
    setSelectedTs(null)
    try {
      const out = await runBacktest(req, setStatus)
      setResults(out)
      setSymbol((prev) => (out.some((r) => r.symbol === prev) ? prev : out[0]?.symbol ?? ''))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setResults([])
    } finally {
      setRunning(false)
      setStatus(null)
    }
  }, [])

  // One run on load, so the page is never empty.
  useEffect(() => {
    void run(DEFAULT_REQUEST)
  }, [run])

  const current = results.find((r) => r.symbol === symbol) ?? null

  return (
    <div className="pad">
      <header className="row head">
        <h1>Backtest</h1>
        {usingFixtures && (
          <span className="badge" title="No backend is being called. Numbers here are synthetic.">
            synthetic data
          </span>
        )}
      </header>

      {usingFixtures && (
        <p className="hint">
          These results are generated in the browser, not by the trading engine. They exist so the page
          can be built and reviewed before the run API (#50) is written. Run with{' '}
          <code>VITE_USE_FIXTURES=false</code> once that endpoint exists.
        </p>
      )}

      <RunControls
        value={request}
        onChange={setRequest}
        onRun={() => void run(request)}
        running={running}
      />

      {status && (
        <p className="hint">
          {status.message}
          {status.progress != null ? ` — ${Math.round(status.progress * 100)}%` : ''}
        </p>
      )}

      {error && (
        <div className="error">
          Backtest failed: {error}
        </div>
      )}

      {results.length > 1 && (
        <div className="seg tabs">
          {results.map((r) => (
            <button
              key={r.symbol}
              type="button"
              className={r.symbol === symbol ? 'on' : ''}
              onClick={() => {
                setSymbol(r.symbol)
                setSelectedTs(null)
              }}
            >
              {r.symbol}
            </button>
          ))}
        </div>
      )}

      {current && (
        <>
          <div className="row summary">
            <strong>{current.symbol}</strong>
            <span className={current.metrics.total_return >= 0 ? 'buy' : 'sell'}>
              {pct(current.metrics.total_return)} net
            </span>
            <span className="dim">
              buy and hold {pct(current.metrics.buy_hold_return)}
            </span>
            <span className="dim">{current.metrics.num_trades} closed trades</span>
            <span className="dim">exposure {pct(current.metrics.exposure)}</span>
            {current.config.context_symbols.length > 0 && (
              <span className="dim">context: {current.config.context_symbols.join(', ')}</span>
            )}
          </div>

          <p className="hint">
            Blue is price, with ▲ for a buy and ▼ for a sell. Purple dashed is equity on its own scale,
            and the dotted line is starting cash.
          </p>

          <Chart
            days={current.days}
            startingCash={current.starting_cash}
            onSelect={setSelectedTs}
            selectedTs={selectedTs}
          />

          <div className="placeholder">
            FE-2 (#56) goes here: the per-trade table with net P&amp;L, the decision drill-down showing
            the signal and the LLM's own words, and the full metric cards.
            {selectedTs != null && (
              <> Selected day: <code>{new Date(selectedTs).toISOString().slice(0, 10)}</code>.</>
            )}
          </div>
        </>
      )}

      {!current && !running && !error && (
        <p className="hint">No results. Add a symbol and press Run.</p>
      )}
    </div>
  )
}
