# Contributing to Chauffeur

Thanks for helping. Chauffeur is maintained by Justin at Dialog; the maintainer decides what goes
in. For anything larger than a fix, open an issue first and say what you want to change and why.

## Set up

Install the prerequisites of the [README](README.md#prerequisites), then build once:

```sh
cargo build -p chauffeurd -p chauffeur-cli
(cd sidecar && bun install --frozen-lockfile && bun run build)
(cd app && bun install --frozen-lockfile)
```

Work on the UI in a browser against a throwaway daemon: see [app/README.md](app/README.md#development).

## Never touch your real state

Tests and scripts never use the real `~/.chauffeur` or `~/chauffeur`. Set `CHAUFFEUR_HOME`,
`CHAUFFEUR_WORKSPACES_DIR` and `CHAUFFEUR_REPOS_DIR` to a temporary folder,
`CHAUFFEUR_PORT_OVERRIDE=0`, and `linear_enabled = false` in that folder's `settings.toml` (a real
Linear call would move real tickets). `app/scripts/sandbox.sh` and the end-to-end scripts do this
for you. Agents' transcripts still land in `~/.claude/projects`: remove them with
`scripts/forget-transcripts.sh <temp folder>`, never with an `rm` pattern of your own.

## Before committing

```sh
cargo fmt --all
cargo clippy --all-targets          # no warnings
cargo test
(cd sidecar && bun run typecheck && bun test)
(cd app && bun run typecheck && bun test)
```

Run `scripts/e2e-cli.sh` after a daemon or CLI change, and `scripts/e2e-chat.sh` after a chat or
sidecar change (it runs real Claude Code and Codex agents). After changing
`crates/chauffeur-protocol`, run `scripts/gen-protocol.sh`: `cargo test` fails while `protocol-ts/`
is stale.

Read [CODING_STANDARDS.md](CODING_STANDARDS.md) before writing code; reviews use it. A change of
behaviour comes with the test that fails without it.

## Commits and pull requests

- Commit messages read `type(scope): subject`, such as `feat(cloud): …` or `fix(chat): …`.
- One subject per pull request. Pull requests land by squash, so a branch's history only serves
  the review. Update a branch by rebasing onto `main`, never by merging `main` into it.
- Say in the description what changes for the user and how you checked it (tests, end-to-end
  scripts, a screenshot of the app for a UI change).
- CI must be green: formatting, clippy, Rust tests, the TypeScript packages, and the ADR index.

The required `check` status collects the checks selected for a pull request. Documentation-only
changes run repository checks; JavaScript-only changes also run package checks. Shared protocol
and native/build inputs run Rust and JavaScript in parallel. Unknown paths get full coverage.
Main and manual runs always check both. Linux packages are built and smoke-tested for packaging
changes, nightly, and on manual dispatch; see [the CI decision](docs/adr/core-ci-by-changed-surface.md).

## Behaviour decisions

[docs/SPEC.md](docs/SPEC.md) describes how Chauffeur behaves. A change of behaviour builds within
it; a new decision is taken by the maintainer, then recorded as an ADR in `docs/adr/` in the same
pull request as the change ([how](docs/adr/README.md)), with `SPEC.md` updated to match. Run
`python3 .github/scripts/adr_index.py` after adding an ADR or changing its title.

## Conduct and security

Follow the [Code of Conduct](CODE_OF_CONDUCT.md). Report vulnerabilities privately, as
[SECURITY.md](SECURITY.md) says, never in a public issue.

## License

Contributions to Chauffeur Core are accepted under the [MIT license](LICENSE). By submitting a
contribution, you agree to license it under those terms. Keep third-party copyright and license
notices with any code you bring in, and update [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
