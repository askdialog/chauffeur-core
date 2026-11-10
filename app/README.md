# Chauffeur desktop app

A Tauri v2 shell (`src-tauri/`, Rust) around a React 19 + TypeScript UI (`src/`, Vite, Tailwind
CSS v4). The UI is a **client of the daemon** `chauffeurd`, exactly like the CLI: it speaks the
JSON-RPC protocol of `protocol-ts` (imported as `@chauffeur/protocol`) over the daemon's
WebSocket. Tauri only covers native concerns: finding or starting the daemon, the app menu (⌘N
New Workspace, ⌘K Switch Workspace; macOS only, the UI handles the same keys), desktop notifications and
window state. It runs on macOS and Linux (WebKitGTK); shortcuts use ⌘ on macOS and Ctrl elsewhere
(`src/lib/keys.ts`).

## How the app finds the daemon

The Tauri command `daemon_connection` returns `{url, token}`:

1. The state folder is `$CHAUFFEUR_HOME`, else `~/.chauffeur`.
2. If `daemon.json` names a port where a chauffeurd answers `GET /health`, the app uses it, with
   the token from `token`.
3. Otherwise it starts the daemon detached (own process group, output appended to
   `<home>/logs/chauffeurd.log`) and waits up to 20 s for a healthy `daemon.json`. The binary is
   `$CHAUFFEURD`, else `chauffeurd` next to the app executable (in development the shell is a
   member of the Cargo workspace, so `target/debug/chauffeur-app` sits next to
   `target/debug/chauffeurd`), else `<repo>/target/debug/chauffeurd` in debug builds, else
   `chauffeurd` on the `PATH`.

The UI then opens `ws://127.0.0.1:<port>/ws?token=<token>` (the daemon accepts the
`tauri://localhost` origin). On disconnect it reconnects with backoff, asking for the connection
again each time (the daemon may have restarted on another port), and re-subscribes every
subscription with `after_seq` = the last persisted `seq` it saw, so no event is lost.

## Development

Prerequisites: Bun, Rust, the daemon and the agent sidecar built once (real chats also need
Claude Code logged in on the machine):

```sh
cargo build -p chauffeurd -p chauffeur-cli     # from the repo root
(cd sidecar && bun install && bun run build)   # the daemon finds sidecar/dist/chauffeur-sidecar
cd app && bun install
```

**In a browser, against a throwaway daemon** (no Tauri needed; this is how the UI is tested):

```sh
eval "$(app/scripts/sandbox.sh)"   # temp CHAUFFEUR_HOME, repos alpha/beta/gamma with run scripts, Linear off
cd app && bun run dev              # http://localhost:1420
app/scripts/sandbox.sh stop        # stops that daemon and deletes the sandbox
```

For cheap real chats, pick Haiku 5.5 in the model picker (search "haiku"); Haiku 4.5 has no auto mode, so its chats ask before acting.

In development the Vite server reads `$CHAUFFEUR_HOME/daemon.json` and `token` (default
`~/.chauffeur`) and gives them to the page: injected into `index.html` and served fresh at
`/__chauffeur/connection`. This exists only in `vite dev`, never in a build. The port is
`$CHAUFFEUR_VITE_PORT`, else 1420.

`http://localhost:1420/#chat-fixture` renders the chat view with fabricated events (tool calls,
subagents, permission requests, questions, streaming text), to work on it without the agent
sidecar. It is development only and not part of the build.

**In the Tauri shell:** `bun run tauri dev` (starts Vite, then the shell; the shell starts the
daemon of `$CHAUFFEUR_HOME` if needed).

## Checks

```sh
bun run typecheck                      # tsc
bun test                               # reducers, RPC client framing, terminal splicing, diff parser
bun run build                          # Vite → dist/
bunx tauri build --debug --no-bundle   # compiles the shell with the UI embedded → target/debug/chauffeur-app
bunx tauri build                       # Chauffeur.app (unsigned)
```

## Layout

- `src/lib/rpc.ts`: the typed daemon client (calls typed by `ClientMethods`, subscriptions,
  reconnect, replay deduplication). Tested with a fake socket in `rpc.test.ts`.
- `src/lib/chat.ts`: the pure reduction of a chat's event stream into what the chat view shows
  (deltas replaced by final messages, tool results attached to their calls, subagent nesting,
  requests opened, resolved and expired, answers read back from AskUserQuestion, per-turn cost
  out of the session's running total). `src/lib/workspaceLive.ts` does the same for a workspace
  (chats, log, scripts' output). `src/lib/terminalSplice.ts` joins a terminal's snapshot and its
  live output by byte offset.
- Workspaces are created with `background: true`: the dialog follows the new workspace's events
  until `state_changed` says `ready` (or `archived` with the error), and can let it continue in
  the background, where the workspace view shows the progress.
- `src/state/app.ts`: workspaces and chats kept live by a `{registry: true}` subscription, the
  selection, unread markers and notifications. `src/state/session.ts`: one subscription per open
  workspace (`after_seq: 0`, ephemeral events), terminal output routed straight to xterm.js.
- `src/components/`: sidebar, create dialog, workspace view (header, chat, diff, terminal,
  processes), quick switcher.
- `src-tauri/`: `daemon.rs` (find or start the daemon), `main.rs` (menu, plugins, commands).

## Shortcuts

⌘N new workspace · ⌘K switch workspace · ⌘1…⌘9 workspaces of the sidebar · Enter or ⌘Enter
send (Shift+Enter for a new line) · Esc interrupt the agent · ⌘Enter create in the dialog.
