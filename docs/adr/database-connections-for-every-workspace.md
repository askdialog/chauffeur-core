# Database connections belong to Chauffeur, not to a workspace

- **Date:** 2026-11-18
- **Status:** accepted

## Context

Connections labelled local were saved for the current workspace only: the same database had to be added again in each workspace, and a remote database labelled local stayed out of the others. The environment (local, staging, production) and where a connection is available had been tied together. Justin asked for a single registry of connections in Chauffeur.

## Decision

Decided with Justin 2026-11-18: every saved PostgreSQL connection is available in all workspaces on this machine, whatever its environment. A workspace keeps only which connection it shows and its open tabs; it owns no connection. Connections an earlier version saved for a single workspace become available everywhere, merged into an identical one when it exists. The connection form warns when a connection labelled local points at a host that is not this machine, since production's read-only default would not apply to it.

## Consequences

The connection form no longer asks where a connection is available. The `workspace` field of `database.save` and `database.list` stays on the wire for older clients and is ignored; a later protocol change may remove it. Queries, schema, history and edits still name a workspace: its admission and read-only state (explorations, inactive workspaces) still apply. A database started by a workspace's own stack, on that workspace's ports, could later be declared by its repo and shown in that workspace only; that is left for later.
