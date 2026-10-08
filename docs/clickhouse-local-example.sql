-- ================================================================
-- ClickHouse 是一个数据库,里面有多张表。这里建三张,和 #88 的设计一样:
--   bars      原始日线,一行一根 K 线
--   features  因子值,一行一个 (股票, 日期, 因子名)
--   outcomes  事后结果,一行一个 (股票, 日期, 持有天数)
-- ================================================================

CREATE TABLE bars (
    symbol    LowCardinality(String),
    ts        DateTime,
    open      Float64, high Float64, low Float64, close Float64,
    volume    Float64
) ENGINE = MergeTree ORDER BY (symbol, ts);

-- 直接从仓库里的 JSONL 文件灌进去(ts 是毫秒 epoch)
INSERT INTO bars
SELECT symbol, toDateTime(intDiv(ts, 1000)), open, high, low, close, volume
FROM file('bars/*.jsonl', JSONEachRow);

SELECT '== bars: 一行一根 K 线 ==' AS note;
SELECT symbol, toDate(ts) AS day, close, volume FROM bars WHERE symbol = 'NVDA' ORDER BY ts DESC LIMIT 3;
SELECT symbol, count() AS rows, min(toDate(ts)) AS first, max(toDate(ts)) AS last FROM bars GROUP BY symbol ORDER BY symbol;

-- ----------------------------------------------------------------
-- features:从 bars 算出来。这里算两个因子:
--   mom_12_1   12 个月前到 1 个月前的涨幅(约 252 到 21 个交易日)
--   hi52_dist  收盘价离 52 周最高价的距离(0 = 正在创新高)
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

SELECT '== features: 一行一个 (股票, 日, 因子) ==' AS note;
SELECT * FROM features WHERE symbol = 'NVDA' AND day = '2026-09-30' ORDER BY feature;
SELECT '-- 2026-09-30 这天按动量排名:' AS note;
SELECT symbol, round(value, 3) AS mom_12_1, rank() OVER (ORDER BY value DESC) AS rnk
FROM features WHERE feature = 'mom_12_1' AND day = '2026-09-30' ORDER BY rnk;

-- ----------------------------------------------------------------
-- outcomes:同样从 bars 算。每只股票每一天,往后 5 天和 20 天的收益,
-- 以及减去 SPY 同期收益后的超额收益。这就是 scorecard 用的表。
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

SELECT '== outcomes: 一行一个 (股票, 日, 持有天数) ==' AS note;
SELECT symbol, day, horizon, round(fwd_ret * 100, 2) AS fwd_pct, round(excess * 100, 2) AS vs_spy_pct
FROM outcomes WHERE symbol = 'NVDA' AND day = '2026-08-28' ORDER BY horizon;

-- ----------------------------------------------------------------
-- IC 表:这就是 #59。features JOIN outcomes,按因子、年、持有天数,
-- 算因子值和前瞻收益的 Spearman 秩相关。
-- ----------------------------------------------------------------
SELECT '== IC table: 因子 × 年 × horizon ==' AS note;
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
