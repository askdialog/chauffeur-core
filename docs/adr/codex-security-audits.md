# Codex Security audits in a workspace

- **Date:** 2026-11-23
- **Status:** accepted

## Context

A Deep audit can outlive the Codex chat turn that launched it. Its workers
belong to Codex Security, while the workspace only showed Chauffeur's own
chat and process activity.

## Decision

A workspace with Codex Security audits shows a persistent card with their
phase, available progress, last activity, and report. Cancellation is
requested in the original Codex chat.

## Consequences

The desktop daemon reads the local Codex Security workbench in read-only
mode and scopes the projection to the workspace's worktrees. It displays
recorded progress and the coordinator's last heartbeat where available.
A missing or stale heartbeat never implies a terminal result. The card
uses the workbench's status to confirm completion or cancellation. Local
reports stay accessible inside Chauffeur through user-only desktop calls.
