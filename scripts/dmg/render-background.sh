#!/usr/bin/env bash
# Renders background.svg to the disk image's background.png (660x400) and background@2x.png
# (Retina), with headless Chrome. Run it after editing the SVG and commit the two PNGs: the build
# reads them and needs no browser. CHROME names another Chrome or Chromium binary.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
chrome=${CHROME:-"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"}
[[ -x $chrome ]] || { echo "render-background.sh: no Chrome at $chrome: install Google Chrome or set CHROME." >&2; exit 1; }
profile=$(mktemp -d)
trap 'rm -rf "$profile"' EXIT
for scale in 1 2; do
  out=$here/background.png
  [[ $scale = 2 ]] && out=$here/background@2x.png
  "$chrome" --headless --disable-gpu --hide-scrollbars --user-data-dir="$profile" \
    --force-device-scale-factor=$scale --window-size=660,400 --screenshot="$out" \
    "file://$here/background.svg" 2>"$profile/log" || { cat "$profile/log" >&2; exit 1; }
  echo "$out ($(sips -g pixelWidth -g pixelHeight "$out" | awk '/pixelWidth/ {w=$2} /pixelHeight/ {printf "%sx%s", w, $2}'))"
done
