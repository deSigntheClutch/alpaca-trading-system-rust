/**
 * The control panel: pick a window, pick symbols, press Run.
 *
 * Everything here is a controlled input over a single `RunRequest`, which is the
 * exact body that gets POSTed to `/api/backtest/run`. Keeping the form state and
 * the wire format identical means there is no mapping layer to drift.
 */
import { useState } from 'react'
import { estimateLlmCalls, usingFixtures } from './api/client'
import type { Arm, RunRequest } from './api/types'

const PRESETS = [30, 90, 180, 365]

const ARMS: { value: Arm; label: string; note: string }[] = [
  { value: 'rule', label: 'Rule', note: 'No LLM. Instant and free.' },
  { value: 'llm', label: 'LLM', note: 'Analyst and trader. Two calls per day.' },
  { value: 'llm+debate', label: 'LLM + debate', note: 'Adds bull and bear. Four calls per day.' },
]

type Props = {
  value: RunRequest
  onChange: (next: RunRequest) => void
  onRun: () => void
  running: boolean
}

export function RunControls({ value, onChange, onRun, running }: Props) {
  const custom = value.days == null
  const set = (patch: Partial<RunRequest>) => onChange({ ...value, ...patch })

  const calls = estimateLlmCalls(value)
  // Rough, and deliberately so: enough to stop someone spending an hour by
  // accident, not an invoice. Real accounting is LLM-1 (#42).
  const cost = (calls * 0.02).toFixed(2)

  return (
    <div className="panel">
      <div className="panel-row">
        <label className="panel-label">Window</label>
        <div className="seg">
          {PRESETS.map((d) => (
            <button
              key={d}
              type="button"
              className={!custom && value.days === d ? 'on' : ''}
              onClick={() => set({ days: d, start: undefined, end: undefined })}
            >
              {d}d
            </button>
          ))}
          <button
            type="button"
            className={custom ? 'on' : ''}
            onClick={() => set({ days: undefined, start: isoDaysAgo(90), end: isoDaysAgo(0) })}
          >
            Custom
          </button>
        </div>
        {custom && (
          <>
            <input
              type="date"
              value={value.start ?? ''}
              onChange={(e) => set({ start: e.target.value })}
            />
            <span className="dim">to</span>
            <input
              type="date"
              value={value.end ?? ''}
              onChange={(e) => set({ end: e.target.value })}
            />
          </>
        )}
      </div>

      <div className="panel-row">
        <label className="panel-label">Traded</label>
        <SymbolField
          symbols={value.symbols}
          onChange={(symbols) => set({ symbols })}
          placeholder="add a symbol to trade"
        />
      </div>

      <div className="panel-row">
        <label className="panel-label">
          Context
          <span className="dim"> (read only)</span>
        </label>
        <SymbolField
          symbols={value.context_symbols}
          onChange={(context_symbols) => set({ context_symbols })}
          placeholder="informs the decision, never traded"
        />
      </div>

      <div className="panel-row">
        <label className="panel-label">Decider</label>
        <div className="seg">
          {ARMS.map((a) => (
            <button
              key={a.value}
              type="button"
              title={a.note}
              className={value.arm === a.value ? 'on' : ''}
              onClick={() => set({ arm: a.value })}
            >
              {a.label}
            </button>
          ))}
        </div>
        {calls > 0 && (
          <span className="warn">
            ≈ {calls.toLocaleString()} LLM calls, roughly ${cost}. The rule arm answers instantly and
            costs nothing.
            {usingFixtures && ' Synthetic results are rule-based whichever arm is picked, so this only changes the estimate.'}
          </span>
        )}
      </div>

      <div className="panel-row">
        <label className="panel-label">Capital</label>
        <NumField
          label="starting cash"
          value={value.starting_cash}
          step={1000}
          onChange={(starting_cash) => set({ starting_cash })}
        />
        <NumField
          label="fee bps"
          value={value.fee_bps}
          step={1}
          onChange={(fee_bps) => set({ fee_bps })}
        />
        <NumField
          label="slippage bps"
          value={value.slippage_bps}
          step={1}
          onChange={(slippage_bps) => set({ slippage_bps })}
        />
        <button
          type="button"
          className="run"
          disabled={running || value.symbols.length === 0}
          onClick={onRun}
        >
          {running ? 'Running…' : 'Run backtest'}
        </button>
      </div>
    </div>
  )
}

/** Chips plus a free-text box. Enter or comma commits, × removes. */
function SymbolField({
  symbols, onChange, placeholder,
}: {
  symbols: string[]
  onChange: (next: string[]) => void
  placeholder: string
}) {
  const [draft, setDraft] = useState('')

  const add = (raw: string) => {
    const s = raw.trim().toUpperCase()
    if (!s || symbols.includes(s)) return
    onChange([...symbols, s])
  }

  return (
    <div className="symbols">
      {symbols.map((s) => (
        <span key={s} className="chip">
          {s}
          <button type="button" onClick={() => onChange(symbols.filter((x) => x !== s))}>
            ×
          </button>
        </span>
      ))}
      <input
        className="chip-input"
        value={draft}
        placeholder={placeholder}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ',') {
            e.preventDefault()
            add(draft)
            setDraft('')
          } else if (e.key === 'Backspace' && draft === '' && symbols.length) {
            onChange(symbols.slice(0, -1))
          }
        }}
        onBlur={() => {
          add(draft)
          setDraft('')
        }}
      />
    </div>
  )
}

function NumField({
  label, value, step, onChange,
}: {
  label: string
  value: number
  step: number
  onChange: (n: number) => void
}) {
  return (
    <label className="numfield">
      <span>{label}</span>
      <input
        type="number"
        value={value}
        step={step}
        min={0}
        onChange={(e) => onChange(Number(e.target.value))}
      />
    </label>
  )
}

function isoDaysAgo(n: number): string {
  return new Date(Date.now() - n * 86_400_000).toISOString().slice(0, 10)
}
