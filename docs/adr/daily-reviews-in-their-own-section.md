# File daily reviews in a folded section of their own

- **Date:** 2026-11-06
- **Status:** accepted

## Context

Each day's session review is a workspace of its own, landing on top of "Workspaces" like any new one. Day after day they crowd the list the user works from. Justin asked for them to be grouped automatically, out of the way.

## Decision

Asked by Justin 2026-11-06: the daemon files every daily review workspace in a sidebar section "Daily reviews", which it creates folded the first time it needs it.

## Consequences

- The daemon remembers the section by its id: the user may rename, unfold, move it or take a review out of it, and it does not undo any of that. A section the user already named "Daily reviews" (any case) is used as it is.
- Deleted, the section comes back, folded, with the next review; the reviews filed in it go to no section as with any deleted section.
- Reviews made before this change are filed in it once, archived ones included; a review the user takes out later stays out.
- Nothing changes for the CLI and the app: it is an ordinary section, so its folded heading still shows a spinner or an amber dot while a review works or needs the user.
