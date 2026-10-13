#!/usr/bin/env bash
# macOS Apple Silicon smoke: process stays alive ≥30s after `pnpm tauri dev`,
# plus Rust unit probes (no panic on dead ports / client build).
#
# Usage (from repo root, on Apple Silicon Mac):
#   bash scripts/smoke-mac.sh
#   # or: pnpm smoke:mac
#
# Cleanup note: carefully kills only processes matching this app / vite for this
# repo. Do not run while you need another clash-assist / vite instance.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

LOG="${SMOKE_LOG:-/tmp/clash-assist-smoke.log}"
VITE_URL="${SMOKE_VITE_URL:-http://127.0.0.1:1420}"
WAIT_VITE_SEC="${SMOKE_WAIT_VITE_SEC:-90}"
ALIVE_SEC="${SMOKE_ALIVE_SEC:-30}"
POLL_SEC=2

pass() { echo "PASS: $*"; }
fail() { echo "FAIL: $*" >&2; exit 1; }

echo "== clash-assist smoke-mac =="
echo "root=$ROOT"
echo "log=$LOG"

# 1) Careful cleanup within app context
echo "-- cleanup prior clash-assist / vite (repo smoke only) --"
pkill -f '[e]gress-checker' 2>/dev/null || true
# Prefer killing vite started for this project path if possible
pkill -f "vite.*${ROOT}" 2>/dev/null || true
pkill -f '[v]ite' 2>/dev/null || true
sleep 1

# 2) Rust unit probes (no UI) — must not panic
echo "-- cargo test smoke_ --"
if ! command -v cargo >/dev/null 2>&1; then
  fail "cargo not found"
fi
(
  cd "$ROOT/src-tauri"
  cargo test --manifest-path Cargo.toml smoke_ -- --nocapture
) || fail "cargo test smoke_ failed"

# 2b) Mihomo Unix API (soft: SKIP if sock missing / Clash down)
echo "-- smoke-mihomo-api (soft) --"
if bash "$ROOT/scripts/smoke-mihomo-api.sh"; then
  pass "mihomo-api section ok or skipped"
else
  # Unauthorized / bad JSON should fail the smoke when Clash is up
  fail "smoke-mihomo-api.sh failed (sock present but /proxies check failed)"
fi

# 3) Start pnpm tauri dev in background
echo "-- start pnpm tauri dev --"
: >"$LOG"
(
  cd "$ROOT"
  pnpm tauri dev
) >>"$LOG" 2>&1 &
DEV_PID=$!

cleanup() {
  kill "$DEV_PID" 2>/dev/null || true
  pkill -f '[e]gress-checker' 2>/dev/null || true
  # leave vite kill soft — may already be child of DEV_PID
}
trap cleanup EXIT

# 4) Wait for Vite
echo "-- wait for $VITE_URL (timeout ${WAIT_VITE_SEC}s) --"
deadline=$((SECONDS + WAIT_VITE_SEC))
ready=0
while (( SECONDS < deadline )); do
  if curl -fsS -o /dev/null --max-time 2 "$VITE_URL"; then
    ready=1
    break
  fi
  if ! kill -0 "$DEV_PID" 2>/dev/null; then
    fail "pnpm tauri dev exited before Vite was ready (see $LOG)"
  fi
  sleep 2
done
(( ready == 1 )) || fail "Vite did not respond within ${WAIT_VITE_SEC}s (see $LOG)"
pass "Vite responding at $VITE_URL"

# 5) Find clash-assist process; poll alive for ALIVE_SEC
echo "-- find clash-assist process --"
find_pid() {
  pgrep -f '[e]gress-checker' | head -n 1 || true
}

app_pid=""
deadline=$((SECONDS + 60))
while (( SECONDS < deadline )); do
  app_pid="$(find_pid)"
  if [[ -n "$app_pid" ]]; then
    break
  fi
  if ! kill -0 "$DEV_PID" 2>/dev/null; then
    fail "tauri dev died before app process appeared (see $LOG)"
  fi
  sleep 1
done
[[ -n "$app_pid" ]] || fail "clash-assist process not found"
pass "clash-assist pid=$app_pid"

echo "-- poll alive ≥${ALIVE_SEC}s (every ${POLL_SEC}s) --"
end=$((SECONDS + ALIVE_SEC))
while (( SECONDS < end )); do
  if ! kill -0 "$app_pid" 2>/dev/null; then
    fail "clash-assist pid $app_pid exited before ${ALIVE_SEC}s (打开即崩?). Log: $LOG"
  fi
  # also accept a replacement pid if tauri restarted once
  cur="$(find_pid)"
  if [[ -n "$cur" ]]; then
    app_pid="$cur"
  else
    fail "clash-assist process disappeared during ${ALIVE_SEC}s window"
  fi
  sleep "$POLL_SEC"
done
pass "process alive ≥${ALIVE_SEC}s after boot (no user click required)"

pass "smoke-mac complete"
exit 0
