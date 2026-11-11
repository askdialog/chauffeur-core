#!/usr/bin/env bash
# Unpacks the Linux worker bundle in a container with the oldest glibc it claims to support
# (Ubuntu 22.04's, 2.35) and checks that its tools load, run and that its daemon starts.
# Usage: scripts/smoke-linux-bundle.sh [path to the .tar.gz]
#        (default: the one scripts/package-linux.sh builds); SMOKE_IMAGE overrides the image.
set -euo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
source "$repo/scripts/release-channel.sh"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo/Cargo.toml" | head -1)
archive=${1:-$repo/dist/$CHAUFFEUR_PACKAGE_PREFIX-$version-linux-x64.tar.gz}
[ -f "$archive" ] || { echo "No bundle at $archive: build it with scripts/package-linux.sh." >&2; exit 1; }
image=${SMOKE_IMAGE:-ubuntu:22.04}

archive=$(cd "$(dirname "$archive")" && pwd -P)/$(basename "$archive")
docker run --rm --platform linux/amd64 -e "VERSION=$version" \
  -v "$archive:/bundle.tar.gz:ro" "$image" bash -euo pipefail -c '
    # What a worker has and a bare image lacks: the daemon needs the system CA certificates.
    apt-get update -qq >/dev/null && apt-get install -y -qq ca-certificates git >/dev/null
    mkdir /bundle && tar xzf /bundle.tar.gz -C /bundle
    cd /bundle
    echo "== every tool is there and loads"
    for tool in chauffeur chauffeurd chauffeur-sidecar; do
      [ -x "$tool" ] || { echo "$tool is missing from the bundle, or not executable." >&2; exit 1; }
      if ldd "./$tool" 2>&1 | grep -q "not found"; then
        ldd "./$tool" >&2
        echo "$tool needs a library this image does not have." >&2
        exit 1
      fi
    done
    echo "== the CLI is this version"
    ./chauffeur --version | grep -q "$VERSION" || { echo "chauffeur --version does not say $VERSION." >&2; exit 1; }
    echo "== the daemon starts and answers"
    export CHAUFFEUR_HOME=/tmp/home CHAUFFEUR_WORKSPACES_DIR=/tmp/workspaces CHAUFFEUR_REPOS_DIR=/tmp/repos
    export CHAUFFEUR_PORT_OVERRIDE=0 CHAUFFEURD=/bundle/chauffeurd
    mkdir -p "$CHAUFFEUR_HOME"
    printf "linear_enabled = false\n[office]\nurl = \"\"\n" > "$CHAUFFEUR_HOME/settings.toml"
    ./chauffeur daemon start >/dev/null || { tail -n 40 "$CHAUFFEUR_HOME/logs/chauffeurd.log" >&2; exit 1; }
    ./chauffeur ls >/dev/null
    ./chauffeur daemon stop >/dev/null
  '
echo "The Linux bundle works: Chauffeur $version"
