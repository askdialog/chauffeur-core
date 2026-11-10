# Platforms

Chauffeur runs on macOS (Apple Silicon, the disk image) and Linux (x86_64, built from source; tested on Arch Linux under Wayland). GCP workers run the Linux bundle the release publishes (`scripts/package-linux.sh`, amd64). Windows is not supported.

How to install and what differs for a Linux user: [`README.md`](../README.md#on-linux). This page lists what the code does differently per platform, found with `grep -rn 'cfg(target_os' crates app/src-tauri` and the daemon's run-time checks, as of 2026-10-29.

## What differs

| Feature | macOS | Linux | Code |
|---|---|---|---|
| Disk image and in-app updates | The disk image, laid out by dmgbuild (`scripts/dmg/`), opens on the app, a link to Applications and Chauffeur's icon. A packaged `Chauffeur.app` finds, downloads, checks and installs a newer release (`hdiutil`, `ditto`) | None: no packaged app. Update with `git pull && scripts/install.sh --app --launch-agent` | `crates/chauffeurd/src/update.rs`, `scripts/package.sh` |
| Start at login | LaunchAgent `com.askdialog.chauffeurd` | systemd user service `chauffeurd.service` | `crates/chauffeurd/src/login.rs`, `crates/chauffeur-cli/src/service.rs` |
| Secrets (Linear key, Jev key, backup keys, Office sign-in) | Keychain (`security`) | Secret Service (`secret-tool`, GNOME Keyring, KWallet…) when one runs, else `~/.config/chauffeur/<file>`, mode 600. A GCP worker keeps its keys in owner-only files of its state folder | `crates/chauffeurd/src/secrets.rs`, `crates/chauffeur-cli/src/linear_key.rs` |
| Keeping the machine awake (`keep_awake`) | `while_working` with `caffeinate -i -w`; `lid_closed` with `pmset -a disablesleep` through a sudoers rule | `while_working` with `systemd-inhibit` when logind is there (logged once otherwise); `lid_closed` is not offered | `crates/chauffeurd/src/power.rs` |
| Sharp screens on the design canvas | Each screen pictured natively (offscreen WKWebViews) and laid over the live frame | Live frames only, which WebKit blurs under zoom when a page holds a composited layer | `app/src-tauri/src/screen_snapshot.rs` |
| App menu and its keys | A native menu showing the keys in force | No menu: the UI catches the keys; Ctrl replaces ⌘ | `app/src-tauri/src/menu.rs` |
| Tray icon | None (the Dock's badge) | StatusNotifierItem, needs `libayatana-appindicator` | `app/src-tauri/src/tray.rs` |
| Open in an editor, a terminal, the file manager | `open -a` on apps found in the app folders | The app's command found on the PATH, with the desktop session's variables | `crates/chauffeurd/src/open.rs` |
| Opening a URL or a file | `open`, `open -R` (Show in Finder) | `xdg-open` (on the file's folder for Show in Finder) | `crates/chauffeurd/src/open.rs`, `office.rs`, `app/src-tauri/src/main.rs` |
| Export PNG to the clipboard | `osascript`; Chrome looked for in the app folders | `wl-copy`, else `xclip`, when installed; Chrome looked for on the PATH | `crates/chauffeurd/src/design_export.rs` |
| Office calls | WKWebView has WebRTC | WebKitGTK needs its permission requests answered; Arch's is built without WebRTC, so the Office opens in the browser (`chauffeur office --browser`) | `app/src-tauri/src/webkit_media.rs`, `crates/chauffeurd/src/extensions/office/web.rs` |
| The Office's browser page, same-user check | Not checked (`chauffeur-web` is not in the disk image) | The connection's owner is read from `/proc/net/tcp` | `crates/chauffeurd/src/extensions/office/web.rs` |
| Explorations' and design chats' sandbox | Seatbelt, built in | Claude Code's sandbox needs `bubblewrap` and `socat`; without them an exploration's Claude chat refuses to start | `SPEC.md`, Explorations |
| A Codex chat reaching the daemon's socket | Through Codex's network proxy, the daemon's socket only | Refused: Codex's sandbox allows every Unix socket or none | `SPEC.md`, Generated workspace config ([ADR 0020](adr/0020-codex-in-a-multi-repo-workspace.md)) |
| A workspace's processes, at archive and before a transfer | `lsof` for working folders; attribution by descent from the daemon or by session (macOS hides other processes' environment) | `/proc`: a process is the workspace's when its environment carries `CHAUFFEUR_WORKSPACE_ID` | `crates/chauffeurd/src/procs.rs`, `workspace_processes.rs`, `cloud_process.rs` |
| Open file limit | Raised to the kernel's per-process maximum | Raised to the hard limit | `crates/chauffeurd/src/main.rs` |
| Blank window with an NVIDIA GPU | — | `WEBKIT_DISABLE_DMABUF_RENDERER=1` set when NVIDIA's driver is found | `app/src-tauri/src/main.rs` |
| Scripts' shell | zsh (always installed) | zsh when installed, else bash, else sh | `crates/chauffeurd/src/shellenv.rs` |

## Known Linux limits

- `scripts/package-linux-desktop.sh` builds an Ubuntu 24.04 amd64 `.deb` with the desktop, daemon, CLI, sidecar and mobile host. The Linux desktop workflow (`.github/workflows/linux-desktop.yml`) builds it, then `scripts/smoke-linux-desktop.sh` installs it with apt in a clean Ubuntu 24.04 container and starts its daemon in a throwaway home, on every push to `main` and on pull requests that touch what it is built from. Other distributions keep the source installer. No in-app Linux updater: install a newer package explicitly; no background package replacement. Native desktop/device validation remains separate from the build check.
- No `lid_closed` keep-awake mode, and no sharp screen pictures on the design canvas.
- Office calls do not work in the app on distributions whose WebKitGTK lacks WebRTC (Arch): the Office opens in the browser instead.
- The sandbox of explorations and design chats depends on packages the user installs (`bubblewrap`, `socat`; on Ubuntu 24.04+, an AppArmor change).
- A Codex chat cannot run `chauffeur` commands from inside its sandbox.

First Linux validation, moved from `docs/PLAN.md` (2026-10-14): daemon, CLI and app build and run on Arch Linux x86_64 under Wayland (tested in an OrbStack Arch machine: `cargo test`, `scripts/e2e-cli.sh`, `scripts/install.sh --app --launch-agent` with its systemd user service, the app under sway, opening a workspace in foot). Not yet exercised there: a real Claude chat (no login in the test machine), Codex, the Secret Service with an unlocked keyring, NVIDIA.

## Windows

Not supported. `SPEC.md` scopes Chauffeur to macOS and Linux, and the code relies on Unix throughout:

- **Terminals**: PTYs with process groups and Unix signals for stopping a command and what it started (`portable_pty`, `libc`), in `crates/chauffeurd/src/terminals.rs` and `procs.rs`.
- **The daemon's socket**: the CLI, agents and the mobile host reach the daemon over a Unix socket only the user can create, and the app knows the daemon is up through it (`crates/chauffeurd/src/server.rs`).
- **The environment**: every spawn gets the login shell's environment, captured from zsh or bash (`crates/chauffeurd/src/shellenv.rs`), and scripts run in zsh or bash.

A port would need a decision first ([`adr/`](adr/README.md)).

## Ubuntu desktop package

On Ubuntu 24.04 amd64, install the [Tauri build prerequisites](https://v2.tauri.app/start/prerequisites/#linux), Bun and Rust, then run `scripts/package-linux-desktop.sh`. The artifact and checksum land in `dist/`; the script neither installs nor publishes them. Install a reviewed artifact with `sudo apt install ./chauffeur_<version>_ubuntu24.04_amd64.deb`. Start Chauffeur from the app launcher; use `chauffeur doctor` for sandbox, keyring, graphical-session and service diagnostics. `chauffeur daemon install` explicitly enables startup at login.

Updates use the same package command. Removing the package leaves the user's workspaces, credentials and daemon state intact. Stop the user's daemon before uninstalling it. Linux Office calls retain the browser fallback when WebKitGTK lacks WebRTC.
