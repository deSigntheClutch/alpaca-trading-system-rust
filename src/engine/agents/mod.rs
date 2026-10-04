//! Agents: the LLM-driven decision roles. Each agent owns its system prompt, builds the
//! user message from the pipeline's context, and (for the trader) parses the reply back
//! into a typed `Decision`. The pipeline orchestrates them in sequence; guards + WAL
//! persistence stay in the pipeline, not here.
//!
//! Today: `analyst` (reads the signal) → `trader` (decides). Next: a `researcher` module
//! for the bull/bear debate step between them, and few-shot memory injected from
//! `engine::learning` — both additive, no change to this seam.

pub mod analyst;
pub mod commentator; // the morning report's note per pick (#66)
pub mod researcher;
pub mod trader;
