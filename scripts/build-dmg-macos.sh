#!/usr/bin/env bash
# Build ad-hoc signed (not notarized) arm64 .app / .dmg for Apple Silicon Macs.
# Must run on macOS (Apple Silicon). Linux CI cannot produce a real DMG.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: this script must run on macOS (Apple Silicon)." >&2
  echo "On Linux you can still run: pnpm typecheck && pnpm build (frontend only)." >&2
  exit 1
fi

ARCH="$(uname -m)"
if [[ "$ARCH" != "arm64" ]]; then
  echo "warning: host arch is $ARCH; product targets Apple Silicon (arm64) only." >&2
fi

command -v pnpm >/dev/null || { echo "pnpm required"; exit 1; }
command -v rustc >/dev/null || { echo "rustc/cargo required"; exit 1; }

pnpm install
pnpm tauri build

echo
echo "Artifacts (typical):"
echo "  src-tauri/target/release/bundle/macos/Egress Checker.app"
echo "  src-tauri/target/release/bundle/dmg/*.dmg"
echo
APP="src-tauri/target/release/bundle/macos/Egress Checker.app"
codesign --verify --deep --strict "$APP" || { echo "error: ad-hoc signature invalid; users would see 已损坏" >&2; exit 1; }
echo "Ad-hoc signed, not notarized. 首次打开：点「完成」→ 系统设置 › 隐私与安全性 → 仍要打开 → 确认框再点「仍要打开」并按提示完成身份验证。"
echo "Apple Silicon (arm64) only."
