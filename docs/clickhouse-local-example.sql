-- ================================================================
-- ClickHouse is a database with many tables. This builds the three from #88:
--   bars      raw daily candles, one row per (symbol, day)
--   features  factor values, one row per (symbol, day, feature)
--   outcomes  what happened next, one row per (symbol, day, holding period)
-- ================================================================

CREATE TABLE bars (
    symbol    LowCardinality(String),
    ts        DateTime,
    open      Float64, high Float64, low Float64, close Float64,
    volume    Float64
) ENGINE = MergeTree ORDER BY (symbol, ts);

-- Load straight from the repo's JSONL files (ts is epoch milliseconds)
INSERT INTO bars
SELECT symbol, toDateTime(intDiv(ts, 1000)), open, high, low, close, volume
FROM file('bars/*.jsonl', JSONEachRow);

SELECT '== bars: one row per candle ==' AS note;
SELECT symbol, toDate(ts) AS day, close, volume FROM bars WHERE symbol = 'NVDA' ORDER BY ts DESC LIMIT 3;
SELECT symbol, count() AS rows, min(toDate(ts)) AS first, max(toDate(ts)) AS last FROM bars GROUP BY symbol ORDER BY symbol;

-- ----------------------------------------------------------------
-- features: derived from bars. Two factors here:
--   mom_12_1   return from 12 months ago to 1 month ago (about 252 to 21 trading days)
--   hi52_dist  distance from close to the 52-week high (0 = making a new high)
-- ----------------------------------------------------------------
CREATE TABLE features (
    symbol  LowCardinality(String),
    day     Date,
    feature LowCardinality(String),
    value   Float64
) ENGINE = MergeTree ORDER BY (feature, day, symbol);

INSERT INTO features
SELECT symbol, day, 'mom_12_1', c21 / c252 - 1
FROM (
  SELECT symbol, toDate(ts) AS day,
         lagInFrame(close, 21)  OVER w AS c21,
         lagInFrame(close, 252) OVER w AS c252
  FROM bars WINDOW w AS (PARTITION BY symbol ORDER BY ts ROWS BETWEEN 252 PRECEDING AND CURRENT ROW)
) WHERE c252 > 0;

INSERT INTO features
SELECT symbol, day, 'hi52_dist', close / hi252 - 1
FROM (
  SELECT symbol, toDate(ts) AS day, close,
         max(high) OVER (PARTITION BY symbol ORDER BY ts ROWS BETWEEN 251 PRECEDING AND CURRENT ROW) AS hi252
  FROM bars
);

SELECT '== features: one row per (symbol, day, feature) ==' AS note;
SELECT * FROM features WHERE symbol = 'NVDA' AND day = '2026-09-30' ORDER BY feature;
SELECT '-- momentum ranking on 2026-09-30:' AS note;
SELECT symbol, round(value, 3) AS mom_12_1, rank() OVER (ORDER BY value DESC) AS rnk
FROM features WHERE feature = 'mom_12_1' AND day = '2026-09-30' ORDER BY rnk;

-- ----------------------------------------------------------------
-- outcomes: also derived from bars. For every symbol and day, the return over the
-- next 5 and 20 days, and the excess over SPY's return in the same window. This is the scorecard's table.
-- ----------------------------------------------------------------
CREATE TABLE outcomes (
    symbol  LowCardinality(String),
    day     Date,
    horizon UInt8,
    fwd_ret Float64,
    excess  Float64
) ENGINE = MergeTree ORDER BY (horizon, day, symbol);

INSERT INTO outcomes
WITH fwd AS (
  SELECT symbol, toDate(ts) AS day, close,
         leadInFrame(close, 5)  OVER w AS c5,
         leadInFrame(close, 20) OVER w AS c20
  FROM bars WINDOW w AS (PARTITION BY symbol ORDER BY ts ROWS BETWEEN CURRENT ROW AND 20 FOLLOWING)
),
spy AS (SELECT day, c5 / close - 1 AS s5, c20 / close - 1 AS s20 FROM fwd WHERE symbol = 'SPY' AND c20 > 0)
SELECT f.symbol, f.day, 5 AS horizon, f.c5 / f.close - 1 AS fwd_ret, fwd_ret - spy.s5 AS excess
FROM fwd f JOIN spy ON spy.day = f.day WHERE f.symbol != 'SPY' AND f.c20 > 0
UNION ALL
SELECT f.symbol, f.day, 20 AS horizon, f.c20 / f.close - 1 AS fwd_ret, fwd_ret - spy.s20 AS excess
FROM fwd f JOIN spy ON spy.day = f.day WHERE f.symbol != 'SPY' AND f.c20 > 0;

SELECT '== outcomes: one row per (symbol, day, horizon) ==' AS note;
SELECT symbol, day, horizon, round(fwd_ret * 100, 2) AS fwd_pct, round(excess * 100, 2) AS vs_spy_pct
FROM outcomes WHERE symbol = 'NVDA' AND day = '2026-08-28' ORDER BY horizon;

-- ----------------------------------------------------------------
-- IC table: this is #59. features JOIN outcomes, grouped by feature, year and horizon,
-- Spearman rank correlation between the factor value and the forward return.
-- ----------------------------------------------------------------
SELECT '== IC table: feature x year x horizon ==' AS note;
SELECT feature, toYear(day) AS year, horizon,
       round(avg(ic), 3) AS mean_ic,
       round(avg(ic) / stddevSamp(ic), 2) AS ir,
       count() AS days
FROM (
  SELECT f.feature AS feature, f.day AS day, o.horizon AS horizon, rankCorr(f.value, o.excess) AS ic
  FROM features f
  JOIN outcomes o ON o.symbol = f.symbol AND o.day = f.day
  GROUP BY feature, day, horizon
  HAVING count() >= 6
)
GROUP BY feature, year, horizon
ORDER BY feature, year, horizon;
