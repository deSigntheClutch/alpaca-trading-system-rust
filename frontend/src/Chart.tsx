import { useEffect, useRef } from 'react'
import {
  createChart,
  ColorType,
  LineStyle,
  type SeriesMarker,
  type Time,
  type UTCTimestamp,
} from 'lightweight-charts'
import type { DayRecord } from './api/types'

type Props = {
  days: DayRecord[]
  startingCash: number
  /**
   * Called with the clicked day's ts in epoch millis, which is the `DayRecord`
   * key. Optional: the chart stands alone in FE-1, and FE-2 (#56) uses this to
   * keep the chart and the trade table selecting the same record.
   */
  onSelect?: (tsMillis: number) => void
  /** Highlighted day, so an external selection draws a marker here too. */
  selectedTs?: number | null
}

// lightweight-charts wants UTC seconds; the wire carries epoch millis.
const toSecs = (ts: number) => (ts / 1000) as UTCTimestamp

export function Chart({ days, startingCash, onSelect, selectedTs }: Props) {
  const containerRef = useRef<HTMLDivElement>(null)
  const onSelectRef = useRef(onSelect)
  onSelectRef.current = onSelect

  useEffect(() => {
    const el = containerRef.current
    if (!el) return

    const chart = createChart(el, {
      height: 420,
      layout: { background: { type: ColorType.Solid, color: '#fff' }, textColor: '#333' },
      grid: { horzLines: { color: '#f0f0f0' }, vertLines: { color: '#f0f0f0' } },
      rightPriceScale: { borderColor: '#ccc' },
      leftPriceScale: { visible: true, borderColor: '#ccc' },
      timeScale: { borderColor: '#ccc' },
      crosshair: { mode: 1 },
    })

    // lightweight-charts requires strictly ascending, unique times.
    const sorted = [...days].sort((a, b) => a.ts - b.ts)

    const price = chart.addLineSeries({ color: '#2962FF', lineWidth: 2, priceScaleId: 'right' })
    price.setData(sorted.map((d) => ({ time: toSecs(d.ts), value: d.close })))

    // Equity on its own (left) scale so its magnitude doesn't flatten the price line.
    const equity = chart.addLineSeries({
      color: '#9c27b0',
      lineWidth: 1,
      lineStyle: LineStyle.Dashed,
      priceScaleId: 'left',
    })
    equity.setData(sorted.map((d) => ({ time: toSecs(d.ts), value: d.equity })))
    equity.createPriceLine({
      price: startingCash,
      color: '#bbb',
      lineStyle: LineStyle.Dotted,
      lineWidth: 1,
      axisLabelVisible: true,
      title: 'start',
    })

    // Buy ▲ below bar (green), Sell ▼ above bar (red); Hold gets no marker.
    const markers: SeriesMarker<Time>[] = sorted
      .filter((d) => d.action === 'Buy' || d.action === 'Sell')
      .map((d): SeriesMarker<Time> => {
        const on = selectedTs === d.ts
        return d.action === 'Buy'
          ? {
              time: toSecs(d.ts),
              position: 'belowBar',
              color: on ? '#0b6b5f' : '#26a69a',
              shape: 'arrowUp',
              text: `B ${fmtQty(d.qty)}`,
            }
          : {
              time: toSecs(d.ts),
              position: 'aboveBar',
              color: on ? '#a52a26' : '#ef5350',
              shape: 'arrowDown',
              text: `S ${fmtQty(d.qty)}`,
            }
      })
    price.setMarkers(markers)
    chart.timeScale().fitContent()

    // Click anywhere, snap to the nearest data point, report its millis ts, so a
    // marker click and a table row click select the exact same record.
    const secsToMs = new Map<number, number>()
    sorted.forEach((d) => secsToMs.set(Number(toSecs(d.ts)), d.ts))
    const times = [...secsToMs.keys()]
    chart.subscribeClick((param) => {
      if (param.time == null || times.length === 0) return
      const t = Number(param.time)
      let best = times[0]
      for (const s of times) if (Math.abs(s - t) < Math.abs(best - t)) best = s
      const ms = secsToMs.get(best)
      if (ms != null) onSelectRef.current?.(ms)
    })

    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width
      if (w) chart.applyOptions({ width: Math.floor(w) })
    })
    ro.observe(el)

    return () => {
      ro.disconnect()
      chart.remove()
    }
  }, [days, startingCash, selectedTs])

  return <div ref={containerRef} style={{ width: '100%' }} />
}

/** Crypto quantities are fractional; stock quantities usually are not. */
function fmtQty(q: number): string {
  if (q === 0) return '0'
  return q >= 1 ? q.toFixed(2) : q.toPrecision(3)
}
