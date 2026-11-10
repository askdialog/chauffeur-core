#!/usr/bin/env bash
# Ubuntu 24.04 amd64 desktop package, built natively. Creates an artifact only;
# installation and daemon activation remain explicit user actions.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || {
  echo 'Build this package on Ubuntu 24.04 amd64.' >&2; exit 1;
}
source /etc/os-release
[[ $ID == ubuntu && $VERSION_ID == 24.04 ]] || {
  echo 'This package targets Ubuntu 24.04; use scripts/install.sh on other distributions.' >&2; exit 1;
}
DAEMON_PACKAGE=chauffeurd
DAEMON_BINARY=chauffeurd
if [[ -f distribution/build-env.sh ]]; then source distribution/build-env.sh; fi
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
export CHAUFFEUR_BUILD="$version+$(git rev-parse --short HEAD)$(git diff --quiet HEAD || echo .dirty)"
cargo build --locked --release -p "$DAEMON_PACKAGE" -p chauffeur-cli
(cd sidecar && bun install --frozen-lockfile && bun run build)
(cd relay && bun install --frozen-lockfile && bun run build:host)
(cd app && bun install --frozen-lockfile && bunx tauri build --no-bundle)

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
bin="$stage/usr/lib/chauffeur"
mkdir -p "$bin" "$stage/DEBIAN" "$stage/usr/bin" "$stage/usr/share/applications"
install -m755 target/release/chauffeur target/release/chauffeur-app sidecar/dist/chauffeur-sidecar relay/dist/chauffeur-mobile-host "$bin/"
install -m755 "target/release/$DAEMON_BINARY" "$bin/chauffeurd"
cp -R app/dist "$bin/chauffeur-web"
ln -s ../lib/chauffeur/chauffeur "$stage/usr/bin/chauffeur"
for size in 32 128 256 512; do
  case $size in 32) icon=32x32.png ;; 128) icon=128x128.png ;; 256) icon=128x128@2x.png ;; 512) icon=icon.png ;; esac
  install -Dm644 "app/src-tauri/icons/$icon" "$stage/usr/share/icons/hicolor/${size}x${size}/apps/chauffeur.png"
done
cat > "$stage/usr/share/applications/chauffeur-app.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=Chauffeur
Comment=Multi-repo workspaces for coding agents
Exec=/usr/lib/chauffeur/chauffeur-app %u
MimeType=x-scheme-handler/chauffeur;
Icon=chauffeur
Terminal=false
Categories=Development;
StartupWMClass=chauffeur-app
DESKTOP
cat > "$stage/DEBIAN/control" <<CONTROL
Package: chauffeur
Version: $version
Architecture: amd64
Maintainer: Chauffeur <support@askdialog.com>
Section: devel
Priority: optional
Depends: libc6 (>= 2.39), libstdc++6, libgcc-s1, libwebkit2gtk-4.1-0, libgtk-3-0t64, libayatana-appindicator3-1, libssl3t64, ca-certificates, git, xdg-utils
Recommends: bubblewrap, socat, gnome-keyring
Description: Workspaces and conversations for coding agents
 Includes the desktop client, CLI, daemon, agent sidecar and mobile host.
Installed-Size: $(du -sk "$stage/usr" | cut -f1)
CONTROL
mkdir -p dist
output="dist/chauffeur_${version}_ubuntu24.04_amd64.deb"
dpkg-deb --root-owner-group --build "$stage" "$output"
(cd dist && sha256sum "$(basename "$output")" > "$(basename "$output").sha256")
echo "$output"
