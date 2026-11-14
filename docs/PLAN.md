# Build plan

Stages, in order. Each stage ends with something runnable and its tests green. Story numbers refer to `SPEC.md`. Status as of 2026-10-29: **done** (shipped), **partial** (shipped with a part left, said in the row), **not started**.

Platforms: [`platforms.md`](platforms.md) (what differs on Linux, the first Linux validation, Windows).

| Stage | Status | Delivers | Stories |
|---|---|---|---|
| 0. Scaffold | done | Cargo workspace, sidecar and app packages, protocol crate with generated TypeScript | — |
| 1. Daemon skeleton | done | `chauffeurd` (store, event log, WebSocket + Unix socket, token auth), `chauffeur daemon start/stop/status` | 15, 35 |
| 2. Workspaces | done | repo registry, workspace create with N worktrees, ports, env, files to copy, setup scripts, generated workspace config, notes folder, archive (process groups, archive scripts, compose down, snapshot refs, worktree removal), restore | 1, 5–8, 10, 16 (config part), 29–32, 34, 36 |
| 3. Sidecar and Claude chats | done | Bun sidecar with the Claude Agent SDK, chat create/send/queue/interrupt, permissions and questions, resume after restart | 12–15 |
| 4. Worktree routing and child workspaces | done | `chauffeur worktree add/remove`, WorktreeCreate/WorktreeRemove hooks, `git worktree add` guard, `chauffeur new --parent`, `status/wait/send`, limits | 19–23 |
| 5. Desktop app | done | Tauri shell + React UI: sidebar with statuses, create dialog, chat view, diff view, terminal, run scripts, notifications | 11, 25–27 |
| 6. Linear and PRs | done | ticket attach and picker, ticket context injection, In Progress, Linear custom script, PR per repo with cross-links | 2–4, 9, 17, 28 |
| 7. Codex | done | Codex through `codex app-server` in the sidecar | 18 |
| 9a. Design chats | done | chat kind `design`, its folder and guards (Claude only), the design brief from a source chat into the composer, the designer's system prompt, `chauffeur chat --design [--from]`; screens opened from the Files tab | 47 |
| 9b. Canvas | done | the screens' own origin in the daemon, `design.canvas`, the design chat's tab (chat left, canvas right): canvas pages, sections, live screens, pan and zoom, selection, full screen, Present, reloads, narrow layout, `chauffeur canvas` | 47 |
| 9c. Back to the code | done | Send to source chat (or to a chat), Open on claude.ai for a published screen | 47 |
| 9d. Export PNG | done | `design.export` rendered by a headless Chrome of the machine through a one-shot page on the screens' listener, the PNG in the design folder's `exports/` and on the clipboard, the canvas's Export PNG (selection, section), `chauffeur export` | 47 |
| 13a. Comments | done | `comments.json` and its RPCs, the element-capture script on framed screens, Comment mode (pins, bubbles, threads, stale), the Comments panel, "Send N new to the agent", "Keep these", the agent's prompt for `resolved`, `chauffeur comments` / `comment` | 51 |
| 13b. Share service | done | `crates/chauffeur-design-share` (Google ID tokens as the Office, SQLite and files on the VM's disk, a reviewer's session cookie, WSS for comments/reloads/presence, HTTPS uploads), its deploy on a VM, its domain, the web OAuth client | 51 |
| 13c. Sharing | done | `design.share_*` in the daemon (one WSS per shared chat, the folder pushed as it changes, reviewers' comments back into `comments.json`), `chauffeur share`, the Share dialog and button, a toast or notification on a reviewer's comment, archived designs retain their shares; delete and explicit stop revoke them, central Sharing page lists outgoing links | 51 |
| 13d. Reviewer page | done | the canvas and the comments in a second Vite build (`app/share.html`, `bun run build:share`) served by the service: Google sign-in (the web client), the snapshot and the files, Comment mode and threads over the reviewer socket, Live / Last update, who is looking; packed by `deploy/gcp.sh` | 51 |
| 10. Repos the agent proposes | done | `list_repos` / `add_repo` served by the sidecar (Claude's in-process MCP server, Codex's dynamic tools), the user asked by the tool itself, `tool.list_repos` / `tool.add_repo` to the daemon, the add-repo card (app and phone), sessions resumed with an added repo, the `chauffeur add-repo` guard | 48 |
| 11. Delegated chats | done | `delegate` served by the sidecar to both agents, `tool.delegate` / `tool.delegate_wait` / `tool.delegate_stop` to the daemon, `parent_chat` on chats, the delegated chat's system prompt, the delegate card and the delegated chat tab in the app, `chauffeur wait <chat>` | 49 |
| 12. In-app updates | done | the daemon looks for a newer release through `gh` (packaged app only), `update.status` / `update.install` (download, SHA-256 check, `hdiutil`, swap of the bundle), the update card at the foot of the sidebar with Restart, `chauffeur update` | 50 |
| 14. Local models | done | Ollama installed (pinned, checked, unpacked by the daemon; macOS and Linux) and run by the daemon (found, started on demand, stopped when unused, an Ollama already running reused as is), `ollama.status` / `ollama.pull` / `ollama.delete` with the pull's progress, local models in Codex's model picker, Codex threads opened on Ollama, Settings › Local models, `chauffeur ollama`; working offline beyond the chat itself is story 52's later step | 52 |
| 15a. Workspace shares in the service | done | `workspace_shares`, guests and roles, `suggestions` in the share service, the daemon socket (`Chats`, `History`, `Events`, `Notes`, `Suggestions`) and the guests' (`Suggestion`, `Viewers`), transcripts and notes on the VM's disk, removed at stop | 53 |
| 15b. Sharing a workspace | partial: a shared workspace's design chats are not pushed yet (`SPEC.md`, Workspace shares) | `workspace.share_*` in the daemon (one WSS per shared workspace, history then live events, notes as they change, its design chats), the `suggestions` store and `chat.suggestions` / `chat.suggestion_dismiss` / `chat.send {suggestion}`, `suggested_by` kept from the agent, `chauffeur share-workspace`, archive and delete stop shares | 53 |
| 15c. Suggestions in the app | done | the Share workspace dialog (guests as chips with their role, Open to the team, link, Live / Connecting / Offline, Stop sharing), the suggestion cards above the composer (Send, Edit, Dismiss), badges and notifications, a sent suggestion's author on its bubble | 53 |
| 15d. Guest page | partial: the design chats' canvases and messages' images are still to come | the `/w/<id>` route of `app/share.html`: the chats read-only and live, the notes, the suggest box and the suggestions' fates, Live / Last update, who is viewing | 53 |
| 17a. Markdown file comments | done | `files.comments*` in the daemon (the notes' `comments.json`, the chat that last wrote the file from its tool calls, the message for the agent), the Comments column and highlighted passages of a markdown file in the Files tab, "Send N new to <chat>", `chauffeur file-comments` / `file-comment` | 57 |
| 17b. Sharing a markdown file | done | a file share in the share service (one file, its comments, its reviewers), the daemon pushing the file as it changes and taking the reviewers' comments back, Share in the Files tab, the reviewer page rendering the markdown with its passages' comments | 57 |
| 16. PostgreSQL client | implemented; automated/app checks passed, physical Touch ID confirmation pending | Database view, scoped/global credentials, SQL consoles, schema navigation and atomic staged table edits; details in the Database extension's repository | 54 |
| Official extension updates | implemented | Signed independent engines and UI, offline bootstrap, per-extension automatic updates, view/operation leases and recovery; [details](extensions.md) | — |

Work outside these stages has its own page:

- **Mobile companion**: its own repository; Core integration: [`mobile.md`](mobile.md).
- **Cloud workspaces** (transfers, GCP workers, backups, and the slices behind them): [`cloud/README.md`](cloud/README.md).
- **Office**: its stages are in the Office extension's repository.
- **Import from Conductor** (story 55): `chauffeur import conductor`, a daemon method (`conductor.import`) with its dry run, release warning and e2e step; the app's first-launch screen is still to come ([decision](adr/conductor-import.md)).
- **Slack message imports**: [`../slack/README.md`](../slack/README.md); local form, authenticated service and direct desktop delivery implemented; direct delivery requires updated service and desktop ([decision](adr/slack-direct-desktop.md)).
- **Decisions** settled with Justin: [`adr/`](adr/README.md). Every document and its status: [`README.md`](README.md).

## Layout

```
crates/chauffeur-protocol      wire types (client ↔ daemon, daemon ↔ sidecar), TypeScript generated by ts-rs
crates/chauffeurd              the daemon (library + `chauffeurd` binary)
crates/chauffeur-cli           the `chauffeur` binary
crates/chauffeur-design        a design folder as a canvas, shared by the daemon and the share service
crates/chauffeur-design-share  the share service (design and workspace shares)
sidecar/                       Bun/TypeScript agent runtime
app/                           Tauri v2 shell (src-tauri) + React/TS UI
app/src/extensions/            Core UI registration and contribution host
crates/chauffeur-extension-api/ engine pipe SDK
app/sdk/                       versioned shared UI build inputs
# The distribution composes pinned source and signed bootstrap packages from separate repos.
crates/chauffeurd/src/extensions/ official extension host, activation and RPC handlers
packages/remote/               Shared Noise connection proof and transport
relay/                         Opaque Bun WebSocket relay, local proof and GCP setup
docs/                          SPEC, PLAN, ADRs, and a page per subject (index: docs/README.md)
```

## Conventions

- Rust 2024 edition, `cargo fmt`, `cargo clippy -D warnings`.
- The daemon owns every write; clients never touch the store or the disk layout.
- Paths: state in `~/.chauffeur/` (db, token, socket, logs, settings), workspaces in `~/chauffeur/workspaces/`, clones in `~/chauffeur/repos/`. `CHAUFFEUR_HOME` and `CHAUFFEUR_WORKSPACES_DIR` override them (tests use temp dirs).

### Daily session review (2026-10-29)

Daemon calendar task, persistent per-day workspace claims, session evidence export and an
exploration at 18:00 local time. Settings enable it by default and optionally override the
normal model. Optional owned macOS wake events respect the sleep policy and low-battery
threshold; missed days catch up at restart/wake. Local lifecycle tests use a fake sidecar.
Review workspaces are filed in a "Daily reviews" sidebar section, created folded, earlier
reviews included (2026-11-06, [decision](adr/daily-reviews-in-their-own-section.md)).

Daily history and weekly synthesis (2026-11-14): previous seven days of reports and the latest
weekly synthesis for dailies; Sunday synthesis over four weeks, recommendation follow-up,
bounded conversation snapshots including archived follow-ups, and persistent weekly claims
([decision](adr/reviews-learn-across-days.md)). Same settings and section.
Visual Reviews (2026-11-14): Overview, pattern evidence, experiments with durable dated follow-up,
and report history; bounded tool-free projection of existing reports, original source links and
explicit coverage; shared settings with weekly explanation ([decision](adr/reviews-interface.md)).

### Routines (story 56, 2026-11-08)

A prompt the daemon runs by itself at set times ([decision](adr/routines.md)): `routine.*` methods,
the `routines`, `routine_versions` and `routine_runs` tables, a scheduler checking every minute with
a cursor per routine (one catch-up run for any number of missed occurrences, none while the
previous run is busy), runs as new chats of the routine's own workspace with clean worktrees
refreshed from `origin/<base>`, `chauffeur routine`, the chat tab's "Make a routine",
the workspace's Routine view and the sidebar's state tag. Not yet: sharing, a scheduled wake, other
triggers.

Find routines (story 57, 2026-11-09, [decision](adr/find-routines.md)): `routine.find`,
`routine.suggestions`, `routine.suggestion_create` and `routine.suggestion_set`, the sidecar's
`agent.ask` (one turn without tools, Claude or Codex), the `routine_suggestions` and
`routine_suggestion_messages` tables, schedules every N days, the sidebar's Find routines button
and Suggestions entry, the modal and its cards, `chauffeur routine find|suggestions|suggestion`.

Pull request links (stage 3/7, 2026-10-29): shared session instructions ask Claude Code and Codex
to link PR mentions to their actual URL, including tables and summaries, in workspaces and explorations.

Design request routing (stage 9a, 2026-11-06): shared session instructions route interface mockups
to a design chat created from the current conversation and started with its request; unsupported
Codex `visualize` references are not used ([decision](adr/design-requests-use-design-chats.md)).

Workspace forks (stage 15, 2026-10-29): fixed chat selection per new link, daemon-filtered histories/events/suggestions and persisted across reconnections; recipient-authenticated import through New workspace and the guest page’s desktop link. Conversations seed fresh sessions; notes are copied once, code and design assets are not carried.

HTML/PDF chat deliverables extend the existing chat and file-preview surfaces: document cards, an in-chat reader, scoped downloads and format-preserving agent instructions (2026-11-10). See [the decision](adr/chat-document-deliverables.md). Dedicated slide presentation follows later.
