# 0005. Creating and naming a workspace

- **Date:** 2026-10-15 (also 2026-10-18, 2026-10-20, 2026-10-29)
- **Status:** accepted

## Context

Creating a workspace fetches every repo and runs setup scripts, which takes time, and a generated slug is a poor name for the work it holds.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, story 1:

> From the app, as in Conductor, a creation does not block: the dialog closes at once and the first chat shows the first message, with the setup going on (fetch, worktrees, config) in the conversation; the agent starts once the workspace is ready, and messages sent meanwhile wait in the queue. A creation that fails then reopens the dialog, filled in, with the error (decided with Justin 2026-10-15).

From `SPEC.md`, Disk:

> A workspace also has a title, the name shown: the one above, or, for a generated name, one drawn from its first message once sent; the user renames it at will (double-click, `chauffeur rename`), the folder and the branch keeping the slug (decided with Justin on 2026-10-15). Created from tickets, it takes the first ticket's Linear title; without a title, it takes the first ticket's once Linear reads it (attached later, refreshed). A rename by the user, or a name given at creation, is never replaced (decided with Justin 2026-10-18). A title Chauffeur chose (from the first message, or none yet, without tickets) is asked again of Claude Haiku from the workspace's conversation at the end of the turn of its 3rd message from the user, then every 10 messages (all its chats counted): the model keeps the title while it still names the work and renames only when the work turned out to be about something else; a ticket's title and the user's are never touched (decided with Justin 2026-10-20); a discreet button beside the title, shown on hover, asks again at once from the whole conversation, whoever named the workspace and with no title to keep, and the name it gives is the user's (never renamed automatically after) (decided with Justin 2026-10-29).

## Consequences

The folder and the branch keep the frozen slug; only the title changes. A title the user or a ticket gave is never replaced automatically.
