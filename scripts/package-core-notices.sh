#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
out=$1
mkdir -p "$out"
for path in LICENSE THIRD_PARTY_NOTICES.md sidecar/plugins/jev/LICENSE sidecar/plugins/jev/README.md sidecar/licenses/codex-LICENSE sidecar/licenses/codex-NOTICE; do
  mkdir -p "$out/$(dirname "$path")"
  cp "$path" "$out/$path"
  chmod 644 "$out/$path"
done
find "$out" -type d -exec chmod 755 {} +
