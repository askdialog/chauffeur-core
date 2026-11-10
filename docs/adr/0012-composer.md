# 0012. Composer: steering, permission mode, drafts and prompt suggestions

- **Date:** 2026-10-16 (also 2026-10-17, 2026-10-18, 2026-10-26)
- **Status:** accepted

## Context

The user steers agents while they work and picks settings per chat; drafts and Claude's guess at the next message should help without getting in the way.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, story 13:

> As a developer, I want to send messages while the agent works — steering it by default (it reads them at its next step, marked until then), or with ⌘⏎ into a queue I can reorder, edit and empty, sent one by one when the turn ends — interrupt it, and choose model, effort, fast mode and permission mode per chat (default: auto, Claude Code's classifier asking only for risky actions, and Codex's auto review; where auto is unavailable the chat asks; decided with Justin 2026-10-17; the picker then shows the mode Claude Code reports running in, "Ask (Auto unavailable)" with the reason on hover, and Auto again once a model change gets it back; decided with Justin 2026-10-18), from a picker listing the agents' own model catalogs with a loadout of favorites (⌃⌘1–9, ⌘⇧/ cycles effort, ⌘⇧E toggles fast), so that I steer the agent without restarting it.

From `SPEC.md`, story 13:

> What I start writing in a chat stays there as a draft when I look at another chat or workspace (its text across restarts of the app too; decided with Justin 2026-10-16).

From `SPEC.md`, story 13:

> After a turn, Claude's guess at my next message shows as grey text in the empty composer of an idle chat: Tab takes it without sending, Esc sets it aside, typing hides it only while the composer is not empty (it shows again once what was typed is erased; decided with Justin 2026-10-26), the next turn clears it; never kept in the event log nor notified, and `prompt_suggestions = false` turns it off (decided with Justin 2026-10-17).

## Consequences

The default permission mode is auto (Claude Code's classifier, Codex's auto review); where it is unavailable the chat asks. A prompt suggestion is never kept in the event log nor notified. The composer's live code preview is still proposed, awaiting Justin, and is not part of this decision.
