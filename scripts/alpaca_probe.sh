#!/usr/bin/env bash
# Probe what the Alpaca account's data plan actually allows (#52).
# Answers four questions with real responses, not documentation:
#   1. Is feed=sip permitted for historical bars, and does the 15-minute rule apply?
#   2. How far back does daily history go?
#   3. How much volume does the free IEX feed carry vs SIP on the same day?
#   4. Do multi-symbol requests work in one call?
# Usage: ./scripts/alpaca_probe.sh            (reads ALPACA_KEY/SECRET from .env)
set -euo pipefail
cd "$(dirname "$0")/.."
set -a; [ -f .env ] && . ./.env; set +a
: "${ALPACA_KEY:?set ALPACA_KEY in .env}"; : "${ALPACA_SECRET:?set ALPACA_SECRET in .env}"
BASE="${ALPACA_DATA_BASE:-https://data.alpaca.markets}"
TRADE_BASE="${ALPACA_BASE:-https://paper-api.alpaca.markets}"
SYM="${1:-AAPL}"

get() { # get <url>  -> prints body then HTTP code
  curl -s -w '\n%{http_code}' -H "APCA-API-KEY-ID: $ALPACA_KEY" -H "APCA-API-SECRET-KEY: $ALPACA_SECRET" "$1"
}
show() { # show <label> <url>
  local out code body; out=$(get "$2"); code=${out##*$'\n'}; body=${out%$'\n'*}
  printf '\n== %s\n   GET %s\n   HTTP %s\n' "$1" "$2" "$code"
  echo "$body" | jq -c 'if .bars then (.bars | to_entries | map({symbol: .key, n: (.value|length), first: .value[0].t, last: .value[-1].t, vol_last: .value[-1].v})) else . end' 2>/dev/null || echo "$body" | head -c 300
}

# yesterday and a definitely-closed day, UTC
Y=$(date -u -v-1d +%Y-%m-%d 2>/dev/null || date -u -d 'yesterday' +%Y-%m-%d)
END_OLD="${Y}T23:59:59Z"

show "1a. SIP, end = yesterday (should be allowed on every plan)" \
  "$BASE/v2/stocks/bars?symbols=$SYM&timeframe=1Day&start=2024-01-01&end=$END_OLD&feed=sip&adjustment=all&limit=3&sort=desc"
show "1b. SIP, no end (includes last 15 min; free plan expects 403/422 here during market hours)" \
  "$BASE/v2/stocks/bars?symbols=$SYM&timeframe=1Day&start=2024-01-01&feed=sip&adjustment=all&limit=3&sort=desc"
show "1c. no feed param (what the code sends today; shows the plan default)" \
  "$BASE/v2/stocks/bars?symbols=$SYM&timeframe=1Day&start=2024-01-01&adjustment=all&limit=3&sort=desc"

show "2. earliest daily bar available (depth)" \
  "$BASE/v2/stocks/bars?symbols=$SYM&timeframe=1Day&start=1990-01-01&end=$END_OLD&feed=sip&adjustment=all&limit=1&sort=asc"

show "3a. IEX volume, one closed day" \
  "$BASE/v2/stocks/bars?symbols=$SYM&timeframe=1Day&start=$Y&end=$END_OLD&feed=iex&limit=1"
show "3b. SIP volume, same day (compare vol_last)" \
  "$BASE/v2/stocks/bars?symbols=$SYM&timeframe=1Day&start=$Y&end=$END_OLD&feed=sip&limit=1"

show "4. three symbols in one request" \
  "$BASE/v2/stocks/bars?symbols=NVDA,MU,AMD&timeframe=1Day&start=2024-01-01&end=$END_OLD&feed=sip&adjustment=all&limit=2&sort=desc"

show "5. trading calendar sample (trading API host, not the data host)" \
  "$TRADE_BASE/v2/calendar?start=2026-09-01&end=2026-09-10"
