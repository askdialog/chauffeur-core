#!/usr/bin/env bash
# Opens a built disk image and checks what a user gets from it: an app that is sealed, whose tools
# run and whose daemon starts and answers, in a throwaway CHAUFFEUR_HOME.
# Usage: scripts/smoke-dmg.sh [path to the .dmg] (default: the one scripts/package.sh builds).
set -euo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
source "$repo/scripts/release-channel.sh"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo/Cargo.toml" | head -1)
dmg=${1:-$repo/target/release/$CHAUFFEUR_DMG_PREFIX-$version.dmg}
[ -f "$dmg" ] || { echo "No disk image at $dmg: build it with scripts/package.sh." >&2; exit 1; }

work=$(mktemp -d)
work=$(cd "$work" && pwd -P)
mount="$work/mount"
mkdir -p "$mount"
attached=0
bin=
cleanup() {
  if [ -n "$bin" ]; then
    CHAUFFEUR_HOME="$work/home" CHAUFFEUR_WORKSPACES_DIR="$work/workspaces" CHAUFFEUR_REPOS_DIR="$work/repos" \
      "$bin/chauffeur" daemon stop >/dev/null 2>&1 || true
  fi
  if [ "$attached" = 1 ]; then hdiutil detach "$mount" -quiet -force >/dev/null 2>&1 || true; fi
  rm -rf "$work"
}
trap cleanup EXIT
fail() { echo "$1" >&2; exit 1; }

hdiutil attach "$dmg" -nobrowse -readonly -mountpoint "$mount" -quiet
attached=1

echo "== the disk image opens on Chauffeur's window, under Chauffeur's icon"
for file in .DS_Store .background.tiff .VolumeIcon.icns; do
  [ -f "$mount/$file" ] || fail "$file is missing from the disk image: scripts/package.sh lays it out with dmgbuild (scripts/dmg/)."
done
[ "$(readlink "$mount/Applications")" = /Applications ] || fail "The disk image has no link to /Applications."
# The custom-icon flag (0x0400 in the Finder flags, bytes 8-9 of FinderInfo) makes the Finder use
# .VolumeIcon.icns.
flags=$(xattr -px com.apple.FinderInfo "$mount" 2>/dev/null | tr -d ' \n' | cut -c17-18)
(( 0x${flags:-00} & 0x04 )) || fail "The disk image's volume lacks the custom-icon flag: the Finder shows a generic disk."

# Copied out as a user drags it into /Applications; ditto keeps the signature's attributes.
ditto "$mount/Chauffeur.app" "$work/Chauffeur.app"
hdiutil detach "$mount" -quiet
attached=0
app="$work/Chauffeur.app"
bin="$app/Contents/MacOS"

echo "== the app is sealed"
codesign --verify --strict "$app"

echo "== every tool is there"
for tool in chauffeur chauffeurd chauffeur-sidecar chauffeur-mobile-host chauffeur-app; do
  [ -x "$bin/$tool" ] || fail "$tool is missing from the app, or not executable."
done

echo "== the CLI is this version"
"$bin/chauffeur" --version | grep -q "$version" || fail "chauffeur --version does not say $version."

echo "== the daemon starts and answers"
# Run from a Chauffeur workspace, the CLI would refuse to start this throwaway daemon.
unset CHAUFFEUR_WORKSPACE_ID
export CHAUFFEUR_HOME="$work/home" CHAUFFEUR_WORKSPACES_DIR="$work/workspaces" CHAUFFEUR_REPOS_DIR="$work/repos"
export CHAUFFEUR_PORT_OVERRIDE=0 CHAUFFEURD="$bin/chauffeurd"
mkdir -p "$CHAUFFEUR_HOME"
printf 'linear_enabled = false\n[office]\nurl = ""\n' > "$CHAUFFEUR_HOME/settings.toml"
"$bin/chauffeur" daemon start >/dev/null
"$bin/chauffeur" ls >/dev/null

echo "The disk image works: Chauffeur $version"
