'use client'

/**
 * One line: the daily close for one name, with a marker on the bar the report saw
 * last. Single series, so no legend; the title names it. The crosshair and tooltip
 * come with lightweight-charts. Bars come from `/api/bars`, i.e. the local store.
 */
import { useEffect, useRef, useState } from 'react'
import { createChart, type IChartApi, type UTCTimestamp } from 'lightweight-charts'

import type { Bar } from '@/bindings/Bar'
import { fetchBars } from './api'

const toSecs = (ms: number) => (ms / 1000) as UTCTimestamp

export function PriceChart({ symbol, asOf }: { symbol: string; asOf: number }) {
  const el = useRef<HTMLDivElement>(null)
  const [bars, setBars] = useState<Bar[] | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let live = true
    setBars(null)
    setError(null)
    fetchBars(symbol, 250)
      .then((b) => live && setBars(b))
      .catch((e) => live && setError(e instanceof Error ? e.message : String(e)))
    return () => {
      live = false
    }
  }, [symbol])

  useEffect(() => {
    if (!el.current || !bars || bars.length === 0) return
    const node = el.current
    const chart: IChartApi = createChart(node, {
      height: 280,
      layout: { background: { color: 'transparent' }, textColor: '#666', fontSize: 11 },
      grid: { vertLines: { color: '#f0f0f0' }, horzLines: { color: '#f0f0f0' } },
      rightPriceScale: { borderVisible: false },
      timeScale: { borderVisible: false, timeVisible: false },
      crosshair: { mode: 0 },
    })
    const line = chart.addLineSeries({ color: '#2962FF', lineWidth: 2, priceLineVisible: false })
    line.setData(bars.map((b) => ({ time: toSecs(b.ts), value: b.close })))
    const visible = bars.filter((b) => b.ts <= asOf)
    const seen = visible[visible.length - 1]
    if (seen) {
      line.setMarkers([
        { time: toSecs(seen.ts), position: 'belowBar', color: '#8a6100', shape: 'arrowUp', text: 'report' },
      ])
    }
    chart.timeScale().fitContent()
    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width
      if (w) chart.applyOptions({ width: Math.floor(w) })
    })
    ro.observe(node)
    return () => {
      ro.disconnect()
      chart.remove()
    }
  }, [bars, asOf])

  return (
    <div>
      <div className="dim" style={{ marginBottom: 4 }}>
        {symbol} · daily close, last {bars?.length ?? 250} sessions
      </div>
      {error && <div className="error">bars: {error}</div>}
      {bars && bars.length === 0 && <div className="hint">No bars in the local store for {symbol}.</div>}
      <div ref={el} className="chart-wrap" />
    </div>
  )
}
