#!/usr/bin/env bash
# Exercises sign-macos.sh's stapling of a disk image against fake Apple tools: a ticket Apple does
# not serve yet is asked for again, and one it never serves fails the build. Signs nothing.
set -euo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
test_dir=$(mktemp -d)
trap 'rm -rf "$test_dir"' EXIT
mkdir -p "$test_dir/bin"
fake() { printf '#!/bin/sh\n%s\n' "$2" > "$test_dir/bin/$1"; chmod +x "$test_dir/bin/$1"; }
fake security 'echo "1) ABC \"Developer ID Application: Test (TEAM)\""'
fake codesign 'exit 0'
fake spctl 'exit 0'
fake sleep 'exit 0'
# Notarization is accepted at once; stapling fails (exit 68) until it has been tried $STAPLE_AFTER times.
# shellcheck disable=SC2016 # expanded by the fake, when it runs
fake xcrun '
case $1 in
  notarytool) echo "{\"id\": \"submission\", \"status\": \"Accepted\"}" ;;
  stapler)
    n=$(($(cat "$TEST_DIR/stapled" 2>/dev/null || echo 0) + 1)); echo $n > "$TEST_DIR/stapled"
    [ "$n" -ge "$STAPLE_AFTER" ] || exit 68 ;;
esac'
touch "$test_dir/Chauffeur.dmg" "$test_dir/key.p8"
sign_dmg() {
  rm -f "$test_dir/stapled"
  env PATH="$test_dir/bin:$PATH" TEST_DIR="$test_dir" STAPLE_AFTER="$1" \
    CHAUFFEUR_CODESIGN_IDENTITY="Developer ID Application: Test (TEAM)" \
    CHAUFFEUR_NOTARY_KEY="$test_dir/key.p8" CHAUFFEUR_NOTARY_KEY_ID=key \
    bash "$repo/scripts/sign-macos.sh" dmg "$test_dir/Chauffeur.dmg" > "$test_dir/output" 2>&1
}

sign_dmg 3 || { cat "$test_dir/output"; echo "A ticket served at the third attempt should be stapled." >&2; exit 1; }
[ "$(cat "$test_dir/stapled")" = 3 ] || { echo "Stapled $(cat "$test_dir/stapled") times, expected 3." >&2; exit 1; }

if sign_dmg 99; then echo "A ticket Apple never serves should fail the build." >&2; exit 1; fi
[ "$(cat "$test_dir/stapled")" = 5 ] || { echo "Stapled $(cat "$test_dir/stapled") times, expected 5." >&2; exit 1; }
grep -q "could not staple Apple's ticket to Chauffeur.dmg after 5 attempts" "$test_dir/output" ||
  { cat "$test_dir/output"; echo "The failure should say stapling gave up." >&2; exit 1; }

echo 'Stapling OK (a late ticket is stapled on a later attempt; a missing one fails after 5)'
