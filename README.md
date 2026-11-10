# Chauffeur

Chauffeur runs coding agents in multi-repository workspaces. A workspace spans the repositories
you pick, one git worktree each on one branch, and runs Claude Code and Codex chats in it. One
local daemon owns every workspace, chat, terminal and process; a desktop app and a CLI are its
clients.

Made by Dialog. Chauffeur Core is licensed under [MIT](LICENSE).

This repository contains Core only. Dialog's distribution, the Database and Office extensions,
and the mobile app are separate projects; Core's license does not apply to their own code.
See [source repositories](docs/source-repositories.md) and
[third-party notices](THIRD_PARTY_NOTICES.md).

## Features

- **Workspaces across repositories**: one worktree per repository on one branch, its own block of
  ports, setup scripts, a notes folder, and a generated `CLAUDE.md`/`AGENTS.md` importing each
  repository's. Archiving a workspace stops and removes everything it created.
- **Claude Code and Codex chats**: several per workspace, streamed, with steering, permission
  requests, questions, forks and resume after a restart. Agents create worktrees and child
  workspaces only through Chauffeur (`chauffeur worktree add`, `chauffeur new --parent`).
- **Explorations**: read-only workspaces at the base branch, to ask how the code works.
- **Design chats**: an agent designs screens on a canvas, with comments and PNG export.
- **The workspace view**: terminals, run scripts, diff, files, pull requests per repository.
- **Linear and GitHub**: tickets attached to a workspace, one pull request per repository.
- **Local models** through Ollama, **cloud workers** on GCP, and optional hosted services (below).
- **Extensions**: Core hosts signed, independently updated extensions
  ([docs/extensions.md](docs/extensions.md)). Core alone starts with an empty registry.

## Platforms

macOS (Apple Silicon) and Linux (x86_64). Windows is not supported. What differs per platform:
[docs/platforms.md](docs/platforms.md).

## Download

A distribution publishes its packaged app as releases of a public repository, which its build names
(`CHAUFFEUR_RELEASE_REPO`) and updates itself from. Dialog's build for macOS (Apple Silicon), for
example, is at [chauffeur.askdialog.com/download](https://chauffeur.askdialog.com/download). Or build
from source, below.

## Prerequisites

- Rust, the version of [`rust-toolchain.toml`](rust-toolchain.toml) (rustup installs it on first use).
- Bun 1.4.x.
- `git` and the GitHub CLI `gh` (Chauffeur can also install its own `gh` and sign it in from the app).
- Claude Code and/or Codex, logged in with your own account: Chauffeur runs the agents' own
  CLIs and never handles their tokens.
- To build the desktop app, Tauri v2's system prerequisites (on Linux, WebKitGTK 4.1).
- To build the macOS disk image (`scripts/package.sh`), Python 3.10 or later, for dmgbuild.

## Build and run from source

Install into `~/.chauffeur/bin` (the CLI linked into `~/.local/bin`), with the desktop app and the
daemon started at login:

```sh
scripts/install.sh --app --launch-agent
```

Without `--app` only the CLI, the daemon and the agent sidecar are installed; without
`--launch-agent` the daemon starts on first use. Then open Chauffeur, or use the CLI:

```sh
chauffeur daemon status
chauffeur new --repo <repo> --prompt "…"
chauffeur ls
```

For development, build the pieces and run the UI against a throwaway daemon, as described in
[app/README.md](app/README.md):

```sh
cargo build -p chauffeurd -p chauffeur-cli
(cd sidecar && bun install --frozen-lockfile && bun run build)
cd app && bun install --frozen-lockfile && bun run dev     # or: bun run tauri dev
```

State lives in `~/.chauffeur` (settings in `~/.chauffeur/settings.toml`, also editable from the
settings window), workspaces in `~/chauffeur/workspaces`. `CHAUFFEUR_HOME` and
`CHAUFFEUR_WORKSPACES_DIR` move them, which is how tests and previews stay off your real state.

### On Linux

Linux has no packaged app and no in-app updates: build from source with
`scripts/install.sh --app --launch-agent` (the daemon becomes a systemd user service, the app gets
a launcher), and update with `git pull && scripts/install.sh --app --launch-agent`. The tray icon
needs `libayatana-appindicator`; the sandbox of explorations and design chats needs `bubblewrap`
and `socat`; secrets go to the Secret Service when one runs. Details and known limits:
[docs/platforms.md](docs/platforms.md).

### A macOS disk image

`scripts/package.sh` builds `target/release/Chauffeur-<version>.dmg` and its `.sha256`, signed ad
hoc: macOS asks once before opening a downloaded copy. The image opens on a window of its own
(Chauffeur's icon on the disk, the app and Applications on a background, an arrow between them),
laid out by [dmgbuild](https://github.com/dmgbuild/dmgbuild) without driving the Finder: the build
needs Python 3.10 or later and installs dmgbuild's pinned version into `target/dmgbuild`. The
background's source and how to render it again are in [scripts/dmg/](scripts/dmg/background.svg). To sign it with your Developer ID instead,
and have Apple notarize it so that it opens without a warning, give the build an identity of your
keychain and an App Store Connect API key:

```sh
CHAUFFEUR_CODESIGN_IDENTITY="Developer ID Application: <name> (<team id>)" \
CHAUFFEUR_NOTARY_KEY=<path to AuthKey_<key id>.p8> CHAUFFEUR_NOTARY_KEY_ID=<key id> \
CHAUFFEUR_NOTARY_ISSUER=<issuer id> scripts/package.sh
```

Without the `CHAUFFEUR_NOTARY_*` values the image is signed but not notarized
([scripts/sign-macos.sh](scripts/sign-macos.sh), [decision](docs/adr/developer-id-signing-and-notarization.md)).

## Hosted services are optional

Core works on one machine with no hosted service. Each of these is off until you configure it,
in `settings.toml` or the settings window:

| Service | Setting | Hosting it |
|---|---|---|
| Design and workspace shares | `[design_share] url` | [crates/chauffeur-design-share](crates/chauffeur-design-share/README.md) |
| The Office | `[office] url`, `[office] board_repos` | the Office extension |
| Google sign-in (Office, shares) | `[office] google_client_id`, `google_client_secret`, `google_hosted_domain` | a Google OAuth client of yours |
| Mobile companion | `[mobile] relay` | [relay/](relay/README.md) |
| Slack | `[slack] url` | [slack/](slack/README.md) |

`[distribution] member_domain` names the domain of a distribution's members: the Office, Slack and
the phone show only to a user signed in with a Google account of it ([decision](docs/adr/members-only-features.md)).

Build-time values: `CHAUFFEUR_RELEASE_REPO` (`owner/repo`) turns on update checks and the
released Linux bundle for GCP workers; `CHAUFFEUR_SENTRY_DSN` turns on error reports. A build
without them checks for no updates and sends nothing. A distribution, such as Dialog's, ships its
own defaults for all of these ([decision](docs/adr/core-open-source-without-dialog-defaults.md)).

## Documentation

- [docs/SPEC.md](docs/SPEC.md): how Chauffeur behaves, the reference.
- [docs/adr/](docs/adr/README.md): the decisions behind it, one file each.
- [docs/README.md](docs/README.md): every document, by subject.
- [CONTRIBUTING.md](CONTRIBUTING.md), [CODING_STANDARDS.md](CODING_STANDARDS.md),
  [SECURITY.md](SECURITY.md), [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

The official extensions (Database, Office) and the iOS and Android companion live in their own
repositories ([docs/source-repositories.md](docs/source-repositories.md)).

## License

To be announced.
