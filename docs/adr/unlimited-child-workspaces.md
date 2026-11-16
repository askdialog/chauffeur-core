# Child workspaces have no count limit

- **Date:** 2026-11-16
- **Status:** accepted

## Context

Chauffeur capped active child workspaces at five by default, with a configurable maximum. Justin asked to remove that limit.

## Decision

Remove the active child workspace count limit and its setting, as requested by Justin on 2026-11-16. Keep the existing restriction to one level of children.

## Consequences

A parent can create more than five active children without archiving existing ones. The settings window and protocol no longer expose `max_children`; an existing settings file containing that obsolete key still loads, and the key has no effect. Archiving a parent still archives all of its children.
