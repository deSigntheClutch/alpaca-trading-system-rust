# Implementation plan — crypto-first, by phase

Built in phases up the trait seams (`src/traits.rs`): each phase turns one `todo!()`
into a real impl, leaving a runnable check behind. Crypto is first; the stock
milestone is a later phase that reuses the same `MarketStream`/`BarSource` seams.

See `DESIGN.md` for the why. Status: ✅ done · ▶ next · ○ later.

## The ports (what each phase fills in)

```
MarketStream  live ticks      → Phase 2 ✅ (CryptoWs)
BarSource     historical bars → Phase 2 ✅ (AlpacaRest, crypto bars endpoint)
Store         WAL + bars      → Phase 1 ✅ append_event / Phase 2 ✅ write_bars / Phase 4a ✅ replay
LlmClient     analyst/decide  → Phase 3 ✅ (Claude)
Broker        orders/account  → Phase 3 ✅ (AlpacaRest) — added as a 5th port so execution is fake-testable
```

## ✅ Phase 0 — skeleton (commit 3258f00)

Single package, layered module folders, all four ports defined, infra/engine stubbed
with `todo!()`. Config loads TOML + env with the **fail-closed live barrier**
(non-paper base aborts unless `ALLOW_LIVE=1`). `docker-compose.yml` + `db/schema.cql`.
*DoD:* `cargo run` prints config + strategies; `cargo test` green.

## ✅ Phase 1 — ScyllaDB store (commit 499a8f3)

`ScyllaStore::connect` (session + prepared stmt), `append_event` at **QUORUM** with a
**local-file JSON-line fallback** so an order is never recorded nowhere.
*DoD:* fallback test green; `#[ignore]` live round-trip hook (needs Scylla).

## ✅ Phase 2 — market data ingest

- **2a `CryptoWs::subscribe`** ✅ — WS connect → in-band auth → subscribe → `BoxStream<Tick>`
  (unfold + buffer to flatten multi-tick frames). Tested: frame parsing + handshake classify.
- **2b `AlpacaRest::daily_bars`** ✅ — crypto bars endpoint (`sort=desc` → recent window,
  reversed to chronological). Works for `"BTC/USD"`; same seam serves stocks later.
- **2c `ScyllaStore::write_bars`** ✅ — INSERT into `market_data`, `CqlDate` day bucket
  (encoding verified from crate source). Tested: `cql_day` epoch offset.
- **2d `Ingester`** ✅ — tokio owner task → `broadcast` fan-out → exp-backoff reconnect.
  Tested with a fake `MarketStream`, no network.

## ✅ Phase 3 — decision pipeline + execution

- **3a Guards** ✅ (`engine/guards.rs`, pure) — Hold/non-positive→Hold, `MAX_NOTIONAL` cap,
  buying-power clamp, SELL ≤ position, parse-fail→Hold. Unit test per rule.
- **3b `Claude::message`** ✅ (`infra/llm.rs`) — Anthropic Messages API.
- **3c Execute + WAL** ✅ (`infra/alpaca.rs`, `Broker`) — submit order + positions + account;
  `client_order_id` idempotency (422 → resolve existing); `order_intent`→submit→`order_result`.
- **3d Pipeline** ✅ (`engine/pipeline.rs`) — bars→signal→analyst→decision(strict JSON)→guards
  →execute, persisting each step. Tested end-to-end with fakes (clamp + WAL ordering).
- **Runner** ✅ (`main.rs`) — one decision cycle per symbol; fails loud without Scylla,
  exits cleanly without creds.

## Phase 4 — durability + learning loop

- **4a Fallback replay** ✅ — `ScyllaStore::replay_fallback` drains the local JSON-line file
  into Scylla on startup; kept lines = still-failing. Tested: write→parse round-trip.
- **4b `decision_id` + outcome job** ▶ next — read fills + t+N forward marks → `outcome`
  rows. Needs accumulated trade history + Scylla `SELECT` seam (not yet added).
- **4c L0 eval** ▶ next — hit-rate / forward-return per score bucket vs buy-hold. Builds on 4b.

> 4b/4c are analytics over history — they only produce meaning once cycles have run and
> written events. Deferred deliberately (not blind-built); the WAL they read from is done.

## ○ Later — extend

Sentiment agent · risk manager · **stock milestone** (new `BarSource`/`MarketStream`
impls, no pipeline change) · Dockerfile → distroless deploy.
