# Prior art

Tools looked at before the V1 spec (2026-10), and what they showed about driving coding agents
across several repositories.

## Comparable tools

- **Conductor** (macOS, closed source): Claude Code and Codex chats in git worktrees, one
  repository per workspace; other directories only through `/add-dir`, pointing at shared
  checkouts rather than isolated worktrees.
- **Vibe Kanban** (Rust, React, Tauri; Apache-2.0): multi-repository workspaces as sibling
  worktrees on a shared branch, with a generated root `CLAUDE.md`/`AGENTS.md` importing each
  repository's.
- **Agent of Empires** (Rust, MIT): tmux and web UI; sibling worktrees on a shared branch.
- **Jean** (Tauri, Rust, React; Apache-2.0): Linear integration, web UI, processes that survive a
  restart; other repositories only as linked context.
- Others: Superset, Emdash, Nimbalyst, Happy, Orca, Claude Squad, Sculptor, Zed (ACP, multi-root).

## Driving the agents

- Claude Code has no official Rust SDK: Chauffeur runs the unmodified `claude` binary with the
  user's own subscription, through its stream-json control protocol, and never handles tokens.
- Codex is driven through `codex app-server` (JSON-RPC over stdio).
- Claude Code loads `CLAUDE.md` from the working directory and its ancestors, and project
  settings only from the working directory's `.claude/`; Codex walks `AGENTS.md` up to the git
  root. A workspace that spans several repositories therefore needs a workspace-level
  configuration: a root `CLAUDE.md`/`AGENTS.md` importing each repository's, merged settings and
  hooks.
