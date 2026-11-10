#!/usr/bin/env bash
# Opt-in proof with real native agent runtimes and local deterministic inference.
set -euo pipefail
cd "$(dirname "$0")/../sidecar"
exec bun scripts/session-portability/run.ts "$@"
