#!/usr/bin/env bash
# CLI 冒烟：单测 + 切回文案自检 +（可选）debug 二进制 + 本地 Vite
#   bash scripts/cli-smoke.sh                     # 只跑单测
#   EGRESS_CLI_SMOKE=1 bash scripts/cli-smoke.sh  # 再跑真实二进制（--mock，不切换节点）
# 注意：check current --mock 仍会经本机代理口做真实深测（含带宽抽样），会耗一定流量。
set -euo pipefail
cd "$(dirname "$0")/.."

echo "== vitest cli + runner =="
pnpm exec vitest run src/lib/cli src/lib/runner src/lib/tauriInvokeContract.test.ts

echo "== restore 文案自检 =="
node scripts/simulate-restore-fail.mjs

if [[ "${EGRESS_CLI_SMOKE:-}" != "1" ]]; then
  echo "SKIP live binary smoke（需要前端）。完整自测："
  echo "  EGRESS_CLI_SMOKE=1 bash scripts/cli-smoke.sh"
  exit 0
fi

BIN="${CLI_BIN:-src-tauri/target/debug/clash-assist}"
if [[ ! -x "$BIN" ]]; then
  echo "building debug binary…"
  (cd src-tauri && cargo build)
fi

# 只结束本仓库编译出的 CLI 进程（不碰 Vite、不碰代理软件）。
kill_cli() { pkill -f 'target/(debug|release)/clash-assist' 2>/dev/null || true; }

STARTED_VITE=0
cleanup() {
  kill_cli
  if [[ "$STARTED_VITE" == "1" && -f /tmp/egress-vite-cli.pid ]]; then
    kill "$(cat /tmp/egress-vite-cli.pid)" 2>/dev/null || true
    rm -f /tmp/egress-vite-cli.pid
  fi
}
trap cleanup EXIT

kill_cli
# debug 二进制从 Vite 加载前端
if ! curl -sf -o /dev/null http://127.0.0.1:1420/; then
  echo "starting vite…"
  pnpm dev > /tmp/egress-vite-cli.log 2>&1 &
  echo $! > /tmp/egress-vite-cli.pid
  STARTED_VITE=1
  for _ in $(seq 1 40); do
    curl -sf -o /dev/null http://127.0.0.1:1420/ && break
    sleep 1
  done
fi

run_to() { perl -e 'alarm shift; exec @ARGV' "$1" "${@:2}"; }

# expect_cli <期望退出码> <期望 stdout 片段> <超时秒> <参数…>
expect_cli() {
  local want_code="$1" want_text="$2" to="$3"
  shift 3
  local out code
  set +e
  out=$(run_to "$to" "$BIN" --cli "$@")
  code=$?
  set -e
  echo "\$ clash-assist --cli $* → exit ${code}"
  echo "$out" | head -c 600
  echo
  if [[ "$code" != "$want_code" ]]; then
    echo "FAIL: 期望退出码 ${want_code}，实际 ${code}" >&2
    exit 1
  fi
  if ! grep -qF -- "$want_text" <<<"$out"; then
    echo "FAIL: 输出里没有：${want_text}" >&2
    exit 1
  fi
}

echo "== help =="
expect_cli 0 "Clash Assist CLI 0.1.16" 60 help --no-json

echo "== 未知命令 / 未知选项 / 非法 client → ok:false + exit 1 =="
expect_cli 1 '"code":"unknown_command"' 60 frobnicate
expect_cli 1 '"code":"unknown_option"' 60 discover --mock --verbose
expect_cli 1 '"code":"client_unsupported"' 60 discover --mock -c flclash
expect_cli 1 '"code":"client_required"' 60 gate

echo "== discover --mock =="
expect_cli 0 '"ok":true' 60 discover --mock --json

echo "== discover --mock --no-json（人类摘要）=="
expect_cli 0 "✓ discover 完成" 60 discover --mock --no-json

echo "== gate --mock =="
expect_cli 0 '"ok":true' 90 gate --mock --json

echo "== check current --mock =="
expect_cli 0 '"target":"current"' 180 check current --mock --json


echo "== serve --mock（/health + POST /v1/discover，再停）=="
# 后台起 serve；从 stdout 就绪 JSON 取 token/port
SERVE_LOG=/tmp/egress-cli-serve.log
rm -f "$SERVE_LOG"
run_to 120 "$BIN" --cli serve --mock --port 0 >"$SERVE_LOG" 2>/tmp/egress-cli-serve.err &
SERVE_PID=$!
TOKEN="" PORT=""
for _ in $(seq 1 60); do
  if grep -q '"command":"serve"' "$SERVE_LOG" 2>/dev/null; then
    # 取最后一行 JSON
    line=$(grep '"command":"serve"' "$SERVE_LOG" | tail -1)
    TOKEN=$(perl -ne 'print $1 if /"token"\s*:\s*"([^"]+)"/' <<<"$line")
    PORT=$(perl -ne 'print $1 if /"port"\s*:\s*(\d+)/' <<<"$line")
    break
  fi
  if ! kill -0 "$SERVE_PID" 2>/dev/null; then
    echo "FAIL: serve 进程已退出" >&2
    cat /tmp/egress-cli-serve.err >&2 || true
    cat "$SERVE_LOG" >&2 || true
    exit 1
  fi
  sleep 1
done
if [[ -z "$TOKEN" || -z "$PORT" ]]; then
  echo "FAIL: 未读到 serve 就绪 token/port" >&2
  cat /tmp/egress-cli-serve.err >&2 || true
  cat "$SERVE_LOG" >&2 || true
  kill "$SERVE_PID" 2>/dev/null || true
  exit 1
fi
echo "serve ready port=$PORT"
code=$(curl -s --max-time 60 -o /tmp/egress-health.json -w "%{http_code}" -H "Authorization: Bearer $TOKEN" "http://127.0.0.1:${PORT}/health")
[[ "$code" == "200" ]] || { echo "FAIL: /health HTTP $code"; cat /tmp/egress-health.json; kill "$SERVE_PID"; exit 1; }
grep -q '"ok":true' /tmp/egress-health.json || { echo "FAIL: /health body"; cat /tmp/egress-health.json; kill "$SERVE_PID"; exit 1; }
# 无 token → 401
code401=$(curl -s --max-time 10 -o /dev/null -w "%{http_code}" "http://127.0.0.1:${PORT}/health")
[[ "$code401" == "401" ]] || { echo "FAIL: 期望 401，得到 $code401"; kill "$SERVE_PID"; exit 1; }
code=$(curl -s --max-time 60 -o /tmp/egress-discover.json -w "%{http_code}" -H "X-Egress-Token: $TOKEN" -X POST "http://127.0.0.1:${PORT}/v1/discover")
[[ "$code" == "200" ]] || { echo "FAIL: /v1/discover HTTP $code"; cat /tmp/egress-discover.json; kill "$SERVE_PID"; exit 1; }
grep -q '"command":"discover"' /tmp/egress-discover.json || { echo "FAIL: discover envelope"; cat /tmp/egress-discover.json; kill "$SERVE_PID"; exit 1; }
grep -q '"ok":true' /tmp/egress-discover.json || { echo "FAIL: discover 非 ok"; cat /tmp/egress-discover.json; kill "$SERVE_PID"; exit 1; }
echo "POST /v1/discover → 200 $(head -c 200 /tmp/egress-discover.json)"
# 第二个请求：确认串行通道回包后能继续处理（回包坏掉时这里会超时）
code=$(curl -s --max-time 60 -o /tmp/egress-discover2.json -w "%{http_code}" -H "Authorization: Bearer $TOKEN" -X POST "http://127.0.0.1:${PORT}/v1/discover")
[[ "$code" == "200" ]] || { echo "FAIL: 第二次 /v1/discover HTTP $code"; cat /tmp/egress-discover2.json; kill "$SERVE_PID"; exit 1; }
# 错 token → 401
code401=$(curl -s --max-time 10 -o /dev/null -w "%{http_code}" -H "Authorization: Bearer wrong" -X POST "http://127.0.0.1:${PORT}/v1/discover")
[[ "$code401" == "401" ]] || { echo "FAIL: 错 token 期望 401，得到 $code401"; kill "$SERVE_PID"; exit 1; }
kill -INT "$SERVE_PID" 2>/dev/null || true
for _ in $(seq 1 20); do
  kill -0 "$SERVE_PID" 2>/dev/null || break
  sleep 1
done
kill -9 "$SERVE_PID" 2>/dev/null || true
wait "$SERVE_PID" 2>/dev/null || true
echo "serve smoke OK"

echo "OK cli smoke"
