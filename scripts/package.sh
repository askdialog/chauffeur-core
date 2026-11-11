#!/usr/bin/env bash
# Builds target/release/Chauffeur-<version>.dmg for colleagues who do not build from source: the
# macOS app carrying the daemon, the CLI and the agent sidecar (Claude Code included). On first
# launch the app lifts the quarantine flag from its own files and links the CLI into ~/.local/bin.
#
# Signed ad hoc by default, so macOS asks once before opening it; with a Developer ID and an App
# Store Connect API key in the environment, signed and notarized instead (scripts/sign-macos.sh).
# The disk image opens on a window laid out by dmgbuild (scripts/dmg/): the app and a link to
# /Applications on a background, under Chauffeur's icon.
# Each disk image gets its own build id (version, commit, date): an updated app restarts a daemon
# of another build, whatever the version number says.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/release-channel.sh
DAEMON_PACKAGE=chauffeurd
DAEMON_BINARY=chauffeurd
if [[ -f distribution/build-env.sh ]]; then
  # shellcheck source=/dev/null
  source distribution/build-env.sh
fi
[ "$(uname)" = Darwin ] || { echo "package.sh builds the macOS disk image; on Linux, run scripts/install.sh --app" >&2; exit 1; }
scripts/sign-macos.sh check
# dmgbuild writes the disk image's window layout without driving the Finder. Installed at its pinned
# version, before the long build so that a missing Python stops it at once.
DMGBUILD=target/dmgbuild
if ! cmp -s scripts/dmg/requirements.txt "$DMGBUILD/requirements.txt" ||
  ! "$DMGBUILD/bin/python" -c 'import dmgbuild' 2>/dev/null; then
  PYTHON=
  for candidate in python3 python3.14 python3.13 python3.12 python3.11 python3.10; do
    if "$candidate" -c 'import sys; sys.exit(sys.version_info < (3, 10))' 2>/dev/null; then
      PYTHON=$candidate
      break
    fi
  done
  [ -n "$PYTHON" ] || { echo "package.sh: dmgbuild, which lays out the disk image, needs Python 3.10 or later: install one (brew install python) and build again." >&2; exit 1; }
  rm -rf "$DMGBUILD"
  "$PYTHON" -m venv "$DMGBUILD"
  "$DMGBUILD/bin/pip" install --quiet --disable-pip-version-check --require-hashes --no-deps -r scripts/dmg/requirements.txt ||
    { rm -rf "$DMGBUILD"; echo "package.sh: could not install dmgbuild from scripts/dmg/requirements.txt: check the network and build again." >&2; exit 1; }
  cp scripts/dmg/requirements.txt "$DMGBUILD/"
fi
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
APP=target/release/bundle/macos/Chauffeur.app
OUT=target/release/$CHAUFFEUR_DMG_PREFIX-$VERSION.dmg
COMMIT=$(git rev-parse --short HEAD)
# Compiled into the daemon and the app (chauffeurd::BUILD).
CHAUFFEUR_BUILD="$VERSION+$COMMIT$(git diff --quiet HEAD || echo .dirty).$(date +%Y%m%d%H%M%S)"
export CHAUFFEUR_BUILD
# Compiled in when set, by the environment or a distribution's build-env.sh: where the daemon
# sends the app's error reports (docs/adr/error-reports-to-sentry.md), and the GitHub repository
# (`owner/repo`) its updates are published to. Unset, the build reports no errors and has no
# updates.
CHAUFFEUR_SENTRY_DSN=${CHAUFFEUR_SENTRY_DSN:-}
CHAUFFEUR_RELEASE_REPO=${CHAUFFEUR_RELEASE_REPO:-}
[ -n "$CHAUFFEUR_SENTRY_DSN" ] || echo "note: no CHAUFFEUR_SENTRY_DSN: this build reports no errors" >&2
[ -n "$CHAUFFEUR_RELEASE_REPO" ] || echo "note: no CHAUFFEUR_RELEASE_REPO: this build looks for no updates" >&2
export CHAUFFEUR_SENTRY_DSN CHAUFFEUR_RELEASE_REPO

cargo build --release -p "$DAEMON_PACKAGE" -p chauffeur-cli
(cd sidecar && bun install --frozen-lockfile && bun run build)
(cd relay && bun install --frozen-lockfile && bun run build:host)
(cd packages/remote && bun install --frozen-lockfile)
(cd app && bun install --frozen-lockfile && bunx tauri build)

for f in target/release/chauffeur sidecar/dist/chauffeur-sidecar relay/dist/chauffeur-mobile-host; do
  cp "$f" "$APP/Contents/MacOS/"
done
cp "target/release/$DAEMON_BINARY" "$APP/Contents/MacOS/chauffeurd"
scripts/package-core-notices.sh "$APP/Contents/Resources/licenses"
scripts/sign-macos.sh app "$APP"

rm -f "$OUT"
"$DMGBUILD/bin/dmgbuild" -s scripts/dmg/settings.py -D app="$APP" -D icon=app/src-tauri/icons/icon.icns \
  -D background=scripts/dmg/background.png "Chauffeur $VERSION" "$OUT"
scripts/sign-macos.sh dmg "$OUT"
# Last, since stapling changes the image. The updater checks the download against it, and an ad-hoc
# signature proves nothing about who built the image: a user checks theirs with
# `shasum -a 256 -c Chauffeur-<version>.dmg.sha256` (the sha256sum format: hash, two spaces, bare
# file name).
(cd "$(dirname "$OUT")" && shasum -a 256 "$(basename "$OUT")" > "$(basename "$OUT").sha256")
echo "$OUT ($(du -h "$OUT" | cut -f1))"
echo "$OUT.sha256 ($(cut -d' ' -f1 "$OUT.sha256"))"
