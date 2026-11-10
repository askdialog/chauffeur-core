# 0008. Compaction through Jev

- **Date:** 2026-10-15 (also 2026-10-21)
- **Status:** accepted

## Context

Claude Code's and Codex's own compaction replace the conversation with a summary (Codex keeps only the user's messages and an encrypted summary). Jev (TypeSafe) keeps it word for word, minus the tool calls and outputs it judges no longer needed.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, story 44:

> As a developer, I want to turn on, in the app's settings (⌘,), the compaction of my Claude and Codex chats through Jev (TypeSafe), with my API key and the share of the context from which a chat compacts, so that a long chat keeps its conversation word for word rather than a summary (decided with Justin 2026-10-15: shipped to everyone, off by default; extended to Codex 2026-10-21, whose own compaction keeps only the user's messages and an encrypted summary). `/compact` compacts a chat on demand, Codex's included: through Jev when it is on, else the agent's own way (decided with Justin 2026-10-21).

## Consequences

Off by default, shipped to everyone; the TypeSafe key is kept in the system's secret store. How it runs is in `SPEC.md`, Jev compaction and Jev compaction of Codex chats.
