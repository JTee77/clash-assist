#!/usr/bin/env bash
# CLI smoke: Mihomo / Clash Verge Rev controller via Unix socket (no GUI).
# Soft-skip when sock missing or Clash not running — does not fail the whole smoke.
#
# Usage:
#   bash scripts/smoke-mihomo-api.sh
#   MIHOMO_SECRET=… bash scripts/smoke-mihomo-api.sh
#
# Secret resolution: $MIHOMO_SECRET, else parse Verge config.yaml (same regex as app).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERGE_CFG="${MIHOMO_VERGE_CFG:-$HOME/Library/Application Support/io.github.clash-verge-rev.clash-verge-rev/config.yaml}"
OUT="${MIHOMO_PROXIES_OUT:-/tmp/clash-assist-proxies-smoke.json}"

pass() { echo "PASS: $*"; }
skip() { echo "SKIP: $*"; exit 0; }
fail() { echo "FAIL: $*" >&2; exit 1; }

# Resolve Verge sock: env override, else service (2.5.6+) → legacy → $TMPDIR.
resolve_sock() {
  if [[ -n "${MIHOMO_SOCK:-}" ]]; then
    echo "$MIHOMO_SOCK"
    return
  fi
  local uid
  uid="$(id -u 2>/dev/null || echo "")"
  local candidates=()
  if [[ -n "$uid" ]]; then
    candidates+=("/var/run/clash-verge-service/users/${uid}/verge-mihomo.sock")
  fi
  if [[ -d /var/run/clash-verge-service/users ]]; then
    local d
    for d in /var/run/clash-verge-service/users/*/verge-mihomo.sock; do
      [[ -e "$d" ]] && candidates+=("$d")
    done
  fi
  candidates+=("/tmp/verge/verge-mihomo.sock")
  candidates+=("${TMPDIR:-/tmp}/verge-mihomo.sock")
  local c
  for c in "${candidates[@]}"; do
    if [[ -e "$c" ]]; then
      echo "$c"
      return
    fi
  done
  echo ""
}

SOCK="$(resolve_sock)"

echo "== clash-assist smoke-mihomo-api =="
echo "sock=${SOCK:-"(none)"}"

if [[ -z "$SOCK" || ! -e "$SOCK" ]]; then
  skip "unix socket missing (tried service/legacy/TMPDIR) — Clash Verge Rev not running or sock disabled"
fi

SECRET="${MIHOMO_SECRET:-}"
if [[ -z "$SECRET" && -f "$VERGE_CFG" ]]; then
  # App regex: (?m)^secret:\s*['"]?([^\s'"]+)['"]?
  SECRET="$(python3 - "$VERGE_CFG" <<'PY'
import re, sys
path = sys.argv[1]
try:
    t = open(path, encoding="utf-8", errors="ignore").read()
except OSError:
    sys.exit(0)
m = re.search(r"(?m)^secret:\s*['\"]?([^\s'\"]+)['\"]?", t)
if m:
    print(m.group(1), end="")
PY
)"
  echo "secret_source=verge-config len=${#SECRET}"
elif [[ -n "$SECRET" ]]; then
  echo "secret_source=env len=${#SECRET}"
else
  echo "secret_source=none (empty Bearer)"
fi

set +e
HTTP_CODE="$(
  curl -sS -m 8 -o "$OUT" -w '%{http_code}' \
    --unix-socket "$SOCK" \
    -H "Authorization: Bearer ${SECRET}" \
    -H "Content-Type: application/json" \
    "http://localhost/proxies" 2>/dev/null
)"
CURL_EC=$?
set -e

if [[ $CURL_EC -ne 0 || -z "$HTTP_CODE" || "$HTTP_CODE" == "000" ]]; then
  skip "curl failed talking to $SOCK (Clash may have exited); curl_ec=$CURL_EC"
fi

echo "proxies_http=$HTTP_CODE out=$OUT"

if [[ "$HTTP_CODE" == "401" || "$HTTP_CODE" == "403" ]]; then
  fail "unauthorized HTTP $HTTP_CODE — set MIHOMO_SECRET or fix Verge secret"
fi

if [[ "$HTTP_CODE" != "200" ]]; then
  fail "expected HTTP 200 from /proxies via unix, got $HTTP_CODE"
fi

python3 - "$OUT" <<PY || fail "proxies JSON invalid / demo-like / no leaves"
import json, sys
path = sys.argv[1]
with open(path, encoding="utf-8") as f:
    d = json.load(f)
prox = d.get("proxies")
if not isinstance(prox, dict) or len(prox) == 0:
    print("FAIL: proxies object empty or missing", file=sys.stderr)
    sys.exit(1)
DEMO = {
    "🇭🇰 香港 01 | Hysteria2",
    "🇯🇵 东京 Premium",
    "🇸🇬 Singapore IEPL",
    "🇺🇸 洛杉矶 家宽",
    "🇹🇼 台北 游戏专线",
    "🇩🇪 Frankfurt",
}
IGNORE = {"Selector", "URLTest", "Fallback", "Direct", "Reject", "Compatible", "Pass", "LoadBalance", "Relay"}
JUNK = ("剩余", "到期", "官网")
hits = [n for n in prox if n in DEMO]
if hits:
    print(f"FAIL: demo mock names present: {hits}", file=sys.stderr)
    sys.exit(1)
leaves = []
for name, p in prox.items():
    if not isinstance(p, dict):
        continue
    t = p.get("type") or ""
    if t in IGNORE:
        continue
    if name.startswith("PASS") or name.startswith("REJECT"):
        continue
    if any(k in name for k in JUNK):
        continue
    leaves.append(name)
if len(leaves) < 1:
    print("FAIL: leaf node count is 0 after filter", file=sys.stderr)
    sys.exit(1)
print(f"proxy_keys={len(prox)} leaf_nodes={len(leaves)} sample_leaves={leaves[:8]}")
PY

pass "unix /proxies HTTP 200; leaf_nodes>0; no demo mock names"

exit 0
