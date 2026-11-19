# Auto uses native Smart Approve in Vibe workspace chats

- **Date:** 2026-11-19
- **Status:** accepted

## Context

Chauffeur asked for every new Bash command in Vibe's Auto mode. Vibe can classify tool calls itself and reports their execution over ACP.

## Decision

Decided with Justin on 2026-11-19. Auto in an ordinary Vibe workspace chat selects Smart Approve when Vibe offers it. Other modes, explorations and Design continue to use Vibe's Ask mode with Chauffeur's own permission rules. A call already approved by Vibe can run without a Chauffeur permission request.

Chauffeur records Bash calls from ACP. It tracks successful direct worktree creation when the command identifies a literal target and Git confirms a linked checkout of a managed repository inside the workspace. The worktree is snapshotted and removed through the usual archive path. If a worktree cannot be attributed, Chauffeur reports it and leaves it to the user. Before archive, Chauffeur inventories the managed repositories again. It adopts any linked checkouts still inside the workspace and snapshots nested worktrees before removing their parent.

## Consequences

A checkout outside the workspace is reported for manual cleanup. A shell command can call a script whose internal Git commands do not appear in the ACP stream. Smart Approve may still ask for confirmation, and Chauffeur falls back to its prior Auto policy if Vibe does not offer the native mode.
