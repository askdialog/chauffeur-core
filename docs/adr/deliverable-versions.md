# Deliverables keep their versions

- **Date:** 2026-11-14
- **Status:** accepted

## Context

[chat-document-deliverables](chat-document-deliverables.md) introduced no version history: a revision replaced the document. With decks edited both by the agent and by hand ([slide-decks](slide-decks.md)), a bad turn or a wrong edit must be undoable, as Claude Slides' version selector allows.

## Decision

Decided with Justin 2026-11-14: every deliverable (HTML, PDF, deck) keeps its versions. A version is taken at the end of each turn that changed the deliverable, and after each change by hand. The reader has a version selector; opening an older version shows it read-only, and "Restore" copies it back as a new version, so nothing is ever lost.

This supersedes the "no version history" part of [chat-document-deliverables](chat-document-deliverables.md).

## Consequences

- The daemon takes the versions in `.chauffeur/artifacts/<deliverable>/.versions/<n>/`, a copy of the deliverable's files with its exports; they are kept with the notes when the workspace is archived.
- Downloads and shares always use the current version.
- No pruning for now; a limit is left for later if the notes grow too large.
