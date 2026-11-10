# 0001. Chauffeur V1: one workspace across several repos

- **Date:** 2026-10-13 (Linux: 2026-10-14)
- **Status:** accepted

## Context

Developers ran Claude Code and Codex in Conductor, where a workspace is one worktree of one repo. Multi-repo tickets went through `/add-dir` on the main checkouts, without isolation, the worktrees agents created were never cleaned (tens of gigabytes on one machine), and local workspaces had no API. A grill-project session with Justin settled what Chauffeur V1 is.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, opening:

> Settled with Justin on 2026-10-13 (grill-project session). No Linear project for this one: this file is the source of truth. Prior art behind it: [`docs/prior-art.md`](../prior-art.md).

From `SPEC.md`, Solution:

> Chauffeur: a desktop app (macOS, and Linux since 2026-10-14) and a CLI on top of one local daemon. A workspace spans the repos you pick, one worktree each on one branch, optionally tied to Linear tickets; Claude Code and Codex chats run in it, and agents create worktrees and child workspaces only through Chauffeur. Archiving a workspace kills and removes everything it created.

From `SPEC.md`, Out of scope for V1:

> - Apple signing, notarization, silent auto-update: no Apple Developer account at the time. Colleagues build from source or get an ad-hoc signed disk image (`scripts/package.sh`) that they approve once in System Settings; later versions install from the app when the user asks (story 50).
> - Review features: diff comments sent to the agent, per-turn checkpoints and rewind, GitHub checks and review threads in the app. Next project.
> - Mobile app and remote access: now started as the follow-up above; not part of V1.
> - A data fork per workspace for a repo with a database (a Neon branch per workspace, say): a separate dev-env project. Chauffeur only provides ids, ports and guaranteed archive scripts.
> - Agents other than Claude Code and Codex.
> - In design chats (story 47): drawing tools, versions of a screen, PDF export, Codex. Later, once design chats are used. Comments and shares are story 51.
> - Cloud workspaces, speech-to-text, workspace templates. Multiplayer beyond the Office and design shares (story 51).
> - Importing Conductor workspaces. Linear writes beyond In Progress.

From `SPEC.md`, Notes, Rejected options:

> - Keep Conductor and script companion worktrees: there is no local API, `/add-dir` stays manual, and there is no multi-repo diff or PR.
> - Agent of Empires: a tmux UX.
> - Forking Vibe Kanban: its company shut down in April 2026 and it is barely maintained. Read it as a reference only; it has the same multi-repo model.
> - A GPUI UI: GPUI is pre-1.0, and nothing could be shared with a React Native app.
> - Driving Claude's stream-json control protocol from Rust: partly documented, changes weekly.
> - ACP: lossy for Claude (no custom permission callback or hooks).
> - Agent cwd in one primary repo with `--add-dir`: hooks and settings would load only for the primary repo.

## Consequences

`SPEC.md` is the source of truth for V1's behaviour; there is no Linear project. An item of the out-of-scope list needs a decision of its own before it is built; several were taken up later: the mobile companion ([0023](0023-mobile-companion.md)), cloud workspaces ([0027](0027-cloud-workspace-handoff-scope.md)), design comments and shares ([0029](0029-design-comments-and-shares.md)).
