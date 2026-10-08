#!/usr/bin/env bash
# Load every data/bars/1Day/*.jsonl into ClickHouse over HTTP, then recompute
# features and outcomes. Works against local docker-compose or the Fly app.
#
#   CH_URL=http://localhost:8123 CH_PASSWORD=dev ./scripts/ch-load-bars.sh
#   CH_URL=https://alpaca-clickhouse.fly.dev CH_PASSWORD=$PW ./scripts/ch-load-bars.sh
set -euo pipefail
CH_URL=${CH_URL:-http://localhost:8123}
CH_USER=${CH_USER:-alpaca}
: "${CH_PASSWORD:?set CH_PASSWORD}"
BARS=${BARS:-data/bars/1Day}

q() { curl -sS -u "$CH_USER:$CH_PASSWORD" "$CH_URL/" --data-binary @- ; }

echo "schema"
q < db/clickhouse/schema.sql

n=0
for f in "$BARS"/*.jsonl; do
  sym=$(basename "$f" .jsonl)
  # ts in the files is epoch millis; the table wants DateTime (seconds)
  curl -sS -u "$CH_USER:$CH_PASSWORD" \
    "$CH_URL/?query=INSERT%20INTO%20alpaca.bars%20(symbol,ts,open,high,low,close,volume)%20SELECT%20symbol,toDateTime(intDiv(ts,1000)),open,high,low,close,volume%20FROM%20input('symbol%20String,ts%20Int64,open%20Float64,high%20Float64,low%20Float64,close%20Float64,volume%20Float64')%20FORMAT%20JSONEachRow" \
    --data-binary @"$f"
  n=$((n+1)); printf '\r%4d %s   ' "$n" "$sym"
done
echo
echo "features + outcomes"
q < db/clickhouse/queries/compute_features_outcomes.sql
echo "rows:"
echo "SELECT 'bars',count() FROM alpaca.bars UNION ALL SELECT 'features',count() FROM alpaca.features UNION ALL SELECT 'outcomes',count() FROM alpaca.outcomes FORMAT PrettyCompact" | q
