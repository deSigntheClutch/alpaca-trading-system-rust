//! Domain: pure types, typed errors, dependency-free indicator math, and config.
//! No I/O — everything here is data + math the rest of the system speaks in.

pub mod config;
pub mod dto; // wire DTOs for the web API + ts-rs frontend types
pub mod error;
pub mod signals;
pub mod types;
pub mod universe; // config/universe.csv → Instrument / Universe
