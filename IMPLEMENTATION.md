# Implementation tasks

Dependency-ordered, grouped into **waves** that can run in parallel. Each task lists
its deps, its parallel track, and a definition-of-done (one runnable check — ponytail).

See `DESIGN.md` for the why. `core::signals` (T0) is already done.

## Dependency graph

```
W0  T1 infra        T2 guards        T3 config          (all parallel, no deps)
        │
W1  T4 alpaca ──┐   T5 llm ──┐   T6 store (needs T1) ──┐   (3 parallel tracks)
        │       │            │                          │
W2  T7 fetch(T4)│   T8 analyst(T5) ‖ T9 decision(T5)    │
        └───────┴── T10 execute+WAL (T4,T6,T2) ─────────┘
                         │
                    T11 orchestrator run() (T7,T8,T9,T10,T2)
                         │
W3               T12 CLI/app (T11,T3)
                         │
W4  T13 decision_id + outcome job (T6,T11) ── T14 L0 eval (T13)
                         │
W5  T15 sentiment ‖ T16 risk (T11) │ T17 WS source (T7) │ T18 Dockerfile+deploy (T12)
```

Critical path: **T1 → T6 → T10 → T11 → T12 → T13 → T14**. Everything else feeds in.

## Tasks

### W0 — prereqs (parallel, start now)
- **T1 Infra** — `docker-compose.yml` with single-node Scylla; `.env` from `.env.example`; create keyspace `trading`. *DoD:* `docker compose up -d` + `CREATE KEYSPACE trading ...` succeeds via cqlsh. *Deps:* none.
- **T2 Guards** (`guards.rs`, pure) — parse-fail→Hold, qty clamp + `MAX_NOTIONAL`, SELL bounded by position, buying-power check, `compute()==None`→Hold. *DoD:* unit test per rule. *Deps:* none. **Highest-value test surface — the only thing between a bad LLM output and a real order.**
- **T3 Config/strategies** (`lib.rs`) — `Config = Vec<Strategy>`; each `Strategy { id, watchlist, tempo, signal_params, alpaca_key/secret (or shared+tag), base }`; load from TOML/env. Replaces scaffold's flat `Config` + `human_in_loop`. *DoD:* load a 2-strategy file, assert parsed. *Deps:* none.

### W1 — leaf clients (3 parallel tracks)
- **T4 Alpaca client** (`alpaca.rs`) — `daily_bars` (batch `/v2/stocks/bars?symbols=`), `latest_quote`, `submit_order` (+`client_order_id`), `positions`, `get_order_by_client_id`, `account` (buying_power). Auth headers. *DoD:* smoke fetch AAPL bars prints closes; deser unit test on canned JSON. *Deps:* none.
- **T5 Claude client** (`llm.rs`) — `message(system,user)` via Anthropic Messages API (`x-api-key`, `anthropic-version`, model, `max_tokens`), parse `content[0].text`. **Verify `claude-sonnet-5` id first.** *DoD:* smoke — non-empty reply to fixed prompt. *Deps:* none.
- **T6 Scylla store** (`store.rs`, was `audit.rs`) — create `events` + `market_data` tables; session builder; `append_event(...)`; **local-file fallback on write error**. *DoD:* integration test append→select round-trip. *Deps:* T1.

### W2 — pipeline (`pipeline.rs`; internal parallelism)
- **T7 fetch** — bars + quote (T4), positions + open orders. *Deps:* T4.
- **T8 analyst agent** ‖ **T9 decision agent** — T8: signals(core)+bars → Claude → analysis text. T9: analysis+positions+signals → `Decision` (strict JSON parse). Parallel with each other. *Deps:* T5 (+core).
- **T10 execute + WAL** — `client_order_id` = `(strategy_id, symbol, trading_day)`; write `order_intent` → `submit_order` → `order_result`; on 422 dup GET existing + still persist; Scylla-fail → fallback. *Deps:* T4, T6, T2.
- **T11 orchestrator `run()`** — fetch→guards→analyze→decide→guards→execute, persist each step. **Fixes the scaffold's execute→audit ordering to WAL.** *Deps:* T7, T8, T9, T10, T2.

### W3 — CLI
- **T12 app/main** — `--strategy <id>`, `--confirm` opt-in gate, `--live`/`ALLOW_LIVE` barrier with loud base-URL print, loop watchlist → `run()`. Rewrite scaffold (drop `--watchlist`/`--auto`/`human_in_loop`). *DoD:* `--strategy x` on paper does one full dry-run cycle. *Deps:* T11, T3.

### W4 — learning loop
- **T13 decision_id + outcome job** — stable `decision_id`; `outcome` subcommand reads fills + t+1/5/20d marks, appends `outcome` rows. *Deps:* T6, T11.
- **T14 L0 eval** — pull `events` partition into Rust; hit-rate + avg forward return per rating/score bucket vs buy-hold, per strategy. *DoD:* prints a table over sample data. *Deps:* T13.

### W5 — extend (later)
- **T15 Sentiment** (Alpaca news) ‖ **T16 Risk manager** — slot into pipeline. *Deps:* T11.
- **T17 WS data source** — pluggable source #2 (intraday). *Deps:* T7's source seam.
- **T18 Dockerfile + deploy** — multi-stage Rust → distroless; compose (agent + Scylla) on VM. *Deps:* T12.

## How to parallelize

- **Solo**: follow the critical path (T1→T6→T10→T11→T12); build T2/T3 in the gaps.
- **Multi-agent / worktrees** (each track is a mostly-independent file, so few merge conflicts):
  - **data track**: T4 → T7 → T10
  - **LLM track**: T5 → T8 ‖ T9
  - **infra track**: T1 → T6
  - **safety track**: T2 · **config track**: T3
  - Join at T11 (orchestrator), then T12.
- Wave barriers are real: T10 needs the data+store+guards tracks all landed; T11 needs everything in W2.
