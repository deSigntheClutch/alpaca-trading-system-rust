/**
 * The only place the SPA talks to the outside world.
 *
 * By default it calls the run API (#50). Synthetic results are still available for
 * working on the page with no backend at all:
 *
 *     NEXT_PUBLIC_USE_FIXTURES=true npm run dev
 *
 * The real flow is asynchronous on purpose. The rule arm answers in
 * milliseconds, but an LLM arm runs for minutes, so a run is started, polled,
 * and then fetched. Fixtures follow the same three steps with an artificial
 * delay, so the loading and progress states are exercised during development
 * rather than discovered later.
 */
import { fixtureRun } from './fixtures'
import type { BacktestResult, RunRequest, RunStatus, RunSummary } from './types'

// `NEXT_PUBLIC_` is Next's rule for env vars that may reach the browser; it is
// inlined at build time, so a static export bakes the choice in.
const USE_FIXTURES = process.env.NEXT_PUBLIC_USE_FIXTURES === 'true'

/**
 * Above this many estimated LLM calls the server refuses a run unless the request
 * says `confirm: true`; mirror it here so the user is asked before, not after.
 * Keep in step with `LLM_CALL_LIMIT` on the web server (default 200).
 */
const LLM_CONFIRM_ABOVE = 200

/** Rough LLM call count for a request, mirroring the estimate BT-7 enforces. */
export function estimateLlmCalls(req: RunRequest): number {
  if (req.arm === 'rule') return 0
  const rounds = req.arm === 'llm+debate' ? 1 : 0
  const days = req.days ?? 90
  return req.symbols.length * days * (2 + 2 * rounds)
}

async function json<T>(res: Response): Promise<T> {
  if (!res.ok) {
    const body = await res.text().catch(() => '')
    throw new Error(`HTTP ${res.status}${body ? `: ${body.slice(0, 200)}` : ''}`)
  }
  return res.json() as Promise<T>
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

/**
 * Start a run and resolve once it finishes.
 *
 * `onProgress` is called as the backend reports it, so a long LLM run shows
 * movement instead of a frozen spinner.
 */
export async function runBacktest(
  req: RunRequest,
  onProgress?: (s: RunStatus) => void,
): Promise<BacktestResult[]> {
  if (USE_FIXTURES) {
    const runId = `fixture_${Date.now().toString(36)}`
    onProgress?.({ run_id: runId, state: 'running', progress: 0.4, message: 'simulating (fixture data)' })
    await sleep(250)
    const out = fixtureRun(req)
    onProgress?.({ run_id: runId, state: 'done', progress: 1, message: 'done' })
    return out
  }

  const estimate = estimateLlmCalls(req)
  let confirm = false
  if (estimate > LLM_CONFIRM_ABOVE) {
    if (typeof window !== 'undefined' && !window.confirm(`This run makes about ${estimate} LLM calls. Continue?`)) {
      throw new Error('run cancelled')
    }
    confirm = true
  }
  const started = await json<{ run_id: string }>(
    await fetch('/api/backtest/run', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ ...req, confirm }),
    }),
  )

  // Poll until terminal. The rule arm is usually done on the first check.
  for (;;) {
    const status = await json<RunStatus>(await fetch(`/api/backtest/runs/${started.run_id}/status`))
    onProgress?.(status)
    if (status.state === 'done') break
    if (status.state === 'error') throw new Error(status.message || 'run failed')
    await sleep(1000)
  }

  return json<BacktestResult[]>(await fetch(`/api/backtest/runs/${started.run_id}`))
}

/** Past runs, newest first. Empty while fixtures are in use. */
export async function listRuns(): Promise<RunSummary[]> {
  if (USE_FIXTURES) return []
  return json<RunSummary[]>(await fetch('/api/backtest/runs'))
}

export const usingFixtures = USE_FIXTURES
