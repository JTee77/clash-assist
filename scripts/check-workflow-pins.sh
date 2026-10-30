#!/usr/bin/env bash
# 供应链检查（windows-plan r4 §2.8 / §3，S-8 ②④）：
#   1. .github/workflows 与 .github/actions 下所有 `uses:`（step 级 action、job 级 reusable
#      workflow、docker://）都必须按不可变引用固定：
#        - owner/repo[/path]@<40 位十六进制 commit SHA>
#        - docker://image@sha256:<64 位十六进制 digest>
#      本地 action / workflow（`./` 开头）除外。
#   2. 禁止 `pull_request_target` 与 `workflow_run` 触发器。
#
# 用法：bash scripts/check-workflow-pins.sh [目录，默认 .github]
# 退出码：0 = 通过；1 = 发现违规；2 = 用法错误或目录不存在。
set -euo pipefail

root="${1:-.github}"
if [ ! -d "$root" ]; then
  echo "check-workflow-pins: 目录不存在：$root" >&2
  exit 2
fi

files=()
while IFS= read -r -d '' f; do
  files+=("$f")
done < <(find "$root" -type f \( -name '*.yml' -o -name '*.yaml' \) -print0 | sort -z)

if [ "${#files[@]}" -eq 0 ]; then
  echo "check-workflow-pins: $root 下没有 YAML 文件" >&2
  exit 2
fi

fail=0
checked=0

for f in "${files[@]}"; do
  lineno=0
  while IFS= read -r line || [ -n "$line" ]; do
    lineno=$((lineno + 1))
    # 去掉注释：整行注释，以及“空白 + #”开始的行尾注释（引号内的 # 在 uses 值中不会出现）。
    code="${line%%[[:space:]]#*}"
    trimmed="${code#"${code%%[![:space:]]*}"}"
    [[ "$trimmed" == '#'* ]] && code=""
    [ -n "$code" ] || continue

    if [[ "$code" =~ (^|[^A-Za-z0-9_-])(pull_request_target|workflow_run)([^A-Za-z0-9_-]|$) ]]; then
      echo "$f:$lineno: 禁止的触发器 ${BASH_REMATCH[2]}：$line"
      fail=1
    fi

    if [[ "$code" =~ (^|[[:space:]{,-])uses:[[:space:]]*(.*)$ ]]; then
      ref="${BASH_REMATCH[2]}"
      # 去掉 flow 映射里的后续字段、首尾引号与空白。
      ref="${ref%%,*}"
      ref="${ref%%\}*}"
      ref="${ref#\"}"; ref="${ref%\"}"
      ref="${ref#\'}"; ref="${ref%\'}"
      ref="${ref%"${ref##*[![:space:]]}"}"
      ref="${ref%\"}"; ref="${ref%\'}"
      checked=$((checked + 1))

      if [ -z "$ref" ]; then
        echo "$f:$lineno: uses 为空：$line"
        fail=1
      elif [[ "$ref" == ./* ]]; then
        : # 本地 action / reusable workflow，跟随本仓库提交
      elif [[ "$ref" == docker://* ]]; then
        if [[ ! "$ref" =~ ^docker://[^@[:space:]]+@sha256:[0-9a-f]{64}$ ]]; then
          echo "$f:$lineno: docker 镜像未按 sha256 digest 固定：$ref"
          fail=1
        fi
      elif [[ ! "$ref" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_./-]+@[0-9a-f]{40}$ ]]; then
        echo "$f:$lineno: 未按 40 位 commit SHA 固定：$ref"
        fail=1
      fi
    fi
  done < "$f"
done

if [ "$fail" -ne 0 ]; then
  echo "check-workflow-pins: 失败（见上方列表）" >&2
  exit 1
fi
echo "check-workflow-pins: 通过（${#files[@]} 个文件，${checked} 处 uses）"
