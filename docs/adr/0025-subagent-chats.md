# 0025. Subagent chats through `delegate`

- **Date:** 2026-10-19
- **Status:** accepted

## Context

A review or a second opinion by another model, or a piece of the work, should not need a new workspace.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, story 49:

> As a developer, I want an agent to hand a task to the other agent (Codex from a Claude chat, Claude from a Codex chat, or its own agent) in the same workspace and get its answer back, so that a review or a second opinion by another model, or a piece of the work, needs no new workspace (decided with Justin 2026-10-19). The agent calls a `delegate` tool, like Claude's own subagents: it starts a subagent chat, a real chat of the workspace shown as a tab (marked as a subagent, its parent named on hover), whose questions and permission requests come to me there and where I may write to it; the call waits until its turn is over and returns its last message. The parent's call is a card of its own, never folded among the steps: the agent, the task, working / waiting for you / answered, its chat one click away, its answer unfolded on demand. Given the subagent chat's id again, the tool gives it more work with its context. One level: a subagent chat has no such tool. `chauffeur wait <chat>` waits for one chat as `chauffeur wait <workspace>` does for a workspace.

## Consequences

One level only: a subagent chat has no `delegate`. Its questions come to the user in its own tab. How it is served: `SPEC.md`, Subagent chats.
