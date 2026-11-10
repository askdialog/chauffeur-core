# 0053. Create a design from Command N

- **Date:** 2026-10-30
- **Status:** accepted

## Context

Command N opened a workspace or exploration, but could not start a design directly.
Creating a design required finding the chat menu in an existing workspace.

## Decision

Asked by Justin on 2026-10-30: the creation dialog opened by Command N offers Design
beside Workspace and Explore, and opens the new design's chat and canvas directly.
The first design prompt is optional. Design chats retain their existing Claude-only
and code-write protections.

## Consequences

The new design uses an exploration for its repository context, without code setup
scripts or a branch to implement changes. The daemon creates the first design chat
even without a prompt. Selecting an existing design as development context remains
a separate workspace creation action.
