# Check for Updates… in the app menu brings the update card back

- **Date:** 2026-11-17
- **Status:** accepted

## Context

A user who hid the sidebar's update card (the cross hides a version until a newer one comes) had no way to update, short of waiting for the next release. Asked in #chauffeur on 2026-11-17; Justin answered that the next release would add an update from the menu.

## Decision

The macOS app menu has Check for Updates… under About Chauffeur. It asks for the newest release at once (`update.check`). When none is newer, a toast says the version is up to date. Otherwise it shows the sidebar's update card again for that version, hidden or not, and shows the sidebar if it was hidden: the update then goes through the same one step as the card (Update now, or When idle), with the same warning when agents work.

## Consequences

- No second update flow: the menu only reveals the card, so the warning, the progress and the planned restart stay in one place.
- Linux has no app menu, so it gets no such entry; the card and `chauffeur update` remain its ways.
