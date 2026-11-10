# 0014. Linear tickets attached after creation

- **Date:** 2026-10-16
- **Status:** accepted

## Context

After a user's feedback: a workspace's tickets change during the work, and its agents' briefing must follow.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, story 17:

> I attach, detach and refresh tickets after creation, from the header (+, × on hover, refresh; an unread ticket stands out) or `chauffeur ticket add|remove|refresh`: the briefing, the generated CLAUDE.md/AGENTS.md and the store follow, a ticket attached moves to In Progress as at creation, and idle sessions close so that their next message resumes them with the new briefing, the conversation kept; a working agent is not interrupted (decided with Justin 2026-10-16 after a user's feedback).

From `SPEC.md`, Linear:

> **Linear.** Personal API key stored in the macOS Keychain; on Linux in the Secret Service, else in `~/.config/chauffeur/linear-api-key` (mode 600). Entry points: the app's ticket picker, `--ticket`, and Linear's custom script (`~/.linear/coding-tools.json` runs `chauffeur new --ticket $LINEAR_ISSUE_IDENTIFIER`). The only write is the move to In Progress: at creation, or when a ticket is attached later (`workspace.set_tickets`), under the same setting, never in an exploration. Identifiers from clients are checked (`TEAM-123`) before they reach the generated config. `workspace.set_tickets` replaces the list and reads every ticket again (the same list refreshes); a ticket Linear does not return keeps its title and briefing, or is attached by identifier only, with a warning; a sub-issue of a ticket already attached stays only while listed. No ticket left: no `tickets.md` (decided with Justin 2026-10-16 after a user's feedback).

## Consequences

The only Linear write stays the move to In Progress, never in an exploration. Idle sessions close and resume with the new briefing; a working agent is not interrupted.
