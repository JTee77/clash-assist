#!/usr/bin/env bash
# check-workflow-pins 自测：
#   - cases/good*.yml 必须通过（退出码 0）；
#   - cases/bad-*.yml 必须以退出码 1 失败，且输出包含用例中每一行 `# expect: <文本>`。
# 本地引用按迷你仓库 tests/workflow-pins/root 解析；该目录会先复制到临时目录，
# 再创建符号链接 .github/actions/comp -> ../../tools/comp（r2 n12 反例）。
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fx="$here/tests/workflow-pins"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cp -R "$fx/root" "$tmp/root"
ln -s ../../tools/comp "$tmp/root/.github/actions/comp"
out="$tmp/out"
fail=0
run() { bash "$here/check-workflow-pins.sh" --repo-root "$tmp/root" "$1" >"$out" 2>&1; }

for f in "$fx"/cases/good*.yml; do
  name="$(basename "$f")"
  if run "$f"; then echo "ok   $name 通过"; else echo "FAIL $name 应通过："; cat "$out"; fail=1; fi
done

for f in "$fx"/cases/bad-*.yml; do
  name="$(basename "$f")"
  run "$f"; rc=$?
  if [ "$rc" -ne 1 ]; then
    echo "FAIL $name 退出码 $rc（应为 1）："; cat "$out"; fail=1; continue
  fi
  missing=0
  while IFS= read -r want; do
    if ! grep -qF -- "$want" "$out"; then
      echo "FAIL $name 输出缺少：$want"; missing=1
    fi
  done < <(sed -n 's/^# expect: //p' "$f")
  if [ "$missing" -eq 0 ]; then echo "ok   $name 被拒绝"; else cat "$out"; fail=1; fi
done

if [ "$fail" -eq 0 ]; then echo "test-check-workflow-pins: 全部通过"; else echo "test-check-workflow-pins: 失败"; fi
exit "$fail"
