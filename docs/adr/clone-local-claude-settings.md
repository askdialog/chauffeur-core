# Workspaces read the clone's local Claude settings

- **Date:** 2026-11-17
- **Status:** accepted

## Context

Chauffeur writes a workspace's `.claude/settings.json` by merging each repo's `.claude/settings.json` and `.claude/settings.local.json` (hooks, plugins, permissions), read in the repo's worktree. The local file is gitignored, so a new worktree never has it: the permissions the user granted in their terminal (Always allow `Bash(pnpm test *)`) and their personal hooks are lost in every workspace, and the user answers the same permission prompts again. Claude Code itself, in a git worktree, reads and writes that file at the main checkout's root ([settings](https://code.claude.com/docs/en/settings), "Where Claude Code keeps the local file in a git repository"). Raised by the import from Claude Code, where it matters most.

## Decision

Proposed 2026-11-16 with the import from Claude Code, decided with Justin 2026-11-17: when a repo's worktree has no `.claude/settings.local.json`, the merge reads the one of the repo's clone instead. The worktree's own file still wins.

Asked by Justin 2026-11-17, with it:

- Allow rules that name the clone's path, in any of the repo's settings files (in practice the clone's local file, or a copy of it in the worktree), (absolute `//…`, from the home folder `~/…`, `$HOME/…`, or plain in a `Bash(…)` rule) are left out, each one reported as a workspace warning: otherwise a rule like `Edit(//Users/…/my-clone/**)` would let every workspace agent write into the clone without asking. Ask and deny rules are kept: they only restrict (first worded as allow and ask, narrowed the same day once it was clear that dropping an ask rule loosens the user's protection).
- A setting turns the reading of the clone's file off, globally or per repo, in `~/.chauffeur/settings.toml` (`clone_local_claude_settings`, and the same key in `[repos.<name>]`). A worktree's own local file is still read, and its allow rules naming the clone still left out.
- The first config write that takes a clone's local file into a workspace lists, for that workspace, the rules and hooks it adds.
- The generated settings' `$comment` names the clone's local file.

## Consequences

- It ships in steps: the merge and the filter of allow rules first; the opt-out setting, then the first-write notice, follow in their own pull requests, until which the left-out rules are warned about at every config write.
- The clone's file is only read, at every config write: its latest permissions and hooks apply from then on, nothing is copied and Chauffeur never writes it.
- The merge already wraps a repo's hooks to run in that repo's worktree, so the clone's hooks run there unchanged. Its `Read`/`Edit` paths are re-anchored at the repo like any repo rule (Claude Code anchors a local file's paths at the session's working directory, not where the file lives).
- Always allow in Chauffeur keeps writing to the workspace's own settings, never to the clone's.
- It applies to every workspace, existing ones from their next config regeneration, agents in auto mode included: permissions granted long ago in the terminal and personal hooks start applying at once. The release notes announce it. Showing in the app where each rule comes from (repo or clone) is left for later; the first-write notice stands in for it.
- A worktree's own local file only exists when `.worktreeinclude` copies it, as a snapshot of the clone's; it then replaces the clone's file, where Claude Code would apply an older starting-folder file together with the root one.
- Ask and deny rules naming the clone protect the clone only, not the worktree's copies of the same files (`.env*` copied by default): mirroring them onto the worktree is left for later.
- Hooks are kept whatever they name: a hook running a script of the clone on the worktree's files is legitimate.
- A workspace restored from the cloud reads the local file of Chauffeur's own copy of the repo, which has none.
