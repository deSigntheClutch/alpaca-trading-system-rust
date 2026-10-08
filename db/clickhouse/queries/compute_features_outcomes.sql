-- Recompute features and outcomes from bars. Idempotent: ReplacingMergeTree
-- de-duplicates on the ORDER BY key at merge time; run OPTIMIZE ... FINAL after.
-- Features so far: mom_12_1 (12-1 month momentum), hi52_dist (distance to 52-week high),
-- rel_vol_20 (today's volume / 20-day average).

INSERT INTO alpaca.features
SELECT symbol, day, 'mom_12_1', c21 / c252 - 1
FROM (
    SELECT symbol, toDate(ts) AS day,
           lagInFrame(close, 21)  OVER w AS c21,
           lagInFrame(close, 252) OVER w AS c252
    FROM alpaca.bars WHERE timeframe = '1Day'
    WINDOW w AS (PARTITION BY symbol ORDER BY ts ROWS BETWEEN 252 PRECEDING AND CURRENT ROW)
) WHERE c252 > 0;

INSERT INTO alpaca.features
SELECT symbol, toDate(ts), 'hi52_dist', close / hi252 - 1
FROM (
    SELECT symbol, ts, close,
           max(high) OVER (PARTITION BY symbol ORDER BY ts ROWS BETWEEN 251 PRECEDING AND CURRENT ROW) AS hi252
    FROM alpaca.bars WHERE timeframe = '1Day'
);

INSERT INTO alpaca.features
SELECT symbol, toDate(ts), 'rel_vol_20', volume / avg20
FROM (
    SELECT symbol, ts, volume,
           avg(volume) OVER (PARTITION BY symbol ORDER BY ts ROWS BETWEEN 20 PRECEDING AND 1 PRECEDING) AS avg20
    FROM alpaca.bars WHERE timeframe = '1Day'
) WHERE avg20 > 0;

INSERT INTO alpaca.outcomes
WITH fwd AS (
    SELECT symbol, toDate(ts) AS day, close,
           leadInFrame(close, 5)  OVER w AS c5,
           leadInFrame(close, 20) OVER w AS c20
    FROM alpaca.bars WHERE timeframe = '1Day'
    WINDOW w AS (PARTITION BY symbol ORDER BY ts ROWS BETWEEN CURRENT ROW AND 20 FOLLOWING)
),
spy AS (SELECT day, c5 / close - 1 AS s5, c20 / close - 1 AS s20 FROM fwd WHERE symbol = 'SPY' AND c20 > 0)
SELECT f.symbol, f.day, 5,  f.c5  / f.close - 1 AS r, r - spy.s5
FROM fwd f JOIN spy ON spy.day = f.day WHERE f.symbol != 'SPY' AND f.c20 > 0
UNION ALL
SELECT f.symbol, f.day, 20, f.c20 / f.close - 1 AS r, r - spy.s20
FROM fwd f JOIN spy ON spy.day = f.day WHERE f.symbol != 'SPY' AND f.c20 > 0;

OPTIMIZE TABLE alpaca.features FINAL;
OPTIMIZE TABLE alpaca.outcomes FINAL;
