#!/usr/bin/env bash
# Keep the worker and standalone extension builders on the same verified cross-linker.
set -euo pipefail
: "${RUNNER_TEMP:?Run this installer on a GitHub Actions runner}"
: "${GITHUB_PATH:?Run this installer on a GitHub Actions runner}"
zig="zig-x86_64-linux-0.17.0"
checksum=1cbe9df9f27e6b78d14ccbca43b6703a404ef79ef1c463de901d7f088d4e2026
curl -fsSL -o "$RUNNER_TEMP/$zig.tar.xz" "https://ziglang.org/download/0.17.0/$zig.tar.xz"
echo "$checksum  $RUNNER_TEMP/$zig.tar.xz" | sha256sum -c -
tar xJf "$RUNNER_TEMP/$zig.tar.xz" -C "$RUNNER_TEMP"
echo "$RUNNER_TEMP/$zig" >> "$GITHUB_PATH"
cargo install --locked cargo-zigbuild --version 0.24.1
"$RUNNER_TEMP/$zig/zig" version
cargo-zigbuild --version
