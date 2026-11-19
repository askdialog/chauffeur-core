# Create workspaces from a repository or group in the sidebar

- **Date:** 2026-11-19
- **Status:** accepted

## Context

Stanislas often works in one repository at a time and wants to launch a workspace from any of his regular repositories. Justin switches between projects whose workspaces usually need the same set of repositories. The global default set covers only one such combination.

## Decision

Asked by Justin 2026-11-19. A repository can be added directly to the sidebar as a section named after it. A named repository group appears alongside it and stores an ordered list of default repositories. A repository can appear on its own and in any number of groups.

The plus on either heading creates a workspace in that section with its default repositories and opens an empty chat. The agent starts when the user sends a message. The selected repositories become the workspace's own worktrees.

## Consequences

Changing a section's default repositories affects future workspaces. A workspace stays in its section when its repositories change. Users can still move workspaces between sections, use sections with no default repositories, and open the standard creation form with `⌘N`.

The Workspaces heading shows the buttons for adding a repository or group and for creating a manual section. A workspace's actions menu opens from its ellipsis button, visible on the selected row and on hover or focus. The Move to section menu identifies repository sections, groups and manual sections by their icons and marks the current section.
