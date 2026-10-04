/**
 * The report page's reads. Same-origin `/api/*`: in dev Next rewrites it to the axum
 * bin on :8080, in prod the bin serves both. Every type here is generated from the
 * Rust structs by ts-rs (`frontend/src/bindings/`), so a renamed field fails `tsc`
 * instead of rendering `undefined`.
 */
import type { Bar } from '@/bindings/Bar'
import type { MorningReport } from '@/bindings/MorningReport'
import type { Regime } from '@/bindings/Regime'
import type { Screen } from '@/bindings/Screen'

/** `GET /api/screen` — a replay for a morning that has no stored report. */
export type ScreenReply = { date: string; regime: Regime | null; screen: Screen }

async function json<T>(res: Response): Promise<T> {
  if (!res.ok) {
    const body = await res.text().catch(() => '')
    throw new Error(`HTTP ${res.status}${body ? `: ${body.slice(0, 200)}` : ''}`)
  }
  return res.json() as Promise<T>
}

/** Dates with a stored report, oldest first. */
export async function fetchDates(): Promise<string[]> {
  return json<string[]>(await fetch('/api/reports'))
}

/** `null` when no report was written for that morning (404). */
export async function fetchReport(date: string): Promise<MorningReport | null> {
  const res = await fetch(`/api/reports/${encodeURIComponent(date)}`)
  if (res.status === 404) return null
  return json<MorningReport>(res)
}

export async function fetchLatest(): Promise<MorningReport | null> {
  const res = await fetch('/api/reports/latest')
  if (res.status === 404) return null
  return json<MorningReport>(res)
}

/** Replay the screener as of `date` against the local bar files. */
export async function fetchScreen(date: string, top: number): Promise<ScreenReply> {
  const q = new URLSearchParams({ date, top: String(top) })
  return json<ScreenReply>(await fetch(`/api/screen?${q}`))
}

/** The most recent `limit` daily bars for one symbol, oldest first. */
export async function fetchBars(symbol: string, limit = 250): Promise<Bar[]> {
  const q = new URLSearchParams({ timeframe: '1Day', limit: String(limit) })
  return json<Bar[]>(await fetch(`/api/bars/${encodeURIComponent(symbol)}?${q}`))
}
