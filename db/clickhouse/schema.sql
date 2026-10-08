-- Research tables (#88). Three tables, one per access pattern:
--   bars      raw daily OHLCV, one row per (symbol, day)         — written by backfill / the morning run
--   features  one row per (symbol, day, feature)                 — computed from bars
--   outcomes  one row per (symbol, day, horizon): forward return and excess over SPY — computed from bars
-- ORDER BY is the primary index: every query below filters on its leading columns.

CREATE DATABASE IF NOT EXISTS alpaca;

CREATE TABLE IF NOT EXISTS alpaca.bars (
    symbol    LowCardinality(String),
    timeframe LowCardinality(String) DEFAULT '1Day',
    ts        DateTime,
    open      Float64,
    high      Float64,
    low       Float64,
    close     Float64,
    volume    Float64
) ENGINE = ReplacingMergeTree
ORDER BY (symbol, timeframe, ts);

CREATE TABLE IF NOT EXISTS alpaca.features (
    symbol  LowCardinality(String),
    day     Date,
    feature LowCardinality(String),
    value   Float64
) ENGINE = ReplacingMergeTree
ORDER BY (feature, day, symbol);

CREATE TABLE IF NOT EXISTS alpaca.outcomes (
    symbol  LowCardinality(String),
    day     Date,
    horizon UInt8,          -- trading days held: 5 or 20
    fwd_ret Float64,        -- close[t+h] / close[t] - 1
    excess  Float64         -- fwd_ret minus SPY's over the same window
) ENGINE = ReplacingMergeTree
ORDER BY (horizon, day, symbol);
