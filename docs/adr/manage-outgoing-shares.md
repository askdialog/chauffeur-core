# Manage outgoing shares and keep archived designs shared

- **Date:** 2026-11-10
- **Status:** accepted

## Context

Justin wants one place to see the designs, workspaces and files he has shared, open their links and stop sharing. He also asks that archiving a design chat leave its share active.

## Decision

An app navigation page lists the outgoing shares managed by this daemon, across workspaces and including archived design sources. Each row identifies its kind, name, workspace, archived state and connection, with actions to open or copy its link and stop sharing. A failed stop leaves the row visible and reports the error.

Archiving a design chat preserves its hosted share and its files. Archiving its parent workspace also preserves that design share; the daemon resumes it from the archived notes. Explicit Stop sharing or permanent deletion ends access. Workspace shares and standalone file shares retain their existing stop-on-workspace-archive behaviour.

## Consequences

The list uses the existing stored share records and stop operations, rather than creating another sharing registry. A link being active is distinct from its source being online. The page does not claim to inventory links owned by other people or managed by another daemon.

Archiving remains an organizational action for designs; it is not a way to revoke their reviewers' access. The archived design's share remains discoverable and revocable on the Sharing page.

While the parent workspace is archived, the owner can read and answer reviewers on the hosted page; restoring the workspace is required to use its local canvas and comment panel. Archived or cleanup-failed workspaces resolve design shares from their archived notes even if an empty live notes folder remains. The inventory RPC is available to the user, not workspace agents.
