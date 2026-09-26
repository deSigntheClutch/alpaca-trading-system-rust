# Veteran Review — alpaca-trading-system-rust

*Reviewed 2026-09-20 at commit `797670f` plus the uncommitted `read_events` port. Perspective: a
buy-side PM / risk / systems veteran asked "will this make money, and if not, why not?"
Verdict first, then the reasoning. Every code claim below was checked against the tree.*

---

## 0. Scorecard

| Dimension | Score /10 | One line |
|---|---|---|
| Edge / alpha hypothesis | **1.5** | None stated, none measurable. The LLM sees nothing the SMA/RSI signal did not already produce. |
| Risk management & sizing | **3** | Good order-level hygiene; no exits, no portfolio limits, sizing collapses to a flat $100 clip. |
| Backtest & evaluation rigor | **2** | Gross, fee-free, 90 days, non-reproducible, benchmark not comparable. Cannot prove or disprove anything. |
| Execution & data quality | **3** | Correct WAL ordering and idempotency; fills never read, partial-bar signal, 422 misread. |
| LLM-agent design | **3** | Clean seams, but the LLM is asked to do the one thing it is worst at (size in BTC units without a price). |
| Engineering craft | **7.5** | Ports/adapters, WAL, Decimal money, backtest reuses live pipeline, 24 tests. Above average for a solo repo. |
| Research iteration speed | **4** | Every experiment is ~720 paid, uncached, unrepeatable LLM calls; no LLM-free baseline. |
| Safety hygiene (paper) | **6** | Fail-closed live barrier, parse-fail to Hold, idempotent orders. Missing kill switch and reconciliation. |
| **Overall as a trading system** | **3** | A well-built execution shell wrapped around a strategy that does not exist yet. |
| **Overall as an engineering/learning project** | **7** | The plumbing is roughly two years ahead of the finance. |

**The one-sentence diagnosis:** the project currently has no way to be wrong. There is no written
objective, no baseline to beat, no fee model, no forward paper record accruing, and the only number it
produces (gross return of a ~1%-exposed book next to 100% buy-and-hold) cannot distinguish skill from
luck. Until that changes, every feature added is unfalsifiable.

---

## 1. The single bottleneck: no measurement apparatus, no clock running

Seven of the findings below are facets of one problem. I collapse them here because fixing them
separately misses the point.

1. **No objective or kill rule is written anywhere.** `DESIGN.md` §8 says "hit-rate vs buy-hold" and
   that is the entire evaluation plan. Nobody has written down what net Sharpe, what max drawdown,
   against what benchmark, over what horizon, would make you keep or kill the signal, or keep or kill
   the LLM layer. Without a pre-registered bar, numbers get rationalized instead of judged.
2. **No LLM-free baseline exists.** The pipeline's only decision path runs through the trader LLM
   (`pipeline.rs:177-191`). You cannot answer the first question any desk asks of a discretionary
   overlay: does it beat the systematic rule net of cost? The rule already exists (`signals.rs:72-82`
   computes a `Rating` and then drops it).
3. **The backtest is a gross upper bound with a broken benchmark.** Fills at the same close the decision
   saw, zero fees, zero slippage (`backtest.rs:105-121`, acknowledged in the file header). Every BUY is
   capped at `MAX_NOTIONAL` ($100 default, `config.rs:14`) on a $10,000 sim book (`main.rs:170`), so
   the strategy is ~1% invested per day while `buy_hold_return` is 100% invested. The headline card
   measures cash drag and market direction, not decision quality.
4. **Nothing is reproducible.** Both adapters send only model, max_tokens, system, messages
   (`claude.rs:53-58`, `bedrock.rs:72-76`). Two identical runs produce different trades. There is no
   response cache, so every rerun pays full price and takes ~40-100 minutes. Results overwrite
   `backtest.jsonl` (`main.rs:155-157`) with no model id, prompt version, debate rounds, or fee
   assumptions persisted. Error days are silently scored as Hold (`backtest.rs:245-257`).
5. **Sample size is meaningless.** One trailing 90-day window, two assets with ~0.8 daily correlation,
   one point estimate, no interval. With ~180 heavily autocorrelated decisions the standard error on
   any Sharpe dwarfs any plausible edge.
6. **The forward clock is not running.** `main.rs` is one-shot. 22 of 24 commits landed on Sept 9-10;
   the last two added a debate step and an agents refactor. Not one added a scheduler. For a daily-bar
   strategy, the only evidence that is fee-inclusive, free, and immune to LLM memory contamination is
   a forward paper record, and it accrues at exactly one observation per symbol per day. Six months
   from when the scheduler ships is six months later than six months from today.
7. **LLM knowledge look-ahead is unguarded.** The no-lookahead test (`backtest.rs:369-381`) covers bars
   only. The analyst prompt sends absolute BTC price levels (`analyst.rs:28-31`); a model that has read
   2024-2025 crypto news can date "63,400 after a run from 58k" and "remember" what followed. The
   current 90-day window is probably post-cutoff, so today's numbers are likely clean, but the natural
   fix for (5), extending the window, walks straight into this hole for the LLM arm.

**What a PM would do first (roughly two days of work, ports already exist):** write a one-page charter
with a net-Sharpe-vs-vol-matched-benchmark target and explicit kill rules; add a rule-based
`LlmClient` (signal → action, no network), a fee/slippage term, and net Sharpe/Sortino/max-DD to the
backtest; cron the rule baseline daily into a dedicated paper account so the out-of-sample clock
starts this week. Only after the baseline number exists does it make sense to ask what, if anything,
an LLM should be given to read.

---

## 2. What is not designed or planned well (ranked by consequence)

### 2.1 There is no alpha hypothesis, and the LLM is structurally unable to supply one

- **Every agent's market input is derived from the daily close series and nothing else.** Analyst:
  score, RSI, last close, ten undated closes (`analyst.rs:28-31`). Bull/bear: the analyst's prose plus
  score and RSI (`researcher.rs:74-79`). Trader: analyst prose, debate, score, position qty, buying
  power, and *not the current price* (`trader.rs:57-59`). No volume, no OHLC range, no volatility, no
  dates, no funding, no basis, no news, no order book. The LLM is the entire decision function over
  three close-derived scalars plus account state. Any mapping it learns over those inputs could be a
  deterministic Rust rule at zero cost and full reproducibility.
- **The signal itself is a display heuristic, not a researched factor.** `signals.rs` is ported from a
  heatmap, self-labelled "naive educational", with hand-set weights (0.4/0.35/0.25), fixed percentage
  scales (±8%, ±5%) that are not vol-normalized, and trend and momentum terms that are both deviations
  from SMA20 and therefore largely collinear. Its tests are two sign checks on linear ramps. Its
  information content on daily BTC/ETH has never been measured. 60 bars are fetched; 21 reach the
  signal; 10 reach the LLM.
- **The debate adds cost and variance, not information.** Bull and bear receive a subset of what the
  analyst saw and are told which side to argue (`researcher.rs:13-20`). Same three numbers, two forced
  rationalizations. Bear always speaks last, and the trader is told to "prefer HOLD when uncertain"
  (`trader.rs:16`) while the analyst is told to be "non-committal about risk" (`analyst.rs:11-12`).
  The net effect is a structural Hold tilt that nobody has measured. It shipped with fake-LLM tests
  only and no ablation. In the backtest the debate text is discarded (`CollectingStore` ignores the
  `debate` step), so its influence on the trader cannot even be inspected afterwards.
- **Long-only spot momentum on two ~0.8-correlated assets is one beta bet with double fee drag.**
  Alpaca crypto does not short, so in a bear regime the best this system can do is sit in cash, and
  nothing in it can learn that it should.

### 2.2 Decisions are memoryless delta orders. This one design choice causes four other findings.

The trader emits `BUY|SELL|HOLD` plus a qty in base units (`trader.rs:13-16`). `Decision`
(`types.rs:76-84`) has no target weight or exposure field. Consequences:

- **Sizing collapses to a flat $100 clip.** The trader is never told the price or the cap, so its qty
  is unanchored; the guard's `min(qty, max_notional/price, buying_power/price)` (`guards.rs:41-47`) is
  the only sizing logic in the system. `Decision.score` is never used for sizing. No volatility
  estimate exists anywhere because only closes are consumed.
- **There is no exit engine.** The trader never sees entry price, unrealized P&L, or holding time
  (`TraderInput`, `trader.rs:20-28`). `Position` and `RawPosition` (`types.rs:87-92`, `rest.rs:151-156`)
  discard Alpaca's `avg_entry_price` and `unrealized_pl`. Guards contain only entry clamps. The only
  exit path is the LLM emitting SELL from a lagging SMA read under a Hold-preferring prompt. In a
  trend strategy the exit rule is where most of the risk-adjusted return comes from.
- **Held exposure is the integral of every past LLM whim.** A stateless daily agent that emits
  increments cannot say "I want to be 40% long"; exits need a separate SELL the agent has no basis for;
  portfolio caps have to be bolted on as more increment clamps.
- **No portfolio-level risk controls.** `GuardConfig` has one field (`guards.rs:16-19`). No per-symbol
  or gross exposure cap as % of equity, no daily loss stop, no high-water-mark drawdown halt, no max
  orders per day, no operator halt flag. The `Broker` port does not even fetch equity
  (`RawAccount` parses only `buying_power`, `rest.rs:147-149`). Fine for a one-shot $100 paper process;
  a hard prerequisite the day a resident loop or real capital arrives.

### 2.3 The execution feedback loop is missing

- **Fills are never read.** `order_result` persists only Alpaca's order id and the (almost always
  non-terminal) status at submit time (`pipeline.rs:243-249`, `RawOrder` at `rest.rs:139-144`). No
  filled price, filled qty, fill time, or fee enters the WAL. Realized P&L and slippage are
  unknowable from local data, and live-paper can never be reconciled against the fee-free backtest.
  Recoverable later via `client_order_id`, but not persisted.
- **Live decides on an in-progress bar at an unpinned wall-clock time.** `daily_bars` requests 1Day
  bars with `start`, `limit`, `sort=desc`, no `end` (`rest.rs:89-101`); the adapter's own comment says
  Alpaca returns the current day's bar. Crypto trades 24/7, so the last close the signal weights most
  is a partial bar whose value depends on whether you ran at 00:05 or 23:55 UTC. The backtest sees
  complete bars and fills at their close. "Identical decision chain" in `DESIGN.md` §6 is therefore
  not true on the bar that matters.
- **Idempotency key has no side.** `client_order_id = strategy:symbol:UTC-day` (`pipeline.rs:69-70`).
  A same-day re-run whose guarded action is the opposite side (LLM flip on the moving partial bar,
  crash retry, or the documented `FORCE_SIDE=buy` then `FORCE_SIDE=sell`) gets a 422, resolves to the
  morning's order without comparing side (`rest.rs:225-227`; `Order` carries no side), and is recorded
  and printed as executed. The WAL says a trade happened that did not.
- **Every 422 is treated as a duplicate.** The body is never read, so a sub-$10 notional, an
  over-precise qty (the guard divides a `Decimal` and `to_string()` sends up to 28 decimals,
  `rest.rs:210`), or any other validation failure surfaces as a misleading 404 from the follow-up
  lookup. A failed submit leaves an `order_intent` with no `order_result`/`order_error` step and
  nothing reconciles intents on startup.
- **Market + GTC from a process that exits immediately** (`rest.rs:212-213`). On a thin or halted 24/7
  venue that order can rest and fill later with no one watching, at a price nobody recorded.
- **No data-quality gate on bars.** A missing symbol key becomes an empty vec (`rest.rs:117`),
  unparseable timestamps are silently dropped (`rest.rs:120-131`), and nothing checks bar count,
  freshness, gaps, or zero volume before a confident market order is sized off `closes.last()`.

### 2.4 LLM-layer specifics that will bite

- **Sonnet 5 runs adaptive thinking by default, and `temperature` is rejected on it.** The Anthropic
  adapter (`claude.rs:53-58`) sends no `thinking` and no `output_config`, so every call thinks at
  default effort `high`, thinking tokens count against `MAX_TOKENS = 1024` (`llm/mod.rs:14`), and
  `stop_reason` is never read (`claude.rs:83-87`). A reply truncated mid-JSON becomes a Hold that is
  indistinguishable in the WAL from a deliberate Hold. The determinism lever you have is
  `output_config.effort: "low"` plus a content-addressed response cache, not temperature 0 (which
  would 400 on this model). Structured output via `output_config.format` would replace the
  first-brace-to-last-brace slicing in `trader.rs:107-113`.
- **The two backends run different model generations.** Anthropic default `claude-sonnet-5`
  (`claude.rs:13`); Bedrock default `claude-sonnet-4-5` (`bedrock.rs:18`). Printed at startup, never
  persisted. Results from the two paths are not comparable and nothing labels which produced what.
- **The cost banner undercounts by 2x.** `main.rs:151` computes `symbols × days × 2` while
  `DEBATE_ROUNDS` defaults to 1 (`main.rs:128-133`), making 4 calls per decision.
- **The trader parser, called "the first line of defense" (`trader.rs:4`), has zero tests**, and every
  fake LLM in the repo returns BUY. A regression to "all Hold" would look like a conservative strategy.
- **No outcome labels, no per-agent attribution.** `outcome` is listed in `schema.cql:14` and written
  nowhere. The just-added `read_events` port has no consumer. You cannot say whether analyst, debate,
  or trader earns its cost.

### 2.5 Concrete bugs found along the way

| Where | What |
|---|---|
| `pipeline.rs:68` + `:154-168` vs `schema.cql:17` | `ts` is computed once per cycle; bull and bear both write step `debate` with the same `ts`. Scylla PK is `((strategy_id, symbol), ts, step)`, so the bear row **overwrites** the bull row in production. `MemStore` appends lines so the JSONL trail hides it. |
| `main.rs:151` | LLM call estimate is half the real count at default `DEBATE_ROUNDS=1`. |
| `rest.rs:225-227` | Every 422 → duplicate lookup; real rejection reason discarded. |
| `pipeline.rs:172-173`, `:297-305` | Position lookup is by symbol only. `FORCE_SIDE` test positions and any future second strategy contaminate `position_qty` and every attribution number. The account is the book. |
| `main.rs:159-176` | Backtest runs each symbol as an independent $10k book with hardcoded `strategy_id: "backtest"`; live is one shared account. No portfolio equity curve, no cross-symbol drawdown, and sim `buying_power` (raw cash) has different semantics from Alpaca's (marginable). |
| `pipeline.rs:67-70` in backtest | Every simulated day gets wall-clock `ts` and the same `decision_id`. Harmless with the throwaway `CollectingStore`; fatal the moment a persistent store is threaded into the backtest. |

### 2.6 Effort allocation

ScyllaDB with QUORUM writes, TTL buckets, outage fallback and replay, plus a WebSocket ingester, for a
workload of roughly 7-9 event rows per symbol per day. `DESIGN.md` §8 itself notes Scylla is weak at
aggregation and defers SQL analytics, yet the learning loop is a join-and-aggregate problem: decision →
t+N return, hit rate by score bucket, by regime, by prompt version. That is twenty lines of SQL over a
flat table or hundreds of lines of hand-rolled Rust over JSON blobs. Meanwhile `tempo = "stream"`,
`CryptoWs`, `Ingester` and `alpaca_data_ws` are dead code, and the one thing a daily strategy actually
needs, a scheduler, does not exist. The craft is real; it is pointed at the wrong layer.

---

## 3. What is genuinely good (keep it)

- Five clean ports with blanket `&T` impls so one client backs every symbol and the whole pipeline
  runs against fakes (`traits.rs`).
- WAL discipline: `order_intent` before the broker call, `order_result` after, and a test that
  asserts the ordering (`pipeline.rs:406-438`).
- Idempotent `client_order_id` with 422 resolution; fail-closed live barrier (`config.rs:79-82`);
  parse failure collapses to Hold; `Decimal` for money.
- **The backtest reuses the live `Pipeline` with swapped ports.** This is the single most important
  architectural decision in the repo and most retail systems get it wrong. Every fix below slots into
  an existing seam.
- ts-rs DTOs as one source of truth across the Rust/TypeScript boundary.
- The dashboard's premise, "see where it bought and why," is the right instinct for a learning system.

---

## 4. What to build to have a chance at profit (sequenced, not a menu)

A solo developer cannot ship sixteen features. The order below is the order in which each step makes
the next one measurable. Effort assumes one competent Rust developer.

### Phase 0 — This week. Make the project falsifiable and start the clock. (2-3 days)

1. **Research charter in `DESIGN.md`.** Objective: net Sharpe and max drawdown versus a
   volatility-matched BTC/ETH benchmark over a stated horizon. Kill rules: if the LLM-free rule
   baseline is net-negative after fees over available history, replace the signal before touching
   prompts; if the LLM arm does not beat the rule arm by a stated margin with a bootstrap interval on
   ≥N post-cutoff decisions, remove the LLM from the decision path. Zero code.
2. **Rule-based `LlmClient` (or a `Decider` seam).** `Rating` → action, no network. This is the control
   arm for everything that follows.
3. **Start the forward paper clock.** A resident daily loop (`tempo = "cron"`) that wakes just after
   the UTC day boundary, fetches only completed bars (`end` = start of today, or drop the last bar),
   checks `by_client_order_id` before spending a single LLM call, and runs the rule baseline with
   `DEBATE_ROUNDS=0` into a dedicated paper account. Weekly one-page scorecard from the event log.

### Phase 1 — Research harness. Make one experiment cheap, honest, and repeatable. (1-2 weeks)

4. **Fill model in `SimMarket`:** fee bps per side, slippage bps, fill at next bar open. A fill
   ledger (price, qty, fee, reference price) per trade.
5. **Metrics that a methodology review would accept:** net Sharpe, Sortino, Calmar, hit rate, payoff
   ratio, turnover, average exposure, per-trade P&L, drawdown duration. Benchmark scaled to the
   strategy's average exposure, or return on deployed capital.
6. **Portfolio-level sim:** one `SimMarket` holding N symbols and one cash balance, one equity curve,
   portfolio max drawdown, gross exposure series; `strategy_id` from config.
7. **Experiment registry:** `run_id`, git sha, model id, adapter, prompt hash, effort, debate rounds,
   window, fee assumptions, error-day count in every result; append, never overwrite.
8. **LLM determinism and cost:** `output_config.effort: "low"` on the trader call, raise `max_tokens`,
   log `stop_reason` and `usage`, structured output via `output_config.format`, content-addressed
   disk cache keyed on (model, system, user), K repeated runs with mean/std. Parser tests for
   SELL/HOLD/fenced/truncated replies.
9. **Cutoff-aware evaluation:** a per-model cutoff date constant; tag each backtest day
   `pre_model_cutoff`; evaluate the LLM arm only on post-cutoff dates plus forward paper; run the rule
   arm over full multi-year history. Add `end` and pagination to `daily_bars` so rolling windows over
   years are possible for the rule arm.
10. **Rolling multi-window evaluation with bootstrap intervals** on daily returns, rule arm first.

### Phase 2 — Turn the signal generator into a strategy. (2-3 weeks)

11. **Target-exposure decision contract.** The rule (or LLM) emits a target weight `w ∈ [0,1]` per
    symbol; Rust computes `delta = w·equity/price − position` and places one order. Add `equity` to
    the `Broker` port. This one change replaces separate entry-clamp, exit-engine, vol-sizing and
    exposure-cap features: exits are `w → 0`, rebalancing is `w` changing, vol-targeting is
    `w = target_vol / realized_vol`, gross cap is `Σw ≤ 1`.
12. **Volatility-targeted sizing in Rust.** Notional = risk budget / ATR% or realized vol. Constant
    risk per position across BTC/ETH and across regimes is the most reliable Sharpe improvement in
    the practitioner literature, and it cuts exposure exactly when tail risk spikes. If an LLM stays
    in the loop, it emits direction plus a bounded conviction, never a qty.
13. **Deterministic exits the LLM cannot override:** ATR-based stop, time stop, trailing exit, in the
    guard layer. Surface `avg_entry_price`, unrealized P&L and holding period to any agent that
    remains.
14. **Portfolio risk layer:** per-symbol and gross exposure caps as % of equity, high-water-mark
    drawdown kill switch, daily loss stop, max orders/day, operator halt flag, bar data-quality gate
    (count, freshness, gaps, finite non-zero close/volume, partial-bar flag) that fails closed to an
    explicit `data_quality` Hold.
15. **Fill reconciliation and order lifecycle:** poll to terminal status, persist a `fill` step with
    price/qty/fee and slippage vs reference, side-aware idempotency key, parse 422 bodies, `notional`
    orders with the $10 floor, startup reconciler for orphan intents, IOC or marketable-limit from a
    fresh quote instead of market GTC.
16. **Strategy-level position book** reconstructed from own fills (`client_order_id` already carries
    `strategy_id`); bound SELLs by that book, not the account. Until then, one paper account per
    strategy.

### Phase 3 — Look for edge where it has historically existed. (ongoing)

17. **Expand the universe and the horizon.** The best-documented systematic anomaly in crypto is
    time-series momentum at 1-to-12-month lookbacks and cross-sectional momentum across liquid pairs.
    Two correlated symbols is one bet; 8-12 liquid Alpaca pairs ranked by vol-scaled TSMOM is a real
    cross-section. A BTC-level regime filter (above/below a long SMA) that scales gross exposure is
    how a long-only constraint stops being pure beta.
18. **Strategy variants in `strategies.toml`:** signal params, lookbacks, target vol, notional, debate
    rounds, prompt variant. Run the grid LLM-free on the cached harness; compare on out-of-sample
    net Sharpe. Today every variant is a code change.
19. **Outcome labeling and per-stage attribution.** Forward returns at t+1/5/20 per `decision_id`; hit
    rate by score bucket, regime, prompt version; P&L attributed to signal-only vs +analyst vs +debate
    vs +trader. Do this in SQL (SQLite or DuckDB over a flat export of events and bars), keep the
    `Store` port, and stop fighting Scylla for aggregation.

### Phase 4 — Give the LLM a job it can actually do. (only after Phase 3 says the rule has legs)

20. **Feed the LLM orthogonal, unstructured information and have it emit a structured, cited
    feature.** Headlines, exchange notices, funding and basis, macro calendar, ETF flow reports, the
    ETH/BTC ratio. Output a bounded sentiment/risk score with citations via `output_config.format`,
    logged as one more Rust-side feature whose predictive value is measured like any other. This is
    the only place an LLM can plausibly contribute alpha in this architecture: price-only signals see
    news-driven regime breaks late.
21. **Debate: default off, A/B on the cached window, and if it survives give each side information
    the other lacks** (bear gets realized vol and drawdown; bull gets trend persistence), randomize
    speaker order, or replace both with one evidence-conditioned critic returning a probability.
22. **Role-appropriate models.** Decision role on the strongest model at low effort for consistency;
    any surviving prose roles on Haiku 4.5. Same model generation on both adapters so results are
    comparable. Per-decision LLM cost is already an order of magnitude below the exchange taker fee;
    the lever is quality per call, not price per token.
23. **Stocks and ETFs via the cron tempo.** More informational edge is available to an LLM in
    earnings, filings and news than in BTC price levels, and the market is closed 17 hours a day,
    which makes decision timing a non-issue.

---

## 5. What to expect, honestly

- **The most probable result of Phase 1 is that a daily SMA/RSI long-only on BTC/ETH loses to
  buy-and-hold net of ~25 bps taker fees per side, and that an LLM re-reading three scalars cannot
  fix that.** That is not a failure of the project; it is the project finally producing a
  falsifiable result. Confirm your actual Alpaca fee tier; it decides whether daily turnover is
  affordable at all.
- **The realistic win condition for a long-only crypto system is not beating BTC on raw return.** It
  is a trend- and volatility-targeted overlay that delivers a materially higher Sharpe and Calmar
  than holding BTC, mostly by sidestepping the worst drawdowns. That is an achievable, documented
  outcome, and it requires multi-year evaluation and vol-based sizing, not prose.
- **Calendar time is the constraint you cannot buy back.** At $100 clips the LLM bill is irrelevant
  and the fee is the enemy. Every week the forward paper clock is not running is a week of
  out-of-sample evidence you will never have.
- **The engineering is good enough to carry a real strategy.** The seams are in the right places. What
  is missing is the finance: an objective, a baseline, costs, exits, sizing, and a scoreboard.

---

## 6. If you only do three things on Monday

1. Write the charter with kill rules. One page.
2. Add the rule-based decider, a fee term, and net Sharpe to the backtest. Run it LLM-free over as
   many years of daily bars as Alpaca will give you.
3. Cron the rule baseline into a paper account on completed bars. Start the clock.

---

## 7. Plan of record — 2026-09-20, after scoping to the owner's goals

Two stated goals, in the owner's own priority order:

1. **Learn `tokio`, ScyllaDB, and LLM interaction** by building this.
2. **Make money without making the system complicated.** This is P0.

These pull in different directions, so the plan resolves the conflict explicitly rather
than pretending it is not there.

### 7.1 The architectural decision

The simplest thing with a real chance of making money is boring: trend following with
volatility-targeted sizing, rebalanced daily to a target weight. It does not need an LLM.
The LLM is the least likely part of this system to earn and the most interesting part to
build, which is a fine reason to build it and a bad reason to put it in the decision path.

So the LLM moves **beside** the chain, not inside it. Rust computes the rule and places the
order. The LLM does what it is actually good at, which is reading unstructured text, and
emits a structured, scored feature that is evaluated on its own merits. The LLM learning
goal survives intact and the LLM loses the ability to quietly destroy P&L.

### 7.2 What each learning goal actually maps to

| Goal | Issues that teach it |
|---|---|
| `tokio` | #39 scheduler (UTC boundary math, `interval`, `select!`, graceful shutdown, backoff); making `run_strategy` concurrent, since it is a sequential `for` loop today; #26 fill polling with timeout and retry; #30 paginated concurrent backfill |
| ScyllaDB | #45 clustering-key collision, a real bug and the best Scylla lesson in the repo; #35 partition-local read path; a second table keyed for a different access pattern, later |
| LLM interaction | #7 structured output; #42 effort / `max_tokens` / `stop_reason` / `usage`; #13 response cache; measuring whether an agent earns its cost; tool use, later |

`tokio` is barely exercised today because `main` is one-shot, so the scheduler is both the
best tokio exercise available and the highest-priority profit item. That overlap is free
and it is why phase 1 starts there.

### 7.3 Phases

**phase:1 — answer "does this make money", and start the clock**
#19 charter · #30 multi-year bars · #32 rule baseline · #12 fees · #11 risk-adjusted metrics ·
#39 daily scheduler · #45 debate-row bug

**phase:2 — make the record honest**
#26 fill reconciliation · #42 LLM call controls · #7 structured output · #13 response cache ·
#35 outcome labels

**phase:3 — act on what phases 1 and 2 found**
#21 keep-or-replace the signal · #23 target-exposure contract · #15 news as a scored feature

Note on #23: a target weight is **less** complex than delta orders, not more. Exits,
rebalancing, volatility targeting and gross caps all collapse into
`order = w·equity/price − position`. It is a simplification that happens to also be correct.

### 7.4 What was deferred, and why

Eighteen issues from the department sweep were closed as not-planned-now. They were not
wrong, they were early: most sat two or more levels deep behind unfinished work, and a
46-issue backlog on a solo side project is functionally the same as an empty one. Each
carries a close comment with an explicit reopen trigger.

Where a closed issue had a cheap, high-value half, that half was folded into a phase 1 or 2
issue instead of being left blocked:

| Closed | Cheap half folded into |
|---|---|
| #44 debate ablation | #42 — `DEBATE_ROUNDS=0` default and the 2x-wrong cost print, which halves LLM spend immediately |
| #41 experiment registry | #32 — run_id, model, git sha on results; stop overwriting `backtest.jsonl` |
| #27 order hygiene | #26 — side-aware `client_order_id` |
| #25 kill switch | #39 — halt flag checked before any order |

One recommendation from section 4 is **withdrawn**: #31, exporting to SQLite or DuckDB for
analytics. It conflicts with both stated goals at once. Scylla as a write-ahead log is the
correct use of Scylla and is what the owner wants to learn; aggregation can be read back
through `read_events` and computed in Rust in a few dozen lines. Revisit only when the Rust
side genuinely hurts.

### 7.5 Taxonomy

The `dept:` labels from section 1 did their job, which was to find the gaps, and have been
deleted. They answered "what is missing"; `phase:` answers "what is next", which is the
question that matters day to day. The department framing is preserved in each issue body
and in sections 1 through 4 above. The ten surviving issues also carry the repo's original
component labels, so `backtest`, `llm`, `risk` and the rest now filter across both the
original 18 issues and these.
