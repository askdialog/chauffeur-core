# Chauffeur

Chauffeur runs coding agents in multi-repository workspaces. A workspace spans the repositories
you pick, one git worktree each on one branch, and runs Claude Code and Codex chats in it. One
local daemon owns every workspace, chat, terminal and process; a desktop app and a CLI are its
clients.

Made by Dialog. Chauffeur Core is licensed under [MIT](LICENSE).

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
- **Local models** through Ollama, **cloud workers** on GCP, and optional hosted services.
- **Extensions**: Core hosts signed, independently updated extensions
  ([docs/extensions.md](docs/extensions.md)). Core alone starts with an empty registry.

## Platforms

macOS (Apple Silicon) and Linux (x86_64). Windows is not supported. What differs per platform:
[docs/platforms.md](docs/platforms.md).

## Download

Core-only packages are listed in [Core releases](https://github.com/askdialog/chauffeur-core/releases?q=core-v&expanded=true),
with `core-v` tags and `Chauffeur-Core` / `chauffeur-core` filenames.
The packages target macOS (Apple Silicon) and Ubuntu 24.04 (amd64).
See [release and packaging details](docs/releases.md), or build Core from source below.

## Prerequisites

- Rust, the version of [`rust-toolchain.toml`](rust-toolchain.toml) (rustup installs it on first use).
- Bun 1.4.x.
- `git` and the GitHub CLI `gh` (Chauffeur can also install its own `gh` and sign it in from the app).
- Claude Code and/or Codex, logged in with your own account: Chauffeur runs the agents' own
  CLIs and never handles their tokens.
- To build the desktop app, Tauri v2's system prerequisites (on Linux, WebKitGTK 4.1).

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

For a Linux source installation, `scripts/install.sh --app --launch-agent` sets up a systemd
user service and an application launcher. Update it with
`git pull && scripts/install.sh --app --launch-agent`. The tray icon
needs `libayatana-appindicator`; the sandbox of explorations and design chats needs `bubblewrap`
and `socat`; secrets go to the Secret Service when one runs. Details and known limits:
[docs/platforms.md](docs/platforms.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, checks and pull requests.
To build and sign a macOS disk image, see [the packaging guide](docs/packaging-macos.md).

## Documentation

- [docs/SPEC.md](docs/SPEC.md): how Chauffeur behaves, the reference.
- [docs/adr/](docs/adr/README.md): the decisions behind it, one file each.
- [docs/README.md](docs/README.md): every document, by subject.
- [CONTRIBUTING.md](CONTRIBUTING.md), [CODING_STANDARDS.md](CODING_STANDARDS.md),
  [SECURITY.md](SECURITY.md), [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## License

Chauffeur Core is licensed under the [MIT license](LICENSE).
See [third-party notices](THIRD_PARTY_NOTICES.md) for dependency licenses.
