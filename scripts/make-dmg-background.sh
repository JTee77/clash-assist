#!/usr/bin/env bash
# Rebuild src-tauri/dmg/background.tiff (multi-resolution 1x+2x) from the editable PNG sources.
# Sources (replace these to restyle; keep sizes): 
#   src-tauri/dmg/dmg-background.png      540x360  (1x, 72 dpi)
#   src-tauri/dmg/dmg-background@2x.png  1080x720  (2x, 144 dpi)
# Layout must match tauri.conf.json bundle.macOS.dmg: window 540x360, app icon centre (140,135),
# Applications centre (400,135), icon size 96.
set -euo pipefail
cd "$(dirname "$0")/../src-tauri/dmg"
tiffutil -cathidpicheck dmg-background.png dmg-background@2x.png -out background.tiff
echo "wrote src-tauri/dmg/background.tiff"
