#!/usr/bin/env bash
# check-workflow-pins 自测：good.yml 必须通过，每个 bad-*.yml 必须以退出码 1 失败。
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fx="$here/tests/workflow-pins"
fail=0
run() { bash "$here/check-workflow-pins.sh" --repo-root "$here/.." "$1" >/tmp/cwp-selftest.out 2>&1; }
if run "$fx/good.yml"; then echo "ok   good.yml 通过"; else echo "FAIL good.yml 应通过："; cat /tmp/cwp-selftest.out; fail=1; fi
for f in "$fx"/bad-*.yml; do
  run "$f"; rc=$?
  if [ "$rc" -eq 1 ]; then echo "ok   $(basename "$f") 被拒绝"; else echo "FAIL $(basename "$f") 退出码 $rc（应为 1）："; cat /tmp/cwp-selftest.out; fail=1; fi
done
[ "$fail" -eq 0 ] && echo "test-check-workflow-pins: 全部通过" || echo "test-check-workflow-pins: 失败"
exit "$fail"
