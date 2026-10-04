'use client'

/**
 * The morning report page (#69). One route, one piece of state: which morning is on
 * screen. A stored report (`/api/reports/{date}`) shows everything — picks with
 * headlines and notes, earnings, the scorecard. A date with no stored report falls
 * back to a replay (`/api/screen?date=`), which has picks and regime only and says so.
 */
import { useCallback, useEffect, useState } from 'react'

import type { MorningReport } from '@/bindings/MorningReport'
import type { Pick } from '@/bindings/Pick'
import type { Scored } from '@/bindings/Scored'
import { fetchDates, fetchLatest, fetchReport, fetchScreen, type ScreenReply } from './api'
import { Aggregates } from './Aggregates'
import { PickDetail } from './PickDetail'
import { PicksTable } from './PicksTable'
import { RegimeBanner } from './RegimeBanner'
import { ScorecardPanel } from './ScorecardPanel'
import { WatchlistTable } from './WatchlistTable'

type View =
  | { kind: 'report'; report: MorningReport }
  | { kind: 'replay'; reply: ScreenReply }

/**
 * Section header. The numbers match the page guide (1 date … 7 run) so the two can
 * be read side by side; the Chinese subtitle says what the block is for.
 */
function Section({ n, title, sub }: { n: number; title: string; sub: string }) {
  return (
    <div className="sec">
      <span className="sec-n">{n}</span>
      <h2>{title}</h2>
      <span className="dim">{sub}</span>
    </div>
  )
}

/** A replayed `Scored` wearing the `Pick` shape, so one table renders both. */
function asPick(scored: Scored): Pick {
  return { scored, earnings: null, headlines: [], commentary: null }
}

export function ReportApp() {
  const [dates, setDates] = useState<string[]>([])
  const [date, setDate] = useState('')
  const [view, setView] = useState<View | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [selected, setSelected] = useState<string | null>(null)

  const load = useCallback(async (wanted: string | null) => {
    setBusy(true)
    setError(null)
    try {
      if (!wanted) {
        const latest = await fetchLatest()
        if (latest) {
          setView({ kind: 'report', report: latest })
          setDate(latest.date)
        } else {
          setView(null)
          setError('No report has been written yet. Run `cargo run --bin morning` first.')
        }
      } else {
        const report = await fetchReport(wanted)
        if (report) {
          setView({ kind: 'report', report })
        } else {
          setView({ kind: 'replay', reply: await fetchScreen(wanted, 8) })
        }
      }
      setSelected(null)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }, [])

  useEffect(() => {
    fetchDates().then(setDates).catch(() => setDates([]))
    void load(null)
  }, [load])

  const picks: Pick[] =
    view?.kind === 'report' ? view.report.picks : view?.kind === 'replay' ? view.reply.screen.picks.map(asPick) : []
  const regime = view?.kind === 'report' ? view.report.regime : view?.kind === 'replay' ? view.reply.regime : null
  const screenLike = view?.kind === 'report' ? view.report : view?.kind === 'replay' ? view.reply.screen : null
  const watchlist = view?.kind === 'report' ? view.report.watchlist : []
  const current =
    picks.find((p) => p.scored.symbol === selected) ??
    watchlist.find((w) => w.symbol === selected)?.pick ??
    null

  return (
    <div className="pad">
      <header className="row">
        <h1>Morning report</h1>
        <span className="dim">rule picks the names · scorecard says whether it worked</span>
      </header>

      <Section n={1} title="Morning" sub="选哪一天的报告 · 没有存报告的日子会当场回放" />
      <div className="panel">
        <div className="panel-row">
          <span className="panel-label">Morning</span>
          <input
            className="numfield"
            type="date"
            value={date}
            onChange={(e) => setDate(e.target.value)}
            aria-label="Report date"
          />
          <button className="seg-btn" disabled={busy || !date} onClick={() => void load(date)}>
            {busy ? 'loading…' : 'Load'}
          </button>
          <button className="seg-btn" disabled={busy} onClick={() => void load(null)}>
            Latest
          </button>
          {dates.length > 0 && (
            <select
              className="numfield"
              value={dates.includes(date) ? date : ''}
              onChange={(e) => {
                setDate(e.target.value)
                void load(e.target.value)
              }}
              aria-label="Stored reports"
            >
              <option value="">stored reports ({dates.length})</option>
              {[...dates].reverse().map((d) => (
                <option key={d} value={d}>
                  {d}
                </option>
              ))}
            </select>
          )}
          <span className="hint" style={{ margin: 0 }}>
            A date with no stored report replays the screener from the bar files as of that morning.
          </span>
        </div>
      </div>

      {error && <div className="error">{error}</div>}

      {view && (
        <>
          <div className="row runline">
            <strong>{view.kind === 'report' ? view.report.date : view.reply.date}</strong>
            {view.kind === 'replay' && (
              <span className="badge">replay · picks and regime only · today&apos;s universe and prices</span>
            )}
            {view.kind === 'report' && view.report.run.warnings.some((w) => w.startsWith('replay')) && (
              <span className="badge">replayed report · today&apos;s universe and prices</span>
            )}
            {screenLike && (
              <span className="dim">
                {view.kind === 'report'
                  ? `${view.report.picks.length} picks of ${view.report.universe_size} · ${view.report.skipped.length} skipped`
                  : `${view.reply.screen.picks.length} picks of ${view.reply.screen.ranked.length + view.reply.screen.skipped.length} · ${view.reply.screen.skipped.length} skipped`}
              </span>
            )}
          </div>

          <Section n={2} title="Regime" sub="大盘状态 · SPY 对 200 日均线" />
          <RegimeBanner regime={regime} />

          {view.kind === 'report' && view.report.scorecard && (
            <>
              <Section n={3} title="Scorecard" sub="命中率 · 过去每份报告的候选 vs 之后走势 vs SPY" />
              <ScorecardPanel scorecard={view.report.scorecard} />
            </>
          )}

          <Section n={4} title="Picks" sub="今天的候选 · 规则选出的前 8 只,ETF 不算 · 点一行看详情" />
          <PicksTable picks={picks} selected={selected} onSelect={setSelected} />

          {view.kind === 'report' && (
            <>
              <Section n={5} title="Watchlist" sub="你一直看的名字 · 不管规则选不选都有新闻和解读 · rank 是今天在全部排名里的位置" />
              <WatchlistTable items={watchlist} selected={selected} onSelect={setSelected} />
            </>
          )}

          {current && (
            <Section n={6} title="Detail" sub={`详情 · ${current.scored.symbol} 的指标、AI 解读、新闻、价格图`} />
          )}
          {current && (
            <PickDetail
              pick={current}
              asOf={current.scored.as_of}
              onClose={() => setSelected(null)}
            />
          )}

          {screenLike && (
            <>
              <Section n={7} title="By sector / theme" sub="分组平均分 · n 是组里有几只,n=1 就是那一只自己的分" />
              <Aggregates bySector={screenLike.by_sector} byTheme={screenLike.by_theme} />
            </>
          )}

          {view.kind === 'report' && (
            <Section n={8} title="Run" sub="这次运行 · 耗时、LLM 花费、警告、没打分的名字" />
          )}
          {view.kind === 'report' && (
            <footer className="hint">
              <div>
                generated {new Date(view.report.generated_at).toISOString().replace('T', ' ').slice(0, 16)}Z ·{' '}
                {view.report.run.elapsed_ms} ms
                {view.report.run.llm_calls + view.report.run.llm_cached > 0 &&
                  ` · llm ${view.report.run.llm_calls} calls, ${view.report.run.llm_cached} cached, $${view.report.run.llm_cost_usd.toFixed(4)}`}
              </div>
              {view.report.run.warnings.map((w) => (
                <div key={w}>warning: {w}</div>
              ))}
              {view.report.skipped.length > 0 && (
                <details>
                  <summary>{view.report.skipped.length} names not scored</summary>
                  {view.report.skipped.map((k) => (
                    <div key={k.symbol}>
                      <code>{k.symbol}</code> {k.reason}
                    </div>
                  ))}
                </details>
              )}
            </footer>
          )}
        </>
      )}
    </div>
  )
}
