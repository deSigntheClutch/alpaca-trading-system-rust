#!/bin/sh
# Container entrypoint: start the dashboard, and if Alpaca keys are present, refresh
# the morning report in the background every REFRESH_SECS (default 6h). Same-day
# reruns hit the LLM cache, so the notes cost one round of calls per day. Bars are
# regenerable, so an empty data/ after a restart just means a longer first backfill.
set -u

refresh() {
  while :; do
    backfill --universe config/universe.csv --start "${BACKFILL_START:-2024-01-01}" \
      && morning \
      || echo "serve.sh: morning refresh failed; retrying next round" >&2
    sleep "${REFRESH_SECS:-21600}"
  done
}

mkdir -p data
if [ -n "${ALPACA_KEY:-}" ] && [ -n "${ALPACA_SECRET:-}" ]; then
  refresh &
else
  echo "serve.sh: ALPACA_KEY/ALPACA_SECRET unset; morning report stays empty" >&2
fi

exec web
