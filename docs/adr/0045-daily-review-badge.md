# 0045. Identify daily review workspaces with a badge

- **Date:** 2026-10-30
- **Status:** accepted

## Context

Daily session reviews create dedicated explorations. Justin asked for a small badge so their purpose is clear at a glance.

## Decision

Asked by Justin 2026-10-30: show a small badge identifying daily session review workspaces in the sidebar.

## Consequences

The badge uses the daemon's recorded review identity, so renaming a workspace does not remove it and an ordinary workspace with a similar name does not acquire it. Regular explorations keep their existing label.
