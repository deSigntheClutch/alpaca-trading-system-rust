# ClickHouse: the research store

Three tables under `alpaca`: `bars`, `features`, `outcomes` (`db/clickhouse/schema.sql`).
Bars come from the JSONL cache; features and outcomes are computed in SQL
(`db/clickhouse/queries/compute_features_outcomes.sql`). The IC table is one query
(`db/clickhouse/queries/ic_table.sql`).

## Local

    docker compose up -d clickhouse
    CH_PASSWORD=dev ./scripts/ch-load-bars.sh
    open http://localhost:8123/play          # user alpaca / dev

RustRover: Database tool window → + → ClickHouse → localhost:8123, same user.

## Fly.io

    fly apps create alpaca-clickhouse
    fly volumes create chdata --region sjc --size 3 -a alpaca-clickhouse
    fly secrets set CLICKHOUSE_PASSWORD="$(openssl rand -hex 24)" -a alpaca-clickhouse
    fly deploy -c deploy/clickhouse/fly.toml
    CH_URL=https://alpaca-clickhouse.fly.dev CH_PASSWORD=<the secret> ./scripts/ch-load-bars.sh

Play UI at https://alpaca-clickhouse.fly.dev/play. About $5–7/month (shared-cpu-1x, 1 GB, 3 GB volume).

## No server at all

`clickhouse local` runs the same SQL straight over the files, nothing to start:

    clickhouse local --path /tmp/chdb --queries-file docs/clickhouse-local-example.sql
