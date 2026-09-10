# alpaca-trading-system-rust — Multi-Agent AI Trading System

A Rust port of the pattern in Alpaca's *Building a Multi-Agent AI Trading System*:
a pipeline of specialized LLM agents that analyze a symbol and place trades on a
**paper** account, with an append-only audit log that doubles as the dataset for
improving the system over time.

> The Alpaca tutorial is Python (CrewAI/LangGraph). We stay in Rust to reuse the
> deterministic `core` crate and to keep learning async Rust (tokio). The
> orchestration is a ~5-step pipeline — hand-rolled, no agent framework needed.

## Decisions (confirmed)

- **Language**: Rust. **LLM**: Claude (`claude-sonnet-5` for reasoning agents,
  `claude-haiku-4-5-20251001` for cheap classification later). Verify the exact
  Messages API model id before the first call or it 400s.
- **Trading**: Alpaca **paper only**. Live is behind an explicit barrier (see Safety).
- **No per-trade human gate.** Runs **unattended by default**; `--confirm` is opt-in
  for when you want to eyeball a run. The gate was throttling the learning loop
  (fewer decision→outcome samples) — dropping it is the right call for paper.
- **Deterministic math stays in Rust**: agents call `alpaca_core::signals` as a
  tool and hand the model the numbers. The LLM does judgment, never arithmetic.
- **Multiple strategies, one account each** (see Strategies & accounts).
- **Run model**: one-shot CLI + external cron/launchd, one schedule per strategy.
  No resident daemon, no DB, no websocket — all premature at daily/paper scale.
- **Persistence**: Alpaca is source of truth for orders/positions. **ScyllaDB from
  day one**, justified by its strong suit — the **intraday/tick market-data layer**
  (`((symbol), ts)`); it also holds the append-only `events` log. Learning-loop eval
  runs in Rust (Scylla weak at aggregation); SQL/QuestDB is the upgrade path.
- **Data source is pluggable**: the pipeline consumes bars/quotes from a source
  it's handed. REST poll is impl #1; a websocket feed drops in as impl #2 with no
  pipeline rewrite.
- **LLM integration**: fixed prompt with pre-computed signals as text for v1; each
  Alpaca fetch stays an independent fn so exposing them as tool-use is additive
  later. No agent framework. Math is always Rust, never the model.

## Architecture

```
strategy config (id, watchlist, tempo, signal params, account) ─┐
                                                                 │  one scheduled
                                                                 ▼  run per strategy
   ┌────────────────┐   ┌─────────────────────┐   ┌──────────────┐   ┌───────────────┐
   │ Market Data     │──►│ Technical Analyst   │──►│ Decision      │──►│ Guards        │
   │ Alpaca bars/    │   │ core::signals (tool)│   │ Buy/Sell/Hold │   │ parse→Hold,   │
   │ quotes (batch)  │   │ + Claude interprets │   │ + qty + reason│   │ clamp qty,    │
   └────────────────┘   └─────────────────────┘   └──────────────┘   │ buying-power  │
                                                                       └──────┬────────┘
              store INTENT (WAL) ◄─────────────────────────────────────       │
                       │                                              ┌────────▼────────┐
                       ▼                                              │ Execution        │
              store RESULT ◄──────────────────────────────────────── │ Alpaca order     │
                       │                                              │ (that strategy's │
                       ▼                                              │  PAPER account)  │
              outcome record (async, t+Nd P&L) ─► eval / few-shot memory (越做越好)

   later: Sentiment (Alpaca news) + Risk Manager slot between Analyst and Decision.
```

## Strategies & accounts

The system runs **several strategies**, each with its own **watchlist, tempo, and
signal/prompt params**. Each strategy is an independent scheduled run.

- **Preferred: one paper account per strategy.** Clean isolation — separate
  positions, buying power, and P&L, so per-strategy attribution is *free* (the
  account balance IS the strategy's score). No cross-strategy interference (one
  strategy buying what another sells).
- **Fallback if the account cap is too low: one shared account + `strategy_id`**
  stamped on every `client_order_id` and every audit/decision/outcome record.
  Attribution is then reconstructable offline from the log; you only lose
  position/buying-power isolation.
- **`strategy_id` is mandatory on every persisted record regardless** — the
  learning loop's per-strategy evaluation depends on it.
- **Tempo = per-strategy schedule.** Daily strategy → daily cron; faster → more
  frequent runs; intraday/crypto → eventually the deferred streaming service.
  Different strategies sit at different points on the frequency axis.
- **Config** becomes a list of strategies, each carrying its own Alpaca key pair
  (or the shared pair + tag). CLI: `alpaca-agent --strategy momentum`.
- **Laziest working shape**: N independent cron lines, one per strategy — no
  cross-strategy supervisor. A supervisor daemon is the trigger-point *only* if
  you later want one process managing many tempos (deferred).

## Safety (no human gate → deterministic guards are load-bearing)

Removing the gate means these are the only thing between a bad LLM output and a
real order. All mandatory:

- **Parse-fail → force Hold.** Unparseable/invalid decision never becomes a trade.
- **Clamp qty**: whole shares, a `MAX_NOTIONAL` cap, SELL bounded by actual position.
- **Buying-power check** before submit (LLM-sized qty can exceed it).
- **Idempotent `client_order_id`** keyed on `(strategy_id, symbol, trading_day)` —
  independent of the LLM's nondeterministic action/qty. On a `422` duplicate,
  GET the existing order and audit it anyway (don't leave an audit hole).
- **`compute() == None`** (too few bars) → explicit Hold/skip.
- **Live barrier** (one-time, not per-trade): auto on *paper* is fine; anything
  that could hit a **live** account requires explicit `--live`/`ALLOW_LIVE=1` +
  a loud startup print of the resolved base URL. Never a silent default.
- **`trading_day`** comes from an ET market calendar, not local time.

## Persistence (what we own vs what Alpaca owns)

**ScyllaDB from day one**, but for the right reason: its genuine strong suit here is
the **intraday/tick market-data layer** (`((symbol), ts)` — firehose writes, read by
symbol+time-range, horizontally scalable), *not* analytics. It also carries the
append-only `events` log. The **learning-loop eval runs in Rust** (pull a partition,
compute) since Scylla is weak at aggregation/joins; a small SQL/QuestDB analytics
store is the upgrade path when eval volume outgrows that. Alpaca stays source of
truth for orders/positions.

| Resource | Store | Consistency | Why |
|---|---|---|---|
| Orders | **Alpaca** (truth) + Scylla event copy | strong (broker) | local copy only drifts; `client_order_id` uniqueness broker-enforced |
| Positions | **Alpaca**, fetched live per run | strong / local ephemeral | caching → acting on stale holdings |
| Decisions + audit (every step, every order) | **Scylla** `events` (append-only) | append-only immutable | the thing we own; one row/step; source for eval + memory |
| Outcome (fill price + t+Nd P&L) | **Scylla** `events`, keyed to `decision_id` | append-only | closes the learning loop (see below) |
| Agent prompts + LLM responses | **Scylla** `events` | append-only | reproducibility + cost tracking |
| Market bars/quotes (daily) | in-memory, refetched from Alpaca | ephemeral | cheap to re-pull; not worth persisting |
| Market data (intraday/tick) | **Scylla** `market_data` `((symbol), ts)` | append-only | Scylla's real use case — firehose writes, read by symbol+range |
| Idempotency key `client_order_id` | broker-enforced, no local store | strong (422) | derived from inputs, recomputed on restart |
| Config + secrets | `.env`, read once | ephemeral in-process | keys only; never written back; gitignored |

**Scylla data model** — everything for a `(strategy, symbol)` lands in one partition, so
the learning-loop queries and the outcome join stay partition-local (no cross-partition scans):

```sql
CREATE TABLE trading.events (
  strategy_id text,
  symbol      text,
  ts          timestamp,
  step        text,   -- market_data | analyst | decision | guard | order_intent | order_result | outcome
  decision_id text,   -- = strategy_id : symbol : trading_day
  payload     text,   -- JSON blob for that step
  PRIMARY KEY ((strategy_id, symbol), ts, step)  -- `step` in the clustering key: two steps at the same ts must not overwrite
) WITH CLUSTERING ORDER BY (ts DESC, step ASC);

-- Scylla's real justification: high-rate market data, always read by symbol + time range.
-- Partition bucketed by `day` so a 24/7-streamed symbol never grows one unbounded partition.
CREATE TABLE trading.market_data (
  symbol text,
  day    date,   -- partition bucket
  ts     timestamp,
  open   double, high double, low double, close double, volume double,
  PRIMARY KEY ((symbol, day), ts)
) WITH CLUSTERING ORDER BY (ts DESC)
  AND default_time_to_live = 2592000;  -- 30d TTL on raw ticks; the event log is kept forever.
```

**Write ordering (WAL) + crash safety — fixes a real bug in the scaffold**:
`pipeline.rs` must write the **`order_intent` row before `submit_order`** and the
**`order_result` row after**. The scaffold's `execute → persist` order loses the
record if a crash lands between the order executing and the write. If the Scylla
write itself fails (node unreachable), fall back to an append-only local file +
`eprintln` so an order is **never** placed with zero record — Scylla being remote
makes this fallback *more* important than it was for a local file.
`ponytail: local fallback file, replayed into Scylla on next run`.

**Deferred until a concrete trigger**: a SQL/QuestDB analytics store (add when eval
aggregation outgrows Rust-over-a-partition); a resident daemon to own the connection
(one-shot opens a session per run).

## Learning loop (越做越好)

The log records *intent*; it can't improve the AI until each decision is joined to
its *outcome*. Ladder, lazy → heavy:

- **decision_id + outcome record**: every decision gets a stable `decision_id`
  (= `strategy_id + symbol + trading_day`); an async job later appends the realized
  fill price + t+1d/5d/20d mark-to-market. Now each row is `(features, score, action,
  reason) → reward`.
- **L0 — measure first (highest ROI)**: pure-Rust eval over an `events` partition pulled into Rust — hit-rate and
  average forward return per rating/score bucket, vs a buy-and-hold benchmark, **per
  strategy**. Answers "does the signal even work?" before any ML.
- **L1 — tune from the eval**: adjust `signals.rs` weights or the decision prompt.
- **L2 — few-shot memory (RAG, no training)**: at decision time, retrieve this
  strategy's past decisions+outcomes on the same symbol/pattern into the prompt.
- **L3 — LLM self-critique**: periodic agent over losing trades → distilled `lessons`
  injected into the system prompt.
- **L4 — fine-tune**: almost certainly YAGNI at this data volume.

**Honest caveats**: tiny sample size (a few decisions/day), non-stationary markets +
nondeterministic LLM, attribution vs the market, and paper ≠ live fills.

## Scaling — real bottlenecks

| Bottleneck | Bites when | Handling |
|---|---|---|
| **LLM cost + latency** (real ceiling) | 2 Claude calls/symbol | gate LLM near the signal's decision boundary (don't fully suppress it → keeps it more than a rubber stamp); Haiku for decide, Sonnet for analyze; bounded concurrency |
| **Alpaca REST rate** (~200/min) | per-symbol loops | multi-symbol batch endpoint `/v2/stocks/bars?symbols=...` → O(1) not O(N) |
| **1 free websocket** | intraday streaming only | daily/REST never touches it; intraday → single owner task + channel fan-out; crypto = separate 24/7 socket |

**Frequency, not symbol count, is the regime change** — daily → cron; intraday →
resident streaming service (deferred). **tokio honesty**: sequential `.await` over a
few daily symbols is already legitimate async; JoinSet/`buffer_unordered` is a
learning exercise, must not cross a `--confirm` gate (stdin race); concurrent
writes are fine — Scylla is built for them (no single-writer bottleneck a local file would force).

## Layout

```
One package, one `src/` — layered by MODULE, not crate (multiple crates was ceremony
at this size). The layering is a reading aid + trait seam, not a build boundary:

```
Cargo.toml            single package `alpaca-trading-system`, bin `crypto-agent`
db/schema.cql         Scylla keyspace + tables (hand-applied; no migration runner yet)
src/
  main.rs             the binary — loads config, wires impls into the engine
  lib.rs              declares the four layers below
  traits.rs           PORTS — MarketStream / BarSource / Store / LlmClient (the seams)
  domain/             types (Symbol, Decimal money, Bar/Tick/Decision) · error (thiserror)
                      · signals + data (dep-free math) · config (TOML+env, live barrier)
  infra/              PORT IMPLS — alpaca (REST + crypto WS) · store (Scylla) · llm (Claude)
  engine/             OVER THE PORTS — ingest (tokio stream) · pipeline (agents+WAL) · guards
```

Traits are the extension point (new source/store = new impl, no pipeline change).
Split a module back into its own crate only if compile time or a 2nd binary demands
it — mechanical, not a redesign.

## Build order

1. ✅ Cleanup + git init + compiling scaffold.
2. Alpaca paper client (`alpaca.rs`): bars (batch), quote, submit_order, positions, order-by-client-id.
3. Scylla `trading.events` table + `store.rs`: session, append event, local-file fallback on write failure.
4. Claude client (`llm.rs`) + Technical Analyst end to end on one symbol (fixed prompt).
5. Decision + Guards + Execution + orchestrator (`pipeline.rs`, WAL persist via `store`).
6. Strategy config (list, per-strategy account + tempo) + `--strategy` CLI. No gate; `--confirm` opt-in.
7. `decision_id` + async outcome records → L0 eval harness. Then: Sentiment + Risk agents.

## Open questions

- **How many Alpaca paper accounts can one login create?** (verify in dashboard →
  decides per-strategy vs shared-account+tag). Which strategies to start with (id, watchlist, tempo, params)?
- Run **during market hours** vs accept+audit **next-open fills** (affects the cron line / market-on-open)?
- Position sizing rule (fixed $ / % equity / signal-scaled)? `MAX_NOTIONAL` value?
- Feed the Decision agent **open orders too** (not just positions) so an unfilled prior BUY doesn't re-trigger?
- Forward-return horizons for outcome records (1d / 5d / 20d)? Audit path: one file or per-strategy/per-run?

## Not doing (yet)

- No SQL/QuestDB analytics store yet — eval runs in Rust over a Scylla partition; add SQL when aggregation volume outgrows that. Scylla is justified by the market-data layer, not the event log.
- No resident daemon, no websocket *yet* — cron one-shot per strategy; data source is pluggable so a WS client drops in later.
- **TODO: web dashboard** (deferred, not v1) — reads over the Scylla event store; dropped `crates/web` for now.
- No agent framework. LLM tool-use deferred — fixed prompt with Rust-computed signals as text for v1; fetches are tool-ready so tool-use is additive when optional data sources (news/fundamentals) land. Math always in Rust.
- No fine-tuning — measure and tune prompts/weights first.
- Not investment advice; the signal is a naive educational heuristic.
```
