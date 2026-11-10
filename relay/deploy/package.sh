#!/usr/bin/env bash
# Package the runnable relay alone; no host, credentials or repository at runtime.
set -euo pipefail
archive=${1:?Usage: bash deploy/package.sh /path/to/relay.tar.gz}
relay_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
bun build "$relay_dir/src/main.ts" --target=bun --outfile "$stage/main.ts"
# macOS otherwise adds an AppleDouble file that the strict Linux rollout rejects.
COPYFILE_DISABLE=1 tar -czf "$archive" -C "$stage" main.ts
