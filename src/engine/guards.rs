//! Deterministic safety guards (Phase 3) — the only thing between a bad LLM output
//! and a real order, so pure + unit-tested per rule:
//!   parse-fail -> Hold · MAX_NOTIONAL cap · SELL bounded by position · Hold on
//!   compute()==None. Crypto allows fractional qty (no whole-share clamp).

// Phase 3: pub fn apply(decision, position, buying_power, cfg) -> Decision.
// Pure functions, no I/O. Empty until Phase 3.
