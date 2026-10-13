#!/usr/bin/env bash
# Tauri 2's DMG bundler has no icon-size option (create-dmg default 128).
# Post-process the built DMG so Finder shows icons at ICON_SIZE (default 96), matching
# src-tauri/dmg background layout (window 540x360, app (140,92), Applications (400,92)).
# Usage: scripts/dmg-set-icon-size.sh [path/to.dmg] [icon_size]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DMG="${1:-$(ls -t "$ROOT"/src-tauri/target/release/bundle/dmg/*.dmg | head -1)}"
SIZE="${2:-96}"
[[ -f "$DMG" ]] || { echo "no dmg: $DMG" >&2; exit 1; }
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
hdiutil convert "$DMG" -format UDRW -o "$TMP/rw.dmg" -quiet
MNT="$(hdiutil attach "$TMP/rw.dmg" -readwrite -noverify -noautoopen | awk -F'\t' '/Apple_HFS/{print $NF}')"
VOL="$(basename "$MNT")"
osascript <<OSA
tell application "Finder"
  tell disk "$VOL"
    open
    set opts to the icon view options of container window
    set icon size of opts to $SIZE
    set text size of opts to 13
    update without registering applications
    delay 2
    close
  end tell
end tell
OSA
sync
hdiutil detach "$MNT" -quiet || { sleep 3; hdiutil detach "$MNT" -force -quiet; }
rm -f "$DMG"
hdiutil convert "$TMP/rw.dmg" -format UDZO -imagekey zlib-level=9 -o "$DMG" -quiet
echo "icon size $SIZE applied: $DMG"
