# 0027. Cloud workspace handoff scope

- **Date:** 2026-10-19
- **Status:** accepted

## Context

Agents should keep working while the Mac sleeps or travels: a workspace moves to a Linux machine and back, on request.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, Cloud workspace follow-up (approved 2026-10-19):

> The manual Mac↔Linux workspace handoff scope is defined in
> [Cloud workspace handoff](../cloud-workspaces.md). It is delivered through PRs,
> without Linear. The first slice is an isolated native session portability proof;
> cloud execution is not yet available to real workspaces. No automatic failover,
> silent summary replacement or concurrent source/destination execution.

## Consequences

The scope itself is [`docs/cloud-workspaces.md`](../cloud-workspaces.md); the architecture as built is [`docs/cloud/README.md`](../cloud/README.md). No automatic failover: a transfer is always the user's.
