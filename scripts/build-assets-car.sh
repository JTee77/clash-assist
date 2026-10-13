#!/usr/bin/env bash
# Precompile src-tauri/icons/AppIcon.icon (Icon Composer / Liquid Glass) -> src-tauri/icons/Assets.car.
#
# Why: on Xcode 27 / macOS 27, actool hands work to a persistent `ibtoold` daemon. When tauri-bundler
# runs actool itself during `tauri build`, the daemon it ends up using is broken and every .icon
# compile fails with "Bad file descriptor" / "Failed to create a catalog from the Icon Studio
# document" ("Failed to create app Assets.car: failed to run actool"). Running actool from a plain
# shell works. So the .icon is NOT passed to Tauri; instead the committed Assets.car is shipped via
# bundle.macOS.files, and Info.plist sets CFBundleIconName=AppIcon. Re-run this script after editing
# AppIcon.icon and commit the resulting Assets.car.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ICON="$ROOT/src-tauri/icons/AppIcon.icon"
OUT="$ROOT/src-tauri/icons/Assets.car"
[[ "$(uname -s)" == "Darwin" ]] || { echo "macOS only" >&2; exit 1; }
for attempt in 1 2 3; do
  pkill -x ibtoold 2>/dev/null || true
  sleep 3
  TMP="$(mktemp -d)"
  cp -R "$ICON" "$TMP/AppIcon.icon"; mkdir "$TMP/out"
  xcrun actool "$TMP/AppIcon.icon" --compile "$TMP/out" --output-partial-info-plist "$TMP/out/p.plist" \
    --app-icon AppIcon --include-all-app-icons --enable-on-demand-resources NO --development-region en \
    --target-device mac --minimum-deployment-target 12.0 --platform macosx >"$TMP/log" 2>&1 </dev/null || true
  if [[ -s "$TMP/out/Assets.car" ]]; then
    cp "$TMP/out/Assets.car" "$OUT"; rm -rf "$TMP"
    echo "wrote $OUT (attempt $attempt)"; exit 0
  fi
  echo "actool attempt $attempt failed:" >&2; grep 'error:' "$TMP/log" >&2 || true
  rm -rf "$TMP"
done
echo "error: actool could not compile AppIcon.icon" >&2; exit 1
