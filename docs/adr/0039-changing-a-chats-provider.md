# 0039. Changing a chat's provider with confirmation

- **Date:** 2026-10-29
- **Status:** accepted

## Context

The model picker disabled Codex once a Claude session started. Justin challenged that restriction: incompatible internal sessions require a context handoff, but do not require a separate conversation tab.

## Decision

**Changing a chat's provider (decided with Justin 2026-10-29; see docs/adr/0039-changing-a-chats-provider.md).** A normal chat can switch between Claude Code and Codex from the model picker in the same tab. A confirmation modal explains that the next message hands over the conversation as text, without the previous provider's internal session or tool outputs, and that long conversations may be shortened. Cancel changes nothing; confirm replaces the idle session and keeps the conversation, files, draft and permission mode. A running turn, background task, open request or queued message prevents the switch until finished or stopped. The transcript marks each switch; the new provider starts afresh with the selected model, effort and speed, never resumes a previous provider's session, even when switching back. Turns before the latest switch cannot be forked from an incompatible session; Fork into another provider remains available for an independent conversation. Design chats remain Claude only.

## Consequences

The daemon must validate confirmation and idleness before replacing a session. The conversation remains in its event log, and the next message uses the existing text handoff. Historical tool outputs and internal agent state are not transferred. Changing back starts a fresh session with the full available conversation rather than resuming stale context. Forks remain a separate action for parallel work.
