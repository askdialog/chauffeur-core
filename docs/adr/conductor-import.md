# Import from Conductor in one command

- **Date:** 2026-11-06
- **Status:** accepted

## Context

Conductor Cloud turns read-only on 2026-11-13, and developers moving to Chauffeur should not lose their work in progress. V1 left a Conductor import out ([0001](0001-v1-scope.md)). In a grill session with Justin, the import's scope and how it treats Conductor's own files were settled.

## Decision

Decided with Justin 2026-11-06: `chauffeur import conductor` moves a developer from Conductor to Chauffeur in one command, without losing work in progress.

- Scope: repos and their configuration, **active workspaces**, and **chat history** as readable Markdown. Archived workspaces only with `--include-archived`. Conversations are not resumed natively: Conductor's Claude transcripts are gone from `~/.claude/projects`.
- Conductor's files are never modified, except releasing a branch. Its database is opened **read-only**.
- Each active workspace is **copied**: Chauffeur creates its own worktree on the same branch and re-applies the uncommitted changes (tracked and untracked) there. Git refuses one branch in two worktrees of a clone, so the branch is **released in Conductor**: its worktree goes to a detached HEAD on the same commit, files and changes untouched (`git switch <branch>` there to go back).
- Before anything changes, a warning lists each workspace and branch to release, and asks (default no); `--yes` skips it; `--dry-run` prints the full plan and the warning and changes nothing.
- The CLI comes first (`--dry-run`, choosing workspaces, `--include-archived`, `--yes`, `--json`); an app screen on first launch comes later. The import is a daemon method, which both call. No separate exporter tool.
- Conductor Cloud workspaces are not handled: the command says to transfer them to local in Conductor before 2026-11-13, then run the import.
- Repos are matched by GitHub remote. Chauffeur reuses Conductor's clones as sources (their folder joins Chauffeur's clone folders) instead of re-cloning; a repo Chauffeur already knows keeps Chauffeur's clone.
- Scripts typed in Conductor's UI (its database, not `.conductor/settings.toml`) go to the user's Chauffeur settings per repo, never into the repo. Conductor's environment variables go where Chauffeur keeps per-repo variables the user types; previews show names, never values.
- Chat history: one Markdown file per chat in the workspace's notes (`.chauffeur/conductor/<chat title>.md`): messages, time, agent. No new chat is started automatically.
- With a workspace come its name, its PR (found by branch), its Linear issue if Conductor had one, its notes, and its `/add-dir` folders as repos of the workspace, on the same branch name.
- Idempotent: running it again skips the workspaces already imported and says so.
- Tested against Conductor 0.93's database; an unknown schema stops with a clear message before any change.

Settled with Justin's orchestrating agent the same day, where the code or Conductor 0.93's database differed from the plan:

- Per-repo settings the user types live in `~/.chauffeur/settings.toml`: `[repos.<name>.scripts]` (`setup`, `run`, `archive`, `run_mode`, as in a repo's `settings.toml`) and `[repos.<name>.env]`. Writing in the clone (`.chauffeur/settings.local.toml`) would change Conductor's own clone.
- `[repos.<name>.env]` is given to that repo's scripts and to the agents of a workspace holding the repo. Its values never appear in logs, `--json` or the preview.
- Conductor keeps no Linear issue: the import reads a `<team>-<number>` prefix of the branch name and attaches it only when Linear is on and the issue exists, without moving its status.
- Conductor 0.93 keeps a workspace's notes in the worktree's gitignored `.context/` folder: it is copied to `.chauffeur/conductor/context/`, and the database's `notes`, when set, to `.chauffeur/conductor/notes.md`.
- When Chauffeur knows the repo through another clone than Conductor's, git does not need the branch released: the branch is fetched from Conductor's clone into Chauffeur's, and Conductor's worktree stays as it is.

## Consequences

The daemon reads Conductor's database with its own SQLite, read-only, and checks the tables and columns it uses before planning anything. A release is a `HEAD` written to the same commit in Conductor's worktree (its files and index untouched); a failed creation puts the branch back. The uncommitted changes are carried as a binary diff against `HEAD` plus a copy of the untracked files, so what was staged arrives unstaged. Each imported Conductor workspace is remembered by its Conductor id, so a second run skips it even after the Chauffeur workspace is archived. An archived Conductor workspace becomes an archived Chauffeur workspace with its chat history only, which cannot be restored. The generated `CLAUDE.md` and `AGENTS.md` point agents at `.chauffeur/conductor/`. The app screen is left for later.
