//! Crypto-first multi-agent trading system.
//!
//! Layered by MODULE folder, not by crate — one package, one `src/`. The layering
//! is a reading aid + trait seam, not a build boundary:
//!   domain/  · traits (ports) · infra/ · engine/
//!
//! Traits are the extension point: a new data source or store is a new impl, no
//! pipeline change. Split a folder back into its own crate only if compile time or
//! a second binary ever demands it — mechanical, not a redesign.

pub mod domain; // types, errors, indicator math, config
pub mod engine;
pub mod infra; // port implementations (alpaca / store / llm)
pub mod traits; // ports — the seams infra implements and engine consumes // orchestration over the ports (ingest / pipeline / guards)
