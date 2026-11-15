# Review conversations are read from Reviews, not listed in the sidebar

- **Date:** 2026-11-15
- **Status:** accepted

## Context

Reviews (see [reviews-interface](reviews-interface.md)) lists every daily and weekly report in its Report history, archived ones included, with their state and a link to the conversation. The "Daily reviews" sidebar section listed the same conversations a second time, one row each, and their number grew every day. Justin saw the duplicate and asked for the conversations to leave the sidebar.

## Decision

Asked by Justin and decided 2026-11-15: the sidebar no longer lists the daily and weekly review conversations. The "Daily reviews" section keeps its Reviews entry, whatever its name or folded state. Report history is the only list of the reports, and opens each conversation as before.

## Consequences

- The conversations are unchanged: they stay workspaces of their own, filed in the section, and the daemon, the CLI (`chauffeur ls`), the switcher and the archive still list them. Only the sidebar's rows, and so the ⌘1–9 shortcuts, leave out a workspace with a review date.
- A section that no longer shows rows can no longer signal a running review on its folded heading: the Reviews entry shows a spinner while a review works and an amber dot when one needs the user (a permission or a question), as a folded section would.
- This replaces only the part of [daily-reviews-in-their-own-section](daily-reviews-in-their-own-section.md) that listed the reviews as rows; the section, its creation and its renaming rules stand.
