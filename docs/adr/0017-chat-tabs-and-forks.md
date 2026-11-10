# 0017. Chat tabs and forks

- **Date:** 2026-10-18 (also 2026-10-26, 2026-10-29)
- **Status:** accepted

## Context

Users close, rename and delete chats, and fork a conversation to try another way (Fork into a new tab, asked by a user 2026-10-19, is described in `SPEC.md` story 12).

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, story 12:

> A chat's tab has a right-click menu: Rename (or double-click; empty gives back the automatic title), Close (⌘W: out of the tabs, its conversation kept, its agent session stopped; the closed chats are listed beside the tabs with their last activity and reopened from there or with ⇧⌘T) and Delete (asked first: its conversation erased from Chauffeur; the files it left in the notes folder and the agents' own transcripts stay). A working agent (a turn, tasks, queued messages, a question) is stopped only once the user agrees; closing or deleting a workspace's last open chat opens a new, empty one; an archived workspace's tabs are read-only (`chat.rename|close|reopen|delete`, `chauffeur chat rename|close|reopen|delete`, `chauffeur chats --all`) (decided with Justin 2026-10-18).

From `SPEC.md`, story 12:

> Fork from here, on the line that ends a turn the agent finished, forks the conversation as it stood then, as in Conductor: the event log up to that turn and the agent's session up to its last message (Claude Code's `forkSession` up to that message, Codex's `thread/fork` through that turn), taken from the session the turn ran in, so that a turn from before a fork or a compaction can be forked too. Offered only on turns that ended successfully since Chauffeur records where the turn ended in the agent's session (none on older ones, nor on interrupted or failed turns); allowed while the agent runs a later turn or waits for the user, since the past turn no longer changes; what the copy leaves open shows ended in the fork (its background tasks, its queue) (`chat.fork` with `turn`, `chauffeur chat fork --turn <n>`, turns counted from 1) (asked by a user 2026-10-26, decided with Justin 2026-10-26).

From `SPEC.md`, story 12:

> A fork, whole or from a turn, may go to the other agent (the tab menu's Fork into Codex / Fork into Claude Code, `chat.fork` with `agent`, `chauffeur chat fork --agent codex|claude`): neither agent resumes the other's session, so the fork copies the event log alone, takes that agent's default model, effort and speed (the user's default when it is that agent's, else its first favorite) and the same permission mode, and starts with no session; until its agent has one, each message it hands over begins with the conversation as text, rendered as for a design brief (the user's messages, the agent's texts, one line per tool call; the first message whole and as much of the end as fits about 150,000 characters), which the user's bubble does not show. Its turns from before the fork offer no Fork from here, their sessions being the other agent's, and their subagents are not read there (asked by a user 2026-10-29, decided with Justin 2026-10-29).

## Consequences

A working agent is stopped only once the user agrees. Fork from here exists only on turns whose end Chauffeur recorded in the agent's session. A fork to the other agent copies the event log alone and hands the conversation over as text until its agent has a session.
