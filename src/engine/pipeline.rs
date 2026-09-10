//! Agent pipeline (Phase 3): window -> signal (core) -> analyst (LLM) -> decision
//! (LLM) -> guards -> execute -> persist each step (WAL). Generic over the ports.
//!
//! Write ordering is load-bearing: `order_intent` row BEFORE submit, `order_result`
//! AFTER — a crash between the two must never lose the record. See DESIGN.md.

// Phase 3: struct Pipeline<M, L, S> { bars/quotes source, llm, store, guards cfg }.
// Intentionally empty until then — no speculative scaffolding.
