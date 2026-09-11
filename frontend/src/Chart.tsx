import { useEffect, useRef } from 'react'
import {
  createChart,
  ColorType,
  LineStyle,
  type SeriesMarker,
  type Time,
  type UTCTimestamp,
} from 'lightweight-charts'
import type { DayRecord } from './bindings/DayRecord'

type Props = {
  days: DayRecord[]
  startingCash: number
  /** called with the clicked day's ts in EPOCH MILLIS (the DayRecord key) */
  onSelect: (tsMillis: number) => void
}

// ts-rs types ts as bigint (Rust i64) but serde sends a JSON number, so at runtime
// it's a JS number; Number() handles both. lightweight-charts wants UTC seconds.
const toSecs = (ts: number | bigint) => (Number(ts) / 1000) as UTCTimestamp

export function Chart({ days, startingCash, onSelect }: Props) {
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
    })

    // lightweight-charts requires strictly ascending, unique times.
    const sorted = [...days].sort((a, b) => Number(a.ts) - Number(b.ts))

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

    // Buy ▲ below bar (green), Sell ▼ above bar (red); Hold → no marker.
    const markers: SeriesMarker<Time>[] = sorted
      .filter((d) => d.action === 'Buy' || d.action === 'Sell')
      .map((d): SeriesMarker<Time> =>
        d.action === 'Buy'
          ? { time: toSecs(d.ts), position: 'belowBar', color: '#26a69a', shape: 'arrowUp', text: `B ${d.qty}` }
          : { time: toSecs(d.ts), position: 'aboveBar', color: '#ef5350', shape: 'arrowDown', text: `S ${d.qty}` },
      )
    price.setMarkers(markers)
    chart.timeScale().fitContent()

    // Click anywhere → snap to the nearest data point → report its ms ts, so a marker
    // click and a table-row click select the exact same DayRecord.
    const secsToMs = new Map<number, number>()
    sorted.forEach((d) => secsToMs.set(Number(toSecs(d.ts)), Number(d.ts)))
    const times = [...secsToMs.keys()]
    chart.subscribeClick((param) => {
      if (param.time == null || times.length === 0) return
      const t = Number(param.time)
      let best = times[0]
      for (const s of times) if (Math.abs(s - t) < Math.abs(best - t)) best = s
      const ms = secsToMs.get(best)
      if (ms != null) onSelectRef.current(ms)
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
  }, [days, startingCash])

  return <div ref={containerRef} style={{ width: '100%' }} />
}
