/**
 * Wire contract between the SPA and the backtest API.
 *
 * These types are HAND-WRITTEN on purpose. The Rust side that produces them does
 * not exist yet: BT-6 (#53) reshapes `BacktestResult`, BT-7 (#50) adds the
 * endpoints. When BT-6 lands, ts-rs will generate the real types into
 * `src/bindings/` and this file should be deleted in favour of the generated
 * ones — the shapes below are written to match what those issues specify, so the
 * swap should be a change of import paths and nothing else.
 *
 * Timestamps are epoch milliseconds. ts-rs renders Rust `i64` as `bigint`, but
 * serde puts a JSON number on the wire, so at runtime they arrive as `number`.
 */

/** Which decision path produced the run. `rule` costs nothing and is instant. */
export type Arm = 'rule' | 'llm' | 'llm+debate'

export type Action = 'Buy' | 'Sell' | 'Hold'

/** What the user configures and POSTs to `/api/backtest/run`. */
export interface RunRequest {
  /** Symbols that are actually traded. */
  symbols: string[]
  /** Read-only symbols that inform the decision but are never traded (STRAT-2). */
  context_symbols: string[]
  /** Trailing window in days. Mutually exclusive with `start`/`end`. */
  days?: number
  /** `YYYY-MM-DD`. Used together with `end` instead of `days`. */
  start?: string
  end?: string
  starting_cash: number
  arm: Arm
  fee_bps: number
  slippage_bps: number
  /** Accept an LLM call estimate above the server's limit. Set by the client. */
  confirm?: boolean
}

/**
 * Snapshot of everything that produced a result, echoed back so two runs can be
 * compared and attributed to whatever changed between them (BT-6).
 */
export interface RunConfig {
  arm: Arm
  /** Model id for an LLM arm; empty for the rule arm. */
  model: string
  /** `anthropic` | `bedrock` | `` for the rule arm. */
  adapter: string
  fee_bps: number
  slippage_bps: number
  starting_cash: number
  window_start: string
  window_end: string
  symbols: string[]
  context_symbols: string[]
  git_sha: string
  /** Days whose decision failed and were recorded as a Hold. */
  error_days: number
}

/** One decision day: the chart x-axis, the price line, and what was decided. */
export interface DayRecord {
  ts: number
  close: number
  score: number
  rsi: number
  action: Action
  /** Filled quantity in the base asset; 0 for Hold. */
  qty: number
  /** Portfolio value after this day's fill — the equity curve. */
  equity: number
  decision_reason: string
  analyst_text: string
}

/**
 * One lot: a completed or still-open round trip. This is the only source for
 * "did this trade make money" (BT-2). An open position leaves `exit_*` null.
 *
 * Accounting is per lot, FIFO. Each entry opens its own `Trade`, so every buy
 * marker on the chart maps to exactly one row, and a sell that closes three
 * lots produces three rows sharing an `exit_ts`. The alternative, blending
 * scale-ins into a single averaged position, loses the marker-to-row link and
 * hides the per-entry outcome. Worth matching on the Rust side.
 */
export interface Trade {
  symbol: string
  entry_ts: number
  entry_price: number
  exit_ts: number | null
  exit_price: number | null
  qty: number
  /** Net of fees on both legs. */
  pnl: number
  pnl_pct: number
  fees: number
  entry_reason: string
  exit_reason: string
}

export interface Metrics {
  total_return: number
  max_drawdown: number
  sharpe: number
  num_trades: number
  win_rate: number
  avg_win: number
  avg_loss: number
  /**
   * Gross wins over gross losses. `null` when there are no losing trades, which
   * would otherwise be infinity — and `serde_json` writes non-finite floats as
   * `null` anyway, so the Rust side must send `Option<f64>` here rather than
   * `f64::INFINITY`. The UI renders null as ∞ when there were wins.
   */
  profit_factor: number | null
  /** Average share of capital deployed, so the benchmark is comparable. */
  exposure: number
  buy_hold_return: number
  total_fees: number
}

/** One traded symbol's result. A run returns one of these per traded symbol. */
export interface BacktestResult {
  run_id: string
  config: RunConfig
  symbol: string
  starting_cash: number
  days: DayRecord[]
  trades: Trade[]
  metrics: Metrics
}

/** Row in the run history list (`GET /api/backtest/runs`). */
export interface RunSummary {
  run_id: string
  created_at: number
  config: RunConfig
}

/** `GET /api/backtest/runs/{id}/status` — the LLM arm takes minutes. */
export interface RunStatus {
  run_id: string
  state: 'queued' | 'running' | 'done' | 'error'
  /** 0..1, or null when the backend cannot estimate. */
  progress: number | null
  message: string
}
