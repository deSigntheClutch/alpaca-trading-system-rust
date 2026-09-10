-- Apply: docker exec -i alpaca-scylla cqlsh < db/schema.cql
-- Dev: SimpleStrategy RF=1. Prod bumps RF and uses NetworkTopologyStrategy.

CREATE KEYSPACE IF NOT EXISTS trading
  WITH replication = {'class': 'SimpleStrategy', 'replication_factor': 1};

-- Append-only event log: one row per pipeline step. `step` is in the clustering
-- key so two steps at the same ts don't overwrite each other. Everything for a
-- (strategy, symbol) lands in one partition -> outcome join stays partition-local.
CREATE TABLE IF NOT EXISTS trading.events (
  strategy_id text,
  symbol      text,
  ts          timestamp,
  step        text,        -- market_data | analyst | decision | guard | order_intent | order_result | outcome
  decision_id text,        -- = strategy_id : symbol : trading_day
  payload     text,        -- JSON blob for that step
  PRIMARY KEY ((strategy_id, symbol), ts, step)
) WITH CLUSTERING ORDER BY (ts DESC, step ASC);

-- Market-data firehose. Partition bucketed by day so partitions stay bounded
-- (a symbol streamed 24/7 would otherwise grow one unbounded partition forever).
-- Raw ticks expire; the event log above is kept forever.
CREATE TABLE IF NOT EXISTS trading.market_data (
  symbol text,
  day    date,             -- partition bucket
  ts     timestamp,
  open   double,
  high   double,
  low    double,
  close  double,
  volume double,
  PRIMARY KEY ((symbol, day), ts)
) WITH CLUSTERING ORDER BY (ts DESC)
  AND default_time_to_live = 2592000;  -- 30 days
