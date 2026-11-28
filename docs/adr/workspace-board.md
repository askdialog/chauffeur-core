# Each workspace has a board its agent keeps: steps and a note

- **Date:** 2026-11-28
- **Status:** proposed, awaiting Justin

## Context

An agent often lays out work that spans several turns, chats and days: "ticket 1, then ticket 2, test on staging, then ticket 3". It says so once, in the conversation, and that plan scrolls out of sight: coming back to a workspace, the user cannot see where the work stands without reading the chat back. The plans Chauffeur already reads (Claude's `update_plan`, Codex's plan, `TodoWrite`) do not answer it: they are one chat's checklist for its current turn, rewritten all the time, and only their count is kept (`plan_progress.rs`, shown in the Office). Asked by a user 2026-11-28.

## Decision

Proposed by a user 2026-11-28, awaiting Justin.

**What a board is.** Every workspace has one board, shared by all its chats; explorations and design chats included, and a child workspace has its own. A board holds two things:

- **Steps**, in order. Each step has a text (one line), a status (`pending`, `in_progress`, `done`, `blocked`), the times the daemon records as the status changes (created, started, done), and an optional planned day, given by the agent or the user. Several steps may be in progress at once.
- **A note**, free markdown, for what is not a step: context, decisions, links, what to watch. No HTML for now.

**Where it is kept.** The daemon writes it, as every write: `.chauffeur/board.json` in the workspace's notes folder, as `comments.json` is, so that it is kept on archive and carried by transfers and shares as the notes are. An agent's own Write or Edit of that file is refused with a pointer to the tools, as its own `chauffeur add-repo` is.

**The agent's tools.** Two tools in the `chauffeur` namespace, served by the sidecar, which asks the daemon (`tool.read_board`, `tool.update_board`, scoped to the chat's workspace): for Claude from the in-process MCP server, always loaded; for Codex as dynamic tools, as `add_repo` is. Never put to the user, in any permission mode.

- `read_board`: the board, each step with its id.
- `update_board`: one or several operations, each on one step by its id (add with its text, position and planned day; edit its text or planned day; set its status; move; remove), or the note replaced whole. It answers with the board as it now is.

Each operation touches one step, so that two chats of a workspace, or the agent and the user, never overwrite each other's steps. The note is replaced whole: a replacement made over a version the caller has not read is refused, with the current note.

**When the agent keeps it.** The instructions of every chat of a workspace say: when the work spans several turns (several tickets, a deployment to test, a step waiting on someone), put its steps on the board, mark each one as it starts and ends, add the steps that appear on the way; read the board when starting or taking up work. Not for a task of one turn: `update_plan`, Codex's plan and `TodoWrite` stay the turn's checklist, unchanged.

**What the user sees and does.** A Board tab in the workspace view, beside Chat, Diff and Files and movable to the side as they are: the steps with their status and dates, the note rendered. The user ticks, adds, edits, reorders and removes steps and edits the note; each chat's agent is told what the user changed at its next turn, as it is told of a deck's slides changed by hand. The tab's badge and the workspace's row in the sidebar show `done/total` while the board has steps. An archived workspace's board is read-only. `chauffeur board` prints the board.

**Kept in mind, not built now.** A view of every workspace's board in time order, possibly with routines' next runs and Linear milestones' target dates. Whether it would be a log (by done time), an agenda (by planned day) or a queue the user orders is not decided. The times the daemon records and the planned day are kept so that such a view can be built without changing boards.

## Consequences

- New protocol methods: `board.get` and `board.update` for the app and the CLI, `tool.read_board` and `tool.update_board` for the sidecar, and a `board_updated` event of the workspace. New: the Board tab and its badge, the count on the sidebar row, the guard on `.chauffeur/board.json`, a paragraph in every workspace chat's instructions.
- An exploration's chats write nothing but their plan; the board is written by the daemon through the tools, so they keep one.
- Bounded: 200 steps, 500 characters a step, 100 KB of note; beyond, the tool refuses and names the limit.
- A Codex chat started before has no board tools, and its instructions do not mention the board.
- Not for now: the board in ⌘K's search, on the phone, in the Office. `ChatProgress` and the Office's progress stay as they are.
- `SPEC.md` describes the board once it is built.
