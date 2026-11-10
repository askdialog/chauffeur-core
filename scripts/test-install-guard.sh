#!/usr/bin/env bash
# Exercises installer preflight only; the fake compiler aborts before any install.
set -euo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
test_dir=$(mktemp -d)
trap 'rm -rf "$test_dir"' EXIT
mkdir -p "$test_dir/bin" "$test_dir/home"
printf '#!/bin/sh\nexit 42\n' > "$test_dir/bin/cargo"
chmod +x "$test_dir/bin/cargo"
check() {
  local expected=$1; shift
  local code=0
  env HOME="$test_dir/home" PATH="$test_dir/bin:$PATH" "$@" bash "$repo/scripts/install.sh" > "$test_dir/output" 2>&1 || code=$?
  if [[ "$code" != "$expected" ]]; then cat "$test_dir/output"; echo "Expected $expected, got $code" >&2; exit 1; fi
}
check 1 CHAUFFEUR_WORKSPACE_ID=test CHAUFFEUR_HOME="$test_dir/home/.chauffeur"
grep -q 'Refusing to replace' "$test_dir/output"
# Use a wrapper to pass the explicit installer flag after its script path.
env HOME="$test_dir/home" PATH="$test_dir/bin:$PATH" CHAUFFEUR_WORKSPACE_ID=test CHAUFFEUR_HOME="$test_dir/home/.chauffeur" bash "$repo/scripts/install.sh" --replace-shared > "$test_dir/output" 2>&1 && code=0 || code=$?
[[ "$code" == 42 ]]
check 42 CHAUFFEUR_WORKSPACE_ID=test CHAUFFEUR_HOME="$test_dir/isolated"
echo 'Installer guard OK (shared install refused; explicit and isolated installs allowed)'
