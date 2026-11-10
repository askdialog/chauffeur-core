#!/usr/bin/env bash
# Installs the Ubuntu desktop package with apt in a clean Ubuntu 24.04 container, as a user would,
# and checks what they get: every program there and loading, the `chauffeur` command of this
# version on the PATH, and a daemon that starts and answers in a throwaway CHAUFFEUR_HOME.
# Usage: scripts/smoke-linux-desktop.sh [path to the .deb]
#        (default: the one scripts/package-linux-desktop.sh builds); SMOKE_IMAGE overrides the image.
set -euo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo/Cargo.toml" | head -1)
package=${1:-$repo/dist/chauffeur_${version}_ubuntu24.04_amd64.deb}
[ -f "$package" ] || { echo "No package at $package: build it with scripts/package-linux-desktop.sh." >&2; exit 1; }
image=${SMOKE_IMAGE:-ubuntu:24.04}

package=$(cd "$(dirname "$package")" && pwd -P)/$(basename "$package")
docker run --rm --platform linux/amd64 -e "VERSION=$version" \
  -v "$package:/tmp/chauffeur.deb:ro" "$image" bash -euo pipefail -c '
    fail() { echo "$1" >&2; exit 1; }
    echo "== apt installs the package and what it depends on"
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq >/dev/null
    apt-get install -y -qq /tmp/chauffeur.deb >/dev/null
    [ "$(dpkg-query -W -f "\${Version}" chauffeur)" = "$VERSION" ] || fail "The package is not version $VERSION."
    echo "== every program is there and loads"
    lib=/usr/lib/chauffeur
    for tool in chauffeur chauffeurd chauffeur-sidecar chauffeur-mobile-host chauffeur-app; do
      [ -x "$lib/$tool" ] || fail "$tool is missing from the package, or not executable."
      if ldd "$lib/$tool" 2>&1 | grep -q "not found"; then
        ldd "$lib/$tool" >&2
        fail "$tool needs a library the package does not depend on."
      fi
    done
    [ -f "$lib/chauffeur-web/index.html" ] || fail "The web app (chauffeur-web) is missing from the package."
    [ -f /usr/share/applications/chauffeur-app.desktop ] || fail "The package has no app launcher entry."
    echo "== the chauffeur command is this version"
    [ "$(command -v chauffeur)" = /usr/bin/chauffeur ] || fail "chauffeur is not on the PATH as /usr/bin/chauffeur."
    chauffeur --version | grep -qF "$VERSION" || fail "chauffeur --version does not say $VERSION."
    echo "== the daemon starts and answers"
    # No CHAUFFEURD: the command must find the daemon installed beside it.
    export CHAUFFEUR_HOME=/tmp/home CHAUFFEUR_WORKSPACES_DIR=/tmp/workspaces CHAUFFEUR_REPOS_DIR=/tmp/repos
    export CHAUFFEUR_PORT_OVERRIDE=0
    mkdir -p "$CHAUFFEUR_HOME"
    printf "linear_enabled = false\n[office]\nurl = \"\"\n" > "$CHAUFFEUR_HOME/settings.toml"
    chauffeur daemon start >/dev/null || { tail -n 40 "$CHAUFFEUR_HOME/logs/chauffeurd.log" >&2; exit 1; }
    chauffeur ls >/dev/null
    chauffeur daemon stop >/dev/null
  '
echo "The desktop package works: Chauffeur $version"
