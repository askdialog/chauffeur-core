# File workspaces by their exact repository set

- **Date:** 2026-11-20
- **Status:** accepted

## Context

A workspace created with ⌘N landed in Workspaces even though its repositories matched a sidebar repository group. The same group accepted drops from workspaces with only some of its repositories or with additional ones.

## Decision

Asked by Justin 2026-11-20. Standard creation files a top-level workspace in a repository section when the selected set exactly matches the section's repositories. Moving a workspace into a repository section requires the same exact match. Repository order is immaterial.

## Consequences

Where multiple sections match, the first in sidebar order receives the workspace. Manual sections remain available for any set. Children follow their parent. Editing a repository list preserves existing placements.
