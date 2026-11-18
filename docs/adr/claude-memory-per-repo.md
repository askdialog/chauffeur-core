# Chats read the Claude memory of their repo

- **Date:** 2026-11-16
- **Status:** accepted

## Context

Claude Code keeps an auto memory per git repo, `~/.claude/projects/<repo key>/memory/`, shared by all of its worktrees; the head of its `MEMORY.md` (200 lines, then 25 KB) is loaded into every session, through the SDK too ([Claude Code's memory](https://code.claude.com/docs/en/memory#storage-location)). Chauffeur starts Claude in the workspace folder, which holds the repos but is not a git repo itself: that folder has a memory of its own (checked with a real chat on 2026-11-18). A workspace therefore does not see what the user's terminal or earlier workspaces learned, starts with an empty memory, and what it learns is never read again once it is archived.

## Decision

In V1, chats read the clone's Claude memory without being able to write it. Proposed 2026-11-16 with the import from Claude Code, then narrowed to reading by Justin on 2026-11-17.

- At every session start, the sidecar appends to the system prompt the head of the `MEMORY.md` that Claude Code keeps for the clone of the workspace's first repo (the one asked first; decided with Théo 2026-11-18), cut as Claude Code cuts it. It presents it as the user's memory, read-only, with the folder's path so that the agent reads its topic files when needed. The folder is read without asking; the agent is told never to write it, and its edit tools are refused there (a shell command is not checked).
- The workspace's own auto memory does not change: what agents write there never reaches the clone's.
- Only the first repo's memory is loaded. The system prompt gives the path of the other repos' memories, to read when needed, read without asking and refused to edits as the first's (decided with Théo 2026-11-18: first planned in the generated `CLAUDE.md`, which is only rewritten when the workspace's structure changes and would have needed a copy of Claude Code's folder key outside the sidecar). It is an accepted limit.
- A setting turns the sharing off, globally or for one repo, in `~/.chauffeur/settings.toml` (`clone_claude_memory`, and the same key in `[repos.<name>]`). A repo turned off is left out, so the first repo left is the one whose memory is loaded (decided with Théo 2026-11-18).
- `CLAUDE_CODE_PROJECT_DIR_NAME` is not used: that variable would move the transcripts too.
- Claude chats only (decided with Théo 2026-11-18): it is Claude Code's memory; Codex and Vibe chats do not get it.

## Why not writing at once

- Claude loads only the head of `MEMORY.md`. In the terminal, one or two sessions per repo write to it. With Chauffeur, dozens of often autonomous agents would push what the user put there past the limit, and nothing makes their task notes stay in `.chauffeur/`.
- Setting `autoMemoryDirectory` to the clone's folder would open reading and writing together. Blocking writes with a hook would have the agent, which Claude Code asks to save its memories, hit a refusal at every attempt. Appending to the system prompt reads without offering to write.

## Later: opening writes

Once we have seen what agents put in their workspace's memory, a new ADR can open writes by setting `autoMemoryDirectory` to the clone's folder. It needs a check first: that setting must be honoured when it comes through the SDK (its `settings` option or `--settings`, as a user setting or a launch option, not in a repo file subject to the workspace's trust).

## Consequences

- It shipped in steps: the first repo's memory in the system prompt first, then the other repos' paths, then the setting that turns it off.
- Every workspace starts with what the user, their terminal and VS Code taught Claude about the repo, always current since it is read again at every session.
- The clone's memory is meant to change only through the terminal or VS Code: a Chauffeur agent is told not to write it and cannot with its edit tools.
- What agents learn stays in their workspace's memory and is not read again after archiving, as today.
- Explorations and design chats read it like other chats.
- In a workspace with several repos, the memory of the repos after the first is not loaded up front.
- The read rules come from the sidecar: Claude Code ignores the allow rules of a folder's `.claude/settings.json` until the folder is trusted, and a workspace never is.
- The import from Claude Code has no memory to copy, and this decision no longer depends on checking `autoMemoryDirectory`.
- A user who moved their memory with `autoMemoryDirectory` is not followed there.
