# 0007. Resuming cut turns and closing idle sessions

- **Date:** 2026-10-15 (also 2026-10-23, 2026-10-25)
- **Status:** accepted

## Context

Closing the app must never kill work, a daemon or sidecar restart must not leave turns half done, and each idle agent session holds about 200 MB of memory.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, story 15:

> As a developer, I want chats to keep running when I close the app and to resume after a daemon or sidecar restart, so that closing a window never kills work. A chat whose turn a daemon restart (a crash, a kill or a stop) or a sidecar crash cut resumes it on its own: the user reads that it resumes, the agent that its commands and background tasks were killed and its open questions went unanswered, to check what they left before running anything again. Once only: cut again before that turn ends, it waits for a message, lest a turn that crashes loop. A chat waiting for the user (a question, a permission) waits for a message, as does every chat with `resume_interrupted_turns = false` (decided with Justin 2026-10-23). A turn that ended on an error trying again could cure (the API unreachable: no network, DNS; overloaded or failing on its side; never a login, a quota or a budget) resumes when the user is back, however late: the app, on connecting and each time its window takes the focus, calls `chat.resume_failed`, and each idle chat whose last turn failed so, with no message of the user's since, resumes it, told what failed and how long ago, to check what it left before running anything again. Once per message of the user's: failing again, the chat waits for a message, lest a network still down loop; `resume_interrupted_turns = false` turns it off too. The daemon never does it on its own when the network returns, lest agents spend tokens while the user is away (decided with Justin 2026-10-25). A chat left alone for `idle_session_minutes` (30) has its agent session closed to free its memory (about 200 MB), unless a background task of the agent still runs; the next message resumes it, about 1.3 s slower. A workspace left alone that long has its Nx daemons stopped when nothing else runs in it; the next `nx` command starts one (decided with Justin 2026-10-15).

## Consequences

A cut turn resumes once, a failed turn resumes once per message of the user's and only when the user is back; the daemon never resumes on its own when the network returns. `resume_interrupted_turns = false` turns both off; `idle_session_minutes` (30) sets when idle sessions close.
