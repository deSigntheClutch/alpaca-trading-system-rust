#!/bin/sh
# Container entrypoint: start the dashboard, and if Alpaca keys are present, refresh
# bars and the morning report once at boot, then every weekday at REFRESH_UTC
# (default 12:00 UTC: 8am New York in summer, 7am in winter, before the open).
# data/ is a Fly volume, so backfill is incremental: the first boot pulls history,
# later ones only the days since the last run. Same-day reruns of `morning` hit the
# LLM cache, so a reboot does not pay for the notes twice.
set -u

refresh_once() {
  backfill --universe config/universe.csv --start "${BACKFILL_START:-2024-01-01}" \
    && morning \
    || echo "serve.sh: refresh failed; next attempt at the next scheduled run" >&2
}

# Seconds until the next REFRESH_UTC that falls on Monday to Friday.
seconds_to_next_run() {
  now=$(date -u +%s)
  next=$(date -u -d "today ${REFRESH_UTC:-12:00}" +%s)
  [ "$next" -le "$now" ] && next=$((next + 86400))
  while [ "$(date -u -d "@$next" +%u)" -gt 5 ]; do next=$((next + 86400)); done
  echo $((next - now))
}

refresh_loop() {
  refresh_once
  while :; do
    sleep "$(seconds_to_next_run)"
    refresh_once
  done
}

mkdir -p data
if [ -n "${ALPACA_KEY:-}" ] && [ -n "${ALPACA_SECRET:-}" ]; then
  refresh_loop &
else
  echo "serve.sh: ALPACA_KEY/ALPACA_SECRET unset; no bars, no morning report" >&2
fi

exec web
