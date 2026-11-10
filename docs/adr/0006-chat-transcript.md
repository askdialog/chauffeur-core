# 0006. What a chat's transcript shows

- **Date:** 2026-10-15 (also 2026-10-16, 2026-10-17, 2026-10-18, 2026-10-27, 2026-10-28, 2026-11-05)
- **Status:** accepted

## Context

An agent produces far more tool calls and thinking than messages. The transcript must read as a conversation, stay responsive when long, and let the user take an answer elsewhere.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, story 12:

> The tool calls and thinking between two of the agent's messages fold into one line (how many calls, by tool, how many failed), opened on demand; while the agent works, the step it is on shows under it; the user's own messages are bubbles on the right, apart from the agent's work (decided with Justin 2026-10-15).

From `SPEC.md`, story 12:

> While Claude Code's auto mode keeps checking a tool call past a few seconds (`permission_check_status`), the call's row, in the chat and in a subagent's panel, says "Auto mode is checking this call…"; it goes when the check ends, the call gets its result, is denied or put to the user, or the turn ends or is interrupted; never kept in the event log (`permission_check`) (decided with Justin 2026-10-18).

From `SPEC.md`, story 12:

> A ```mermaid block in an answer is drawn as a diagram (its code a click away, full screen to zoom), as Claude's own apps do (decided with Justin 2026-10-16).

From `SPEC.md`, story 12:

> A long conversation opens on its end: the app shows each chat's last 300 events as soon as they arrive, and a "Loading earlier messages…" note stays over the list while the rest is read apart, in short slices so that the page keeps answering, and put under it in one go (a reader at the bottom stays there, one who scrolled up stays on the message they were on); until then the cost and Fork from here, which count from the first turn, wait, and a workspace moved to another machine loads whole behind "Loading the conversation…" (asked by Justin 2026-10-27).

From `SPEC.md`, story 12:

> A thin rail on the chat's right edge marks each of the user's messages where it sits in the conversation: hovering a mark previews the message, clicking it scrolls back there, as in Codex (decided with Justin 2026-10-17).

From `SPEC.md`, story 12:

> A ring after the cost in the composer shows how full the context window is and where the chat compacts (Jev's threshold, else Claude Code's autocompact); hovering it opens Claude Code's `/context` breakdown, counted exactly on opening; the reading taken after each turn is kept, so a closed session shows its last one (`chauffeur context`; Codex gives only its total) (decided with Justin 2026-10-18).

From `SPEC.md`, Copy an agent response:

> **Copy an agent response (decided with Justin 2026-10-27).** Each completed, non-empty assistant message has a discreet copy icon below it, with an accessible label and tooltip. One click offers the Markdown source as `text/plain` and portable rendered Markdown as `text/html` in the same clipboard item, without the chat's interactive controls. The receiving application chooses its format; retaining tables in Slack is not guaranteed. A webview that cannot write rich text falls back to the same plain text. In the plain text, each heading is written as a bold line (`**Title**`, its closing `#`s dropped, never `****`) and the rest of the Markdown is unchanged byte for byte; the HTML is rendered from the source as written (decided with Justin 2026-11-05: when the plain text is Markdown, Slack ignores the HTML and converts the Markdown itself, and drops the blank line under a heading but keeps it under a bold line; the existing Copy changes, there is no separate Copy for Slack). The button confirms success briefly and reports a failed copy.

From `SPEC.md`, Copy a table:

> **Copy a table (decided with Justin 2026-10-28).** Each rendered Markdown table has a discreet copy icon beside its top-right corner, shown on hover or keyboard focus (always visible on touch screens). It copies just that table as tab-separated plain text, including its header and without Markdown syntax, for pasting into Slack or a spreadsheet. Quotes, tabs and line breaks inside cells use spreadsheet quoting; empty cells keep their columns. The response's Copy button still offers the full Markdown and HTML.

## Consequences

Ephemeral state (an auto-mode check) never enters the event log. A long history is read tail first, the rest apart. A copied answer carries both its Markdown, headings as bold lines, and its HTML; a table copies as tab-separated text.
