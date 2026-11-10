#!/usr/bin/env bash
# Builds Chauffeur and installs it for this user, on macOS or Linux:
#   ~/.chauffeur/bin/{chauffeur,chauffeurd,chauffeur-sidecar}, ~/.local/bin/chauffeur → it,
#   with --app, the desktop app: /Applications/Chauffeur.app on macOS; on Linux
#     ~/.chauffeur/bin/chauffeur-app with its launcher in ~/.local/share/applications,
#   and, with --launch-agent, the daemon started at login (a LaunchAgent, or a systemd user service).
set -euo pipefail
cd "$(dirname "$0")/.."
DAEMON_PACKAGE=chauffeurd
DAEMON_BINARY=chauffeurd
if [[ -f distribution/build-env.sh ]]; then
  # shellcheck source=/dev/null
  source distribution/build-env.sh
fi
BIN="${CHAUFFEUR_HOME:-$HOME/.chauffeur}/bin"

# A worktree is a feature preview, while the default install is shared by every
# workspace. Replacing it must be intentional: another branch may lack features
# (such as mobile trust RPCs) already in use by the installed daemon.
if [[ "${CHAUFFEUR_HOME:-$HOME/.chauffeur}" == "$HOME/.chauffeur" ]] \
  && { [[ -n "${CHAUFFEUR_WORKSPACE_ID:-}" ]] \
    || [[ "$(git rev-parse --git-dir)" != "$(git rev-parse --git-common-dir)" ]]; } \
  && [[ " $* " != *" --replace-shared "* ]]; then
  echo "Refusing to replace the shared Chauffeur installation from a feature workspace." >&2
  echo "Use an isolated CHAUFFEUR_HOME for a preview, or --replace-shared for an intentional installation." >&2
  echo "Installing another branch can remove unreleased features and interrupt active agents." >&2
  exit 1
fi

# Replace, never overwrite in place: a running daemon or app keeps its old binary.
install_bin() {
  local name=${2:-$(basename "$1")}
  cp "$1" "$BIN/.$name.new" && mv -f "$BIN/.$name.new" "$BIN/$name"
}

cargo build --release -p "$DAEMON_PACKAGE" -p chauffeur-cli
(cd sidecar && bun install --frozen-lockfile && bun run build)
(cd relay && bun install --frozen-lockfile && bun run build:host)
(cd packages/remote && bun install --frozen-lockfile)
mkdir -p "$BIN" "$HOME/.local/bin"
for f in target/release/chauffeur sidecar/dist/chauffeur-sidecar relay/dist/chauffeur-mobile-host; do
  install_bin "$f"
done
install_bin "target/release/$DAEMON_BINARY" chauffeurd
ln -sf "$BIN/chauffeur" "$HOME/.local/bin/chauffeur"
echo "installed in $BIN"
case ":$PATH:" in
  *":$HOME/.local/bin:"*) ;;
  *) echo "$HOME/.local/bin is not on your PATH; add it, then open a new terminal:"
     case "$(basename "${SHELL:-zsh}")" in
       fish) echo "  fish_add_path ~/.local/bin" ;;
       bash) echo "  echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> ~/.bashrc" ;;
       *) echo "  echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> ~/.zshrc" ;;
     esac ;;
esac

if [[ " $* " == *" --app "* ]]; then
  if [ "$(uname)" = Darwin ]; then
    (cd app && bun install --frozen-lockfile && bunx tauri build)
    rm -rf /Applications/Chauffeur.app
    ditto target/release/bundle/macos/Chauffeur.app /Applications/Chauffeur.app
    echo "installed /Applications/Chauffeur.app"
  else
    # The app next to the daemon, which it starts when none runs; a launcher named after the
    # window's Wayland app_id, so that desktops show its icon.
    (cd app && bun install --frozen-lockfile && bunx tauri build --no-bundle)
    install_bin target/release/chauffeur-app
    # The Office's page for the browser, served by the daemon (docs/office.md): calls need WebRTC,
    # which a WebKitGTK may be built without.
    rm -rf "$BIN/.chauffeur-web.new" && cp -R app/dist "$BIN/.chauffeur-web.new"
    rm -rf "$BIN/chauffeur-web" && mv "$BIN/.chauffeur-web.new" "$BIN/chauffeur-web"
    ICONS="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor"
    for size in 32 128 256 512; do
      case $size in 32) src=32x32.png ;; 128) src=128x128.png ;; 256) src=128x128@2x.png ;; 512) src=icon.png ;; esac
      install -Dm644 "app/src-tauri/icons/$src" "$ICONS/${size}x${size}/apps/chauffeur.png"
    done
    APPS="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
    mkdir -p "$APPS"
    cat > "$APPS/chauffeur-app.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Chauffeur
Comment=Multi-repo workspaces for coding agents
Exec="$BIN/chauffeur-app" %u
MimeType=x-scheme-handler/chauffeur;
Icon=chauffeur
Terminal=false
Categories=Development;
StartupWMClass=chauffeur-app
DESKTOP
    command -v update-desktop-database >/dev/null && update-desktop-database -q "$APPS" || true
    echo "installed $BIN/chauffeur-app (Chauffeur in your app launcher)"
  fi
fi

# Replacing the user's daemon is this script's purpose, run from a workspace's terminal or by an
# agent the user asked to install: the daemon commands would refuse it there otherwise.
if [[ " $* " == *" --launch-agent "* ]]; then
  "$BIN/chauffeur" daemon install --real-daemon
else
  "$BIN/chauffeur" daemon stop --real-daemon >/dev/null 2>&1 || true
  echo "the daemon starts on first use; run with --launch-agent to start it at login"
fi
