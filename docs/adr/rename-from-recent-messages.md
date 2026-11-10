# Rename a workspace from recent messages

- **Date:** 2026-11-06
- **Status:** accepted

## Context

Justin asked for an action in the workspace context menu to name the workspace automatically from its N latest messages.

## Decision

Add an auto-rename action to the workspace context menu, using the N latest user and assistant messages across its chats.

## Consequences

The action uses the existing title generation and rename flow. The display title changes; the folder and branch retain their names. Tool output and thinking are excluded from the recent messages.
