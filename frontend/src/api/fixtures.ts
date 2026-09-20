/**
 * Synthetic backtest results, so the page can be built and reviewed before the
 * Rust endpoints exist (BT-7 #50). Delete this file once the real API is wired.
 *
 * Two properties matter here:
 *
 *  - **Deterministic.** The series is seeded from the symbol and the window, so
 *    the same controls always produce the same chart. Clicking 90 and then back
 *    to 30 and then 90 again gives the identical picture, which is what makes it
 *    usable for eyeballing UI changes.
 *  - **Structurally honest.** The indicators mirror `src/domain/signals.rs`
 *    exactly (SMA20 trend, SMA5/SMA20 momentum, RSI14 contrarian, weights
 *    0.4/0.35/0.25), fills happen on the NEXT bar, and fees plus slippage are
 *    charged on both legs. So the shapes the UI has to render — clustered
 *    signals, whipsaws, fee drag, open positions at the end — are the shapes the
 *    real engine will produce, not smooth fake curves.
 *
 * Position sizing here is a flat 25% of equity per entry, purely so the equity
 * curve moves enough to look at. Real sizing is STRAT-1 (#23) and RISK-3 (#55).
 */
import type {
  Action, BacktestResult, DayRecord, Metrics, RunConfig, RunRequest, Trade,
} from './types'

const MS_PER_DAY = 86_400_000

// ─────────────────────────────────────────────────────────── deterministic rng

function hashString(s: string): number {
  let h = 2166136261
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i)
    h = Math.imul(h, 16777619)
  }
  return h >>> 0
}

/** mulberry32: small, fast, good enough, and identical across reloads. */
function rng(seed: number): () => number {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = Math.imul(a ^ (a >>> 15), 1 | a)
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

/** Box-Muller, so returns are normal rather than uniform. */
function gauss(next: () => number): number {
  const u = Math.max(next(), 1e-9)
  const v = next()
  return Math.sqrt(-2 * Math.log(u)) * Math.cos(2 * Math.PI * v)
}

// ───────────────────────────────────────────────── indicators (mirror signals.rs)

function sma(closes: number[], n: number): number | null {
  if (closes.length < n) return null
  let sum = 0
  for (let i = closes.length - n; i < closes.length; i++) sum += closes[i]
  return sum / n
}

function rsi(closes: number[], n: number): number | null {
  if (closes.length < n + 1) return null
  let gain = 0
  let loss = 0
  for (let i = closes.length - n; i < closes.length; i++) {
    const c = closes[i] - closes[i - 1]
    if (c >= 0) gain += c
    else loss -= c
  }
  const avgGain = gain / n
  const avgLoss = loss / n
  if (avgLoss === 0) return 100
  return 100 - 100 / (1 + avgGain / avgLoss)
}

const clamp = (x: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, x))

/** Returns null when there are fewer than 20 closes, exactly like `compute`. */
function signal(closes: number[]): { score: number; rsi: number } | null {
  const last = closes[closes.length - 1]
  const s20 = sma(closes, 20)
  const s5 = sma(closes, 5)
  const r = rsi(closes, 14)
  if (s20 == null || s5 == null || r == null) return null
  const trend = clamp((last - s20) / s20 / 0.08, -1, 1)
  const momentum = clamp((s5 - s20) / s20 / 0.05, -1, 1)
  const rsiScore = clamp((50 - r) / 30, -1, 1)
  return { score: 0.4 * trend + 0.35 * momentum + 0.25 * rsiScore, rsi: r }
}

// ──────────────────────────────────────────────────────────── price + calendar

const isCrypto = (symbol: string) => symbol.includes('/')

/** Crypto trades every day; stocks skip weekends. Holidays are ignored here. */
function calendar(symbol: string, endMs: number, count: number): number[] {
  const out: number[] = []
  let t = endMs
  while (out.length < count) {
    const dow = new Date(t).getUTCDay()
    if (isCrypto(symbol) || (dow !== 0 && dow !== 6)) out.push(t)
    t -= MS_PER_DAY
  }
  return out.reverse()
}

/** Plausible starting level, so BTC is not priced like a small-cap. */
function basePrice(symbol: string): number {
  if (symbol.startsWith('BTC')) return 60_000
  if (symbol.startsWith('ETH')) return 3_000
  return 40 + (hashString(symbol) % 260)
}

function priceSeries(symbol: string, n: number, seed: number): number[] {
  const next = rng(seed)
  // Per-symbol drift and volatility, so different tickers look different but
  // the same ticker always looks the same.
  const drift = (((hashString(symbol + ':drift') % 100) / 100) - 0.45) * 0.0016
  const vol = isCrypto(symbol) ? 0.032 : 0.017
  const out: number[] = []
  let p = basePrice(symbol)
  for (let i = 0; i < n; i++) {
    p *= Math.exp(drift + vol * gauss(next))
    out.push(Math.round(p * 100) / 100)
  }
  return out
}

// ────────────────────────────────────────────────────────────────── the run

const WARMUP = 30 // enough for SMA20 + RSI14 before the first decision day

function analystText(symbol: string, score: number, r: number, close: number): string {
  const tone = score > 0.2 ? 'constructive' : score < -0.2 ? 'cautious' : 'mixed'
  const rsiNote = r > 70 ? 'stretched' : r < 30 ? 'washed out' : 'neutral'
  return (
    `${symbol} closed at ${close.toFixed(2)} with a composite score of ${score.toFixed(3)}, ` +
    `a ${tone} read. RSI at ${r.toFixed(1)} looks ${rsiNote}. ` +
    `Trend and momentum carry most of the weight here, so the setup is only as good as the ` +
    `prevailing direction; treat this as educational, not investment advice.`
  )
}

function simulate(symbol: string, req: RunRequest, days: number, endMs: number): BacktestResult {
  const seed = hashString(`${symbol}|${days}|${req.start ?? ''}|${req.end ?? ''}`)
  const total = days + WARMUP
  const times = calendar(symbol, endMs, total)
  const closes = priceSeries(symbol, total, seed)

  const feeRate = req.fee_bps / 10_000
  const slipRate = req.slippage_bps / 10_000

  let cash = req.starting_cash
  let position = 0
  let entryPrice = 0
  let entryTs = 0
  let entryReason = ''
  let feesOnEntry = 0

  const records: DayRecord[] = []
  const trades: Trade[] = []
  let totalFees = 0
  let daysInMarket = 0

  // Decisions are made on day i and filled on day i+1, so the last bar only
  // marks the book to market. This is the "fill on the next bar" rule from BT-1.
  const pending: { action: Action; reason: string }[] = []

  for (let i = WARMUP; i < total; i++) {
    const window = closes.slice(0, i + 1)
    const sig = signal(window)
    const close = closes[i]
    const ts = times[i]

    // Fill whatever yesterday decided, at today's price plus slippage.
    let filledAction: Action = 'Hold'
    let filledQty = 0
    const order = pending.shift()
    if (order && order.action === 'Buy' && cash > 0) {
      const price = close * (1 + slipRate)
      const notional = Math.min(cash, req.starting_cash * 0.25)
      const qty = notional / price
      if (qty > 0) {
        const fee = notional * feeRate
        cash -= notional + fee
        totalFees += fee
        if (position === 0) {
          entryPrice = price
          entryTs = ts
          entryReason = order.reason
          feesOnEntry = fee
        } else {
          // Averaging up: blend the cost basis rather than opening a second lot.
          entryPrice = (entryPrice * position + price * qty) / (position + qty)
          feesOnEntry += fee
        }
        position += qty
        filledAction = 'Buy'
        filledQty = qty
      }
    } else if (order && order.action === 'Sell' && position > 0) {
      const price = close * (1 - slipRate)
      const proceeds = position * price
      const fee = proceeds * feeRate
      cash += proceeds - fee
      totalFees += fee
      const cost = entryPrice * position
      const pnl = proceeds - cost - fee - feesOnEntry
      trades.push({
        symbol,
        entry_ts: entryTs,
        entry_price: entryPrice,
        exit_ts: ts,
        exit_price: price,
        qty: position,
        pnl,
        pnl_pct: cost > 0 ? pnl / cost : 0,
        fees: fee + feesOnEntry,
        entry_reason: entryReason,
        exit_reason: order.reason,
      })
      filledAction = 'Sell'
      filledQty = position
      position = 0
      entryPrice = 0
      feesOnEntry = 0
    }

    // Today's decision, filled tomorrow.
    let reason = 'insufficient bars for signal'
    if (sig) {
      if (sig.score >= 0.2 && cash > req.starting_cash * 0.02) {
        reason = `score ${sig.score.toFixed(3)} above the buy threshold with RSI ${sig.rsi.toFixed(1)}`
        pending.push({ action: 'Buy', reason })
      } else if (sig.score <= -0.2 && position > 0) {
        reason = `score ${sig.score.toFixed(3)} below the sell threshold with RSI ${sig.rsi.toFixed(1)}`
        pending.push({ action: 'Sell', reason })
      } else {
        reason = `score ${sig.score.toFixed(3)} inside the hold band`
      }
    }

    if (position > 0) daysInMarket++
    const equity = cash + position * close

    records.push({
      ts,
      close,
      score: sig?.score ?? 0,
      rsi: sig?.rsi ?? 0,
      action: filledAction,
      qty: filledQty,
      equity,
      decision_reason: reason,
      analyst_text: sig ? analystText(symbol, sig.score, sig.rsi, close) : '',
    })
  }

  // An open position at the end is a real outcome, not an error: report it with
  // a null exit so the UI can show it as still running.
  if (position > 0) {
    const last = records[records.length - 1]
    const cost = entryPrice * position
    const mark = last.close * position
    trades.push({
      symbol,
      entry_ts: entryTs,
      entry_price: entryPrice,
      exit_ts: null,
      exit_price: null,
      qty: position,
      pnl: mark - cost - feesOnEntry,
      pnl_pct: cost > 0 ? (mark - cost - feesOnEntry) / cost : 0,
      fees: feesOnEntry,
      entry_reason: entryReason,
      exit_reason: '',
    })
  }

  return {
    run_id: '',
    config: {} as RunConfig, // filled in by the caller, which knows the whole request
    symbol,
    starting_cash: req.starting_cash,
    days: records,
    trades,
    metrics: metrics(records, trades, req.starting_cash, totalFees, daysInMarket),
  }
}

function metrics(
  days: DayRecord[], trades: Trade[], startingCash: number, totalFees: number, daysInMarket: number,
): Metrics {
  const equity = days.map((d) => d.equity)
  const final = equity.length ? equity[equity.length - 1] : startingCash

  let peak = -Infinity
  let maxDd = 0
  for (const e of equity) {
    if (e > peak) peak = e
    if (peak > 0) maxDd = Math.max(maxDd, (peak - e) / peak)
  }

  const rets: number[] = []
  for (let i = 1; i < equity.length; i++) {
    if (equity[i - 1] > 0) rets.push(equity[i] / equity[i - 1] - 1)
  }
  const mean = rets.length ? rets.reduce((a, b) => a + b, 0) / rets.length : 0
  const variance = rets.length > 1
    ? rets.reduce((a, b) => a + (b - mean) ** 2, 0) / (rets.length - 1)
    : 0
  const sd = Math.sqrt(variance)
  const sharpe = sd > 0 ? (mean / sd) * Math.sqrt(252) : 0

  const closed = trades.filter((t) => t.exit_ts != null)
  const wins = closed.filter((t) => t.pnl > 0)
  const losses = closed.filter((t) => t.pnl <= 0)
  const sumWin = wins.reduce((a, t) => a + t.pnl, 0)
  const sumLoss = Math.abs(losses.reduce((a, t) => a + t.pnl, 0))

  const first = days.length ? days[0].close : 0
  const last = days.length ? days[days.length - 1].close : 0

  return {
    total_return: startingCash > 0 ? (final - startingCash) / startingCash : 0,
    max_drawdown: maxDd === -Infinity ? 0 : maxDd,
    sharpe,
    num_trades: closed.length,
    win_rate: closed.length ? wins.length / closed.length : 0,
    avg_win: wins.length ? sumWin / wins.length : 0,
    avg_loss: losses.length ? -sumLoss / losses.length : 0,
    profit_factor: sumLoss > 0 ? sumWin / sumLoss : sumWin > 0 ? Infinity : 0,
    exposure: days.length ? daysInMarket / days.length : 0,
    buy_hold_return: first > 0 ? (last - first) / first : 0,
    total_fees: totalFees,
  }
}

const iso = (ms: number) => new Date(ms).toISOString().slice(0, 10)

/** Produce one result per traded symbol for the given request. */
export function fixtureRun(req: RunRequest): BacktestResult[] {
  // Snap to UTC midnight. Using `Date.now()` directly would shift the calendar
  // by milliseconds between two otherwise identical calls, and the whole point
  // of this module is that the same controls redraw the same chart.
  const endMs = req.end
    ? Date.parse(`${req.end}T00:00:00Z`)
    : Math.floor(Date.now() / MS_PER_DAY) * MS_PER_DAY
  const days = req.days ?? Math.max(
    20,
    Math.round((endMs - Date.parse(`${req.start ?? iso(endMs - 90 * MS_PER_DAY)}T00:00:00Z`)) / MS_PER_DAY),
  )

  const runId = `fixture_${hashString(JSON.stringify(req)).toString(16)}`
  const results = req.symbols.map((s) => simulate(s, req, days, endMs))
  const windowStart = results[0]?.days[0]?.ts ?? endMs
  const config: RunConfig = {
    arm: req.arm,
    model: req.arm === 'rule' ? '' : 'claude-sonnet-5',
    adapter: req.arm === 'rule' ? '' : 'anthropic',
    fee_bps: req.fee_bps,
    slippage_bps: req.slippage_bps,
    starting_cash: req.starting_cash,
    window_start: iso(windowStart),
    window_end: iso(endMs),
    symbols: req.symbols,
    context_symbols: req.context_symbols,
    git_sha: 'fixture',
    error_days: 0,
  }
  return results.map((r) => ({ ...r, run_id: runId, config }))
}
