# 0011. Sidebar order, sections, child folding and notification sounds

- **Date:** 2026-10-16 (also 2026-10-18, 2026-10-21)
- **Status:** accepted

## Context

With many workspaces running in parallel, users need to order and file them, to fold an orchestrator's children, and to hear when one needs them (asked by users).

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, story 11:

> I order them by dragging them in the sidebar (onto the pinned ones to pin, out of them to unpin); the daemon keeps the order, a new workspace goes on top (decided with Justin 2026-10-16). I file them by theme in sections I name, between Pinned (still first) and the others (still "Workspaces", last): a "+" on hover by the "Workspaces" heading, or "Move to section › New section…" in a workspace's right-click menu, names one in place; I file a workspace by dragging it onto a section's heading or among its rows (the same drag as the order), or with "Move to section ›" (a section, "No section", "New section…"). A workspace is in one section at most; pinning is apart: a pinned workspace shows in Pinned and goes back to its section once unpinned. A section's heading shows its name and count, a chevron folds it (the daemon remembers it), and while folded a spinner or an amber dot says that one of its workspaces works or needs input; right-click to rename or delete it (its workspaces go back to "Workspaces", without asking since nothing is lost); headings are dragged to reorder the sections. Within a section the manual order holds; ⌘1–9 follow the rows shown (Pinned, the sections, folded ones skipped, then Workspaces). A new workspace still lands on top of "Workspaces", a child workspace in its parent's section. An archived workspace keeps its section and, restored, returns to it; the archived list is not sectioned. `chauffeur ls` groups by section, a header line each, in the sidebar's order (`--json`: `section_id` and `section` per workspace); `chauffeur section new|rename|move|fold|unfold|rm|ls`, `chauffeur move <workspace> <section>|--none` (decided with Justin 2026-10-18). A sound goes with the notification, if I picked one in Settings › Notifications (`[notifications]` of `settings.toml`): one of the app's own, made by the app (chime, bell, station; no file, no licence), or an audio file of mine (MP3, WAV, AIFF, M4A, OGG, FLAC, at most 5 MB), which the daemon serves to the app only as audio (asked by users; decided with Justin 2026-10-16).

From `SPEC.md`, Domain:

> Section (id, name unique in any case, place among the sections, folded; a workspace optionally in one, kept through archive and pin; deleting it files its workspaces in none; in the store beside the sidebar's order, published as a registry event; `section.list|create|rename|fold|move|delete`, `workspace.set_section`, `workspace.move` with a section) (decided with Justin 2026-10-18).

From `SPEC.md`, story 22:

> A parent with children has a chevron on its row that folds them (the app remembers it, per machine); while folded it shows how many there are, and a spinner or an amber dot when one works or needs input; ⌘1–9 skip folded children (decided with Justin 2026-10-21).

## Consequences

The order and the sections live in the daemon's store, the same in every client (`chauffeur ls` groups by section); a parent's folded children are remembered per machine by the app.
