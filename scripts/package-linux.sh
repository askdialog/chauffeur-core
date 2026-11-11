#!/usr/bin/env bash
# Builds the Linux x86_64 bundle a GCP worker installs (docs/cloud-workers.md):
#   dist/chauffeur-<version>-linux-x64.tar.gz (chauffeur, chauffeurd, chauffeur-sidecar) and its .sha256.
# Built from a Mac or from Linux x86_64 (the release workflow): Rust with cargo-zigbuild against
# glibc 2.35 (Ubuntu 22.04 and later), the sidecar with bun --target, from a copy of sidecar/
# holding the Linux build of Claude Code.
# The release skill publishes both files; for a build that is not released, set `bundle` in
# [workers] to the tarball.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/release-channel.sh
DAEMON_PACKAGE=chauffeurd
DAEMON_BINARY=chauffeurd
if [[ -f distribution/build-env.sh ]]; then
  # shellcheck source=/dev/null
  source distribution/build-env.sh
fi
for tool in zig cargo-zigbuild bun rustup; do
  command -v "$tool" >/dev/null || {
    echo "$tool is missing: brew install zig && cargo install --locked cargo-zigbuild (bun and rustup as for scripts/install.sh)" >&2
    exit 1
  }
done
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
target=x86_64-unknown-linux-gnu
work=target/package-linux
name=$CHAUFFEUR_PACKAGE_PREFIX-$version-linux-x64.tar.gz

rustup target add "$target" >/dev/null
cargo zigbuild --release --target "$target.2.35" -p "$DAEMON_PACKAGE" -p chauffeur-cli

rm -rf "$work" && mkdir -p "$work/bin"
rsync -a --exclude node_modules --exclude dist sidecar/ "$work/sidecar/"
ln -s "$PWD/protocol-ts" "$work/protocol-ts"
(cd "$work/sidecar" && bun install --frozen-lockfile --os linux --cpu x64 >/dev/null \
  && bun scripts/build.ts --target bun-linux-x64 --outfile dist/chauffeur-sidecar)

cp "target/$target/release/chauffeur" "$work/sidecar/dist/chauffeur-sidecar" "$work/bin/"
cp "target/$target/release/$DAEMON_BINARY" "$work/bin/chauffeurd"
scripts/package-core-notices.sh "$work/bin/licenses"
mkdir -p dist
# Without macOS metadata, which GNU tar on the worker would warn about; GNU tar adds none.
mac_only=()
[ "$(uname)" = Darwin ] && mac_only=(--no-xattrs --no-mac-metadata)
COPYFILE_DISABLE=1 tar "${mac_only[@]}" -czf "dist/$name" -C "$work/bin" chauffeur chauffeurd chauffeur-sidecar licenses
(cd dist && shasum -a 256 "$name" > "$name.sha256")
echo "dist/$name"
