# A history exploration opens its latest imported session

- **Date:** 2026-11-24
- **Status:** accepted

## Context

[claude-code-import](claude-code-import.md) imported every session left in a repo's history exploration as a closed tab. A workspace keeps one open chat, so the exploration also got a blank, untitled one: opened, the exploration showed that empty chat and none of the conversations just imported. Asked by Justin 2026-11-24, after an import whose history explorations looked empty.

## Decision

Decided with Justin 2026-11-24: when an import makes a history exploration, or finds one with no chat yet, the most recently active session it imports there is an open tab; the others stay closed tabs. A later run adding to an exploration that already has chats opens none. The open tab is imported first, so the exploration does not get a blank chat beside closed ones. The import's output says the latest is open and the others are closed tabs, with the command that lists them (`chauffeur chats <workspace> --all`).

## Consequences

- An exploration made by an import opens on a real conversation, not on an empty chat.
- A child workspace of a parent still has no chat of its own: its conversation is its parent's, which the import's plan already says (`child of <parent>`).
- Explorations an earlier import made keep their closed tabs and blank chat; the user reopens a chat with `chauffeur chat reopen`.
