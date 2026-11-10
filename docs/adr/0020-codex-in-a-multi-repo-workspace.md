# 0020. Codex in a multi-repo workspace

- **Date:** 2026-10-18
- **Status:** accepted

## Context

Chauffeur starts agents in the workspace folder, above the repos. Codex reads `.codex/config.toml` only from its project root down to its cwd, and its sandbox blocks Unix sockets, while agents need the `chauffeur` CLI.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, Generated workspace config:

> Codex reads `.codex/config.toml` only from its project root down to its cwd, so above the repos it would read none of theirs: the sidecar asks Codex what it loads in each repo (`config/read` with the repo's worktree as cwd) and gives the thread that repo's layer as config overrides, so that a repo's MCP servers and settings apply as they would starting Codex in the repo. Trust stays Codex's own: a repo's layer is given only when Codex enables it (the worktree or its main checkout trusted in the user's `config.toml`, or a managed config); an untrusted repo's is left out and the chat says so once, with Codex's line on how to trust it; Chauffeur never marks a repo trusted. Two repos declaring one server: the first repo's is kept whole, the other said in the chat, as for Claude's merged `.mcp.json`; any other setting, the first repo that sets it. A repo's `hooks`, `projects`, `project_root_markers` and `bypass_hook_trust` are not given (they would be trusted as the user's own). A repo's server names that repo's file as `declared_in` (decided with Justin 2026-10-18).

From `SPEC.md`, Generated workspace config:

> On macOS, a Codex chat's sandbox (every mode but Bypass, which has none) has no network, Unix sockets included, yet lets the agent's `chauffeur` commands reach the daemon's socket, and only that socket, so that `chauffeur worktree add` (where the `git worktree add` guard sends it) and the rest of the CLI work without leaving the sandbox. Codex 0.162 allows a Unix socket only through its network proxy, which it starts only for a thread that begins with network in its sandbox: the thread begins in the workspace-write sandbox with network on and the proxy (`features.network_proxy`) allowing the daemon's socket and no domain, so any other connection is refused, and its first turn sets the mode's own sandbox, the proxy staying for the thread's life. A command run outside the sandbox (approved escalation, Bypass) is not proxied. Not in an exploration, whose agent gets neither the CLI nor `CHAUFFEUR_HOME` and whose read-only sandbox keeps the socket out; nor when the user's config already gives the sandbox the network, which reaches the socket as it is (a proxy would close it). On Linux, where Codex's sandbox allows every Unix socket or none, it stays refused. Where a sandbox refuses the socket (EPERM or EACCES, the socket there), the CLI says that a sandbox blocked it and to run the command again with escalated permissions or outside the sandbox, not that the daemon is not running; Codex chats' instructions say the same (decided with Justin 2026-10-18).

## Consequences

Trust stays Codex's own: Chauffeur never marks a repo trusted. On Linux the daemon's socket stays refused inside Codex's sandbox.
