#!/usr/bin/env bash
# 薄包装：实际检查在 scripts/check-workflow-pins.py（用 YAML 解析器，见该文件说明）。
# 用法：bash scripts/check-workflow-pins.sh [--repo-root DIR] [路径 ...]（默认 .github）
# 退出码：0 = 通过；1 = 发现违规；2 = 用法错误 / 无法解析 / 缺少 PyYAML。
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec python3 "$here/check-workflow-pins.py" "$@"
