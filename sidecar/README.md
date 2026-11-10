# chauffeur-sidecar

The agent runtime of Chauffeur: one Bun process, spawned and supervised by `chauffeurd`, hosting every agent session.

- **Claude** runs through the official Claude Agent SDK (`@anthropic-ai/claude-agent-sdk`, pinned to `0.3.322`).
- **Codex** runs through `codex app-server` (JSON-RPC over stdio), one process per session. Its TypeScript types are generated from codex-cli `0.167.0` (stable surface only) into `src/agents/codex/generated/`.

## Build, test, run

```sh
bun install
bun run typecheck          # tsc --noEmit, strict
bun test                   # unit tests (the E2E test is skipped)
bun run build              # → dist/chauffeur-sidecar
bun run test:e2e           # build + real Claude and Codex sessions (needs both logins)
bun run gen:codex          # regenerate the Codex types from the installed codex
```

`bun run build` compiles `src/main.ts` into one executable and embeds the Claude Code binary of the host platform (`--target bun-<os>-<arch>` to cross-compile, after installing that platform's `@anthropic-ai/claude-agent-sdk-<os>-<arch>` package). The SDK cannot find its own binary from inside a compiled executable, so the sidecar extracts the embedded one on first use to `/tmp/claude-<uid>/claude-agent-sdk-<hash>/claude` (content-addressed, reused across runs).

Codex is not embedded: its npm platform package is 300 MB of binaries and resources that must sit side by side. The sidecar runs `CHAUFFEUR_CODEX_PATH`, else `codex` on the agent's `PATH`, and logs a warning when its version differs from the one the types were generated from (after a deliberate upgrade: `bun run gen:codex`, then update `CODEX_APP_SERVER_VERSION` in `src/version.ts`). Set `CHAUFFEUR_SIDECAR_E2E_CODEX=0` to skip the Codex E2E tests.

The opt-in `bash scripts/e2e-session-portability.sh` (from the repository root)
checks text, native tool history, an explicit fixture file and manually compacted context across
Mac→Linux→Mac using Docker and deterministic local inference, without real
accounts. Use `--scenario text`, `--scenario tools` or `--scenario compact` to run one scenario.
For real encrypted OpenAI compaction, the separate
`bun scripts/session-portability/hosted-run.ts --use-chatgpt-account` (from `sidecar/`)
explicitly uses the existing native ChatGPT account's quota. It keeps selected
access credentials in memory, carries only synthetic history, and deletes its
generated state. It does not test durable login refresh or enable cloud execution.
See [scope and limits](../docs/session-portability.md).

The daemon spawns `dist/chauffeur-sidecar` with piped stdin/stdout:

- JSON-RPC 2.0, one JSON object per line: daemon → sidecar on stdin, sidecar → daemon on stdout. Only protocol lines reach stdout; logs go to stderr.
- On stdin EOF (or SIGTERM/SIGINT) the sidecar stops every session and exits 0.
- `chauffeur-sidecar codex-hook pre-tool-use` is the same binary run as Codex's PreToolUse hook (see below).

Environment:

| Variable | Effect |
|---|---|
| `CHAUFFEUR_CLAUDE_PATH` | Claude Code binary to run instead of the embedded one. |
| `CHAUFFEUR_CODEX_PATH` | Codex binary to run instead of `codex` on `PATH`. |
| `CHAUFFEUR_SIDECAR_LOG` | `debug`, `info` (default), `warn`, `error`. `debug` includes the agents' stderr. |
| `CHAUFFEUR_SIDECAR_TRACE` | File to append every raw SDK message / app-server notification to (JSON lines), for debugging. |

Agents inherit the sidecar's environment plus the session's `env`.

## Protocol

Types come from `../protocol-ts` (generated from `crates/chauffeur-protocol`). Methods: `session.start|send|interrupt|respond|configure|stop`, `sidecar.ping`; notifications `session.event`; requests to the daemon `hook.worktree_create|worktree_remove`.

Behaviour worth knowing on the daemon side, for every agent:

- `session.start` resolves once the agent answered its handshake; it fails with `503` if the agent cannot start, `409` if the chat already has a session. A crashed session emits `notice` (error) and, mid-turn, `turn_ended` (error), then frees the chat for a new `session.start` with `resume_session_id`.
- Every `turn_ended` follows a `turn_started`. `text_delta` / `thinking_delta` carry chunks whose `message_id` is the one of the final `assistant_text` / `thinking`.
- `request_resolved.outcome` is `allowed`, `denied`, `answered` or `cancelled` (interrupt, stop, or a request the agent withdrew). Requests resolve before the `turn_ended` of their turn.
- `PreToolUse` on Bash denies `git worktree add` (any form: `git -C`, env prefixes, chains, `sh -c`) and names `chauffeur worktree add`, in every permission mode, bypass included.

### Claude

- `turn_started` is emitted when a message is handed to the CLI while idle, or when the CLI starts a turn on its own. Messages sent mid-turn are queued by the CLI; when one runs as its own turn, `turn_ended` is immediately followed by `turn_started`.
- `message_id` is `<API message id>:<block index>`. `cost_usd` is the SDK's `total_cost_usd` (session-cumulative). `context_tokens` is the input + output of the turn's last main-thread API call.
- `session.configure` changes model, permission mode and effort live; if the effort cannot change live, a `notice` says it applies at the next start.
- `auto` is Claude Code's auto mode: a classifier runs what it judges safe, denies the rest (the agent reads why in the tool result; a `permission_denied` from the classifier also becomes a warning `notice`, "Auto mode blocked …"), and asks through `permission_request` when it cannot decide. Where auto mode is unavailable (a model without it such as Haiku 4.5, a plan, an admin, `disableAutoMode`), Claude Code starts in `default` (asks) without an error and refuses a live switch to auto: the session then sets `default` and a warning `notice` says the chat asks instead. Never a mode that asks less.

### Codex

- `session.start` returns the thread id as `agent_session_id` right away and emits `session_started`; `resume_session_id` resumes the thread with `thread/resume` in a new app-server.
- Turns map from `turn/started` / `turn/completed` (`completed` → success, `interrupted`, `failed` → error with the API's message). `cost_usd` is null (Codex reports tokens, not money); `context_tokens` is the last request's `totalTokens`.
- Messages sent mid-turn are steered into the running turn (`turn/steer`): the model reads them at its next step, as Claude does. Only messages that arrive while a `turn/start` is in flight, or whose steer loses the race with the end of the turn, wait in a local queue for the turn's id or the next turn.
- Items: agent messages → `assistant_text` (deltas from `item/agentMessage/delta`); reasoning summaries → `thinking` (summaries are requested with `summary: "auto"`); command executions → `tool_call` / `tool_result` named `Bash` with `{command, cwd}` (the `zsh -lc` wrapper removed, output from the item); file changes → `apply_patch` with `{changes}`; MCP tool calls → `mcp__<server>__<tool>`. Tool calls an interrupt leaves open get an error `tool_result` at turn end.
- Approvals: `item/commandExecution/requestApproval` and `item/fileChange/requestApproval` → `permission_request` (`Bash` / `apply_patch`, `title` = Codex's reason). allow → `accept`, allow always → `acceptForSession`, deny → `decline` (a deny message is steered into the turn, since Codex's decline carries none), interrupt/stop → `cancel`. `item/permissions/requestApproval` → `permission_request` `permissions` (granted for the turn, or the session when always). MCP elicitations are declined; other server requests get a method-not-found error.
- Permission modes: `auto` → `on-request` + `workspace-write`, with approvals reviewed by Codex's auto review (`approvalsReviewer: "auto_review"`) instead of the user: what leaves the sandbox (a commit, network) is approved or denied by risk. Its denials (`guardianWarning`) become warning `notice`s, its approvals show nothing. Writing threads' `developerInstructions` ask the agent to rerun a command the sandbox blocked with escalated permissions, since a small model tends to give up on a failed commit instead. `bypass_permissions` → approval `never` + sandbox `danger-full-access` (the workspace-write sandbox cannot commit in a worktree: the gitdir is outside its writable roots). `default` → `on-request` + `workspace-write` (the workspace folder, `additional_directories` and /tmp writable, no network; commits ask). `accept_edits` → the same as default: Codex already applies edits inside the workspace without asking, and has no mode that asks for commands but not edits. `plan` → `on-request` + `read-only`, and each message carries a note asking for a plan (Codex's own plan mode is experimental).
- `session.configure` changes take effect from the next turn (sent with `turn/start`; a `notice` says so when a turn is running). Effort is passed as Codex's reasoning effort; `append_system_prompt` as `developerInstructions`.
- The guard: Codex has no SDK callbacks, but runs command hooks. The sidecar starts the app-server with `-c hooks.PreToolUse=[{matcher="Bash", …}]` pointing at its own binary (`codex-hook pre-tool-use`), then trusts exactly that hook for the thread (Codex only runs hooks the user trusted: `hooks/list` gives its key and hash, passed as `hooks.state` in the thread config). A blocked command produces no item: the sidecar reports it as a warning `notice` ("A command was blocked: …"). If the hook cannot be trusted, a notice says the guard is off and the workspace AGENTS.md remains the only barrier.

## Layout

```
src/main.ts               entry point: the server, or the Codex hook
src/server.ts             stdio server
src/app.ts                wires the peer, the dispatcher and the sessions
src/rpc/peer.ts           JSON-RPC over newline-delimited JSON, requests both ways
src/dispatch.ts           SidecarMethods → session manager, param validation
src/sessions.ts           one session per chat, agent registry
src/agents/agent.ts       Agent / AgentSession / SessionHost: what an agent implements
src/agents/pending.ts     permission requests and questions awaiting session.respond
src/agents/claude/        Claude: session (query + input queue), events (SDK → ChatEvent),
                          requests (canUseTool), hooks, options, executable
src/agents/codex/         Codex: session (app-server + thread), events (notifications → ChatEvent),
                          app-server process, options (modes, hook), hook, executable, generated/
src/guard/                the `git worktree add` matcher (agent-agnostic)
scripts/build.ts          compile + embed the Claude Code binary
```
