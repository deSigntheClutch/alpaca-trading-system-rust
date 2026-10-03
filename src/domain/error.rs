//! Typed errors at the library boundary (thiserror). `anyhow` stays in the app
//! binary only. Each port has its own error so callers can match on the failure.

use thiserror::Error;

/// Market-data source failures (REST or WS).
#[derive(Debug, Error)]
pub enum MarketError {
    #[error("connect failed: {0}")]
    Connect(String),
    #[error("subscribe failed: {0}")]
    Subscribe(String),
    #[error("decode failed: {0}")]
    Decode(String),
    #[error("request failed: {0}")]
    Request(String),
    #[error("timed out")]
    Timeout,
}

/// Fundamentals-source failures (earnings calendar, market cap). REST only.
#[derive(Debug, Error)]
pub enum FundamentalError {
    #[error("request failed: {0}")]
    Request(String),
    #[error("decode failed: {0}")]
    Decode(String),
    #[error("rate limited by vendor")]
    RateLimited,
    #[error("API key missing: {0}")]
    MissingKey(&'static str),
    #[error("timed out")]
    Timeout,
}

/// `config/universe.csv` failures. Every variant that comes from a row names the
/// 1-based line in the file, so a bad edit is a one-glance fix.
#[derive(Debug, Error)]
pub enum UniverseError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("missing required column `{0}` in header")]
    MissingColumn(&'static str),
    #[error("line {line}: {source}")]
    Row {
        line: u64,
        #[source]
        source: csv::Error,
    },
    #[error("line {line}: empty symbol")]
    EmptySymbol { line: u64 },
    #[error("line {line}: duplicate symbol {symbol} (first seen on line {first})")]
    Duplicate {
        symbol: String,
        line: u64,
        first: u64,
    },
}

/// Event/market-data store failures.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("connect failed: {0}")]
    Connect(String),
    #[error("query failed: {0}")]
    Query(String),
    #[error("write failed: {0}")]
    Write(String),
}

/// LLM client failures.
#[derive(Debug, Error)]
pub enum LlmError {
    #[error("request failed: {0}")]
    Request(String),
    #[error("bad response: {0}")]
    BadResponse(String),
}

/// Broker (order placement / account) failures.
#[derive(Debug, Error)]
pub enum BrokerError {
    #[error("request failed: {0}")]
    Request(String),
    #[error("broker rejected order: {0}")]
    Rejected(String),
    #[error("decode failed: {0}")]
    Decode(String),
}

/// Config load / validation failures.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("cannot read {0}: {1}")]
    File(String, String),
    #[error("parse error: {0}")]
    Parse(String),
    /// Fail-closed live barrier: refuse a non-paper base unless explicitly allowed.
    #[error("refusing non-paper base {0} without ALLOW_LIVE=1")]
    LiveBlocked(String),
}

/// Anything that can go wrong inside the pipeline; composes the port errors.
#[derive(Debug, Error)]
pub enum PipelineError {
    #[error(transparent)]
    Market(#[from] MarketError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Llm(#[from] LlmError),
    #[error(transparent)]
    Broker(#[from] BrokerError),
    #[error(transparent)]
    Fundamental(#[from] FundamentalError),
    #[error("decision parse failed: {0}")]
    Decision(String),
}
