'use client'

/**
 * The backtest page.
 *
 * FE-1 (#51) is the run controls, the price chart with buy and sell markers, and
 * the equity curve. FE-2 (#56) is the metric cards, the trade table and the
 * decision drill-down below it.
 *
 * Chart and table share one `selectedTs`, so clicking a marker highlights the
 * trade and clicking a trade moves the drill-down, without either view owning
 * the selection.
 */
import { useCallback, useEffect, useMemo, useState } from 'react'
import dynamic from 'next/dynamic'
import { DecisionDetail } from './DecisionDetail'
import { MetricCards } from './MetricCards'
import { RunControls } from './RunControls'
import { TradeTable } from './TradeTable'
import { runBacktest, usingFixtures } from './api/client'
import type { BacktestResult, RunRequest, RunStatus } from './api/types'

// lightweight-charts draws on a <canvas> and only runs in a browser. `ssr: false`
// keeps it out of the build-time pre-render, so the static export never tries
// to run it in Node. Everything else on the page is plain React and pre-renders fine.
const Chart = dynamic(() => import('./Chart').then((m) => m.Chart), { ssr: false })

const DEFAULT_REQUEST: RunRequest = {
  symbols: ['NVDA'],
  context_symbols: ['MU', 'SPY'],
  days: 90,
  starting_cash: 10_000,
  arm: 'rule',
  fee_bps: 25,
  slippage_bps: 5,
}

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
  const lastTs = current?.days.length ? current.days[current.days.length - 1].ts : 0
  const selectedDay = useMemo(
    () => (current && selectedTs != null ? current.days.find((d) => d.ts === selectedTs) ?? null : null),
    [current, selectedTs],
  )

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
        <div className="error warn-box">
          <strong>These prices are invented, not market data.</strong> The browser makes no network
          request at all: the series is a random walk seeded from the letters of the ticker, indexed to
          100 at the start of the window. A real symbol will not show its real price, and the returns
          here mean nothing. This build was made with <code>NEXT_PUBLIC_USE_FIXTURES=true</code>, for
          working on the page without a backend; a normal build calls the run API.
        </div>
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

      {error && <div className="error">Backtest failed: {error}</div>}

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
          <div className="row runline">
            <strong>{current.symbol}</strong>
            <span className="dim">
              {current.config.window_start} to {current.config.window_end}
            </span>
            <span className="dim">{current.config.arm} arm</span>
            <span className="dim">
              {current.config.fee_bps} bps fee · {current.config.slippage_bps} bps slippage
            </span>
            {current.config.context_symbols.length > 0 && (
              <span className="dim">context: {current.config.context_symbols.join(', ')}</span>
            )}
          </div>

          <MetricCards metrics={current.metrics} />

          <p className="hint">
            Blue is price, with ▲ for a buy and ▼ for a sell. Purple dashed is equity on its own scale,
            and the dotted line is starting cash. Click a marker to read the reasoning behind it.
          </p>

          <Chart
            days={current.days}
            startingCash={current.starting_cash}
            onSelect={setSelectedTs}
            selectedTs={selectedTs}
            watermark={usingFixtures ? 'SIMULATED — NOT MARKET DATA' : undefined}
          />

          <div className="grid">
            <TradeTable
              trades={current.trades}
              selectedTs={selectedTs}
              onSelect={setSelectedTs}
              lastTs={lastTs}
            />
            <DecisionDetail day={selectedDay} trades={current.trades} lastTs={lastTs} />
          </div>
        </>
      )}

      {!current && !running && !error && (
        <p className="hint">No results. Add a symbol and press Run.</p>
      )}
    </div>
  )
}
