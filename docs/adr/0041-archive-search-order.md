# 0041. Archived search uses the archive date

- **Date:** 2026-10-29
- **Status:** accepted

## Context

The Archived picker used sidebar order for workspaces and relevance for conversation matches. This made recently archived work hard to find.

## Decision

Asked by Justin 2026-10-29: order archived workspaces and search results chronologically by archive date, with the most recently archived workspace first.

## Consequences

Pins, manual sidebar order and parent relationships do not affect archived ordering. Missing archive dates sort last. Conversation results are sorted before the 50-result limit; relevance and message recency break ties within a workspace.
