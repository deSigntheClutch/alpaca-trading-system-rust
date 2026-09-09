# alpaca-trading-system-rust — Multi-Agent AI Trading System

A Rust port of the pattern in Alpaca's *Building a Multi-Agent AI Trading System*:
a pipeline of specialized LLM agents that analyze a symbol and place a trade on a
**paper** account, with a human-in-the-loop gate and an append-only audit log.

> The Alpaca tutorial is Python (CrewAI/LangGraph). We stay in Rust to reuse the
> deterministic `core` crate and to keep learning async Rust (tokio). The
> orchestration is a ~5-step pipeline — hand-rolled, no agent framework needed.

## Decisions (confirmed)

- **Language**: Rust. **LLM**: Claude (`claude-sonnet-5` for reasoning agents,
  `claude-haiku-4-5-20251001` for cheap classification later).
- **Trading**: Alpaca **paper only**. Live is a deliberate config change, never a default.
- **Scope v1**: thin slice — Market Data → Technical Analyst → Decision → gate → Execution.
  Sentiment (news) + Risk Manager agents come after the slice works end to end.
- **Deterministic math stays in Rust**: agents call `alpaca_core::signals` as a
  tool and hand the model the numbers. The LLM does judgment, never arithmetic.
- **Human-in-the-loop** approval before every order (`--auto` to skip).
- **Audit**: every agent output + order appended to a JSONL file. No DB in v1.

## Architecture

```
watchlist ─► orchestrator (tokio, per symbol)
                │
                ▼
   ┌────────────────┐   ┌─────────────────────┐   ┌──────────────┐
   │ Market Data     │──►│ Technical Analyst   │──►│ Decision      │
   │ Alpaca bars/    │   │ core::signals (tool)│   │ Buy/Sell/Hold │
   │ quotes          │   │ + Claude interprets │   │ + qty + reason│
   └────────────────┘   └─────────────────────┘   └──────┬───────┘
                                                          │  human gate
                                                   ┌──────▼───────┐
                                                   │ Execution     │
                                                   │ Alpaca order  │
                                                   │ (PAPER)       │
                                                   └──────┬───────┘
                                                          ▼
                                             audit.jsonl (every step)

   later: Sentiment (Alpaca news) + Risk Manager slot between Analyst and Decision.
```

## Layout

```
crates/
├── core/    [pure] indicators + composite signal (agent tools). No deps. cargo test -p alpaca-core.
├── agents/  Alpaca client, Claude client, agents + orchestrator, audit log.
│   └── src/{alpaca,llm,audit,pipeline}.rs, lib.rs (Config)
└── app/     CLI binary `alpaca-agent --watchlist AAPL,MSFT [--auto]`.
```

## Build order

1. ✅ Cleanup + git init + this rewrite + compiling scaffold.
2. Alpaca paper client (`alpaca.rs`): bars, quote, submit_order, positions.
3. Claude client (`llm.rs`) + Technical Analyst end to end on one symbol.
4. Decision + Execution + orchestrator (`pipeline.rs`) + audit (`audit.rs`).
5. Human gate + CLI polish. Then: Sentiment + Risk agents.

## Open questions

- Watchlist defaults? Position sizing rule for the Risk agent (fixed $ / % equity / signal-scaled)?
- Should the Decision agent see current positions (avoid re-buying)? (Fetch via `positions()`.)
- Audit path — fixed `audit.jsonl` or per-run file?

## Not doing (yet)

- No QuestDB/Scylla (old design) — JSONL audit is enough for v1.
- No web dashboard — dropped `crates/web`; can return over the audit log later.
- No agent framework, no LLM tool-use round-trip — signals computed in Rust, fed as text.
- Not investment advice; the signal is a naive educational heuristic.
```
