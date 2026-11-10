# 0013. Background tasks and subagents in a chat

- **Date:** 2026-10-16 (also 2026-10-17, 2026-10-18)
- **Status:** accepted

## Context

Agents leave commands, subagents and workflows running in the background. The user needs to see and stop them and to read a subagent's work, without them cluttering the chat.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, story 12:

> The agent's tasks running now (a command left in the background, a subagent, a workflow; not the watchers Claude Code keeps for itself) show in a bar above the composer, folded on demand: by kind, with their description, subagent type, how long they have run and their last step, each with a Stop button (`chat.stop_task`, `chauffeur stop-task`; Codex has no such task). The sidebar counts them on the workspace's row. Task progress is not kept in the event log; a task still running when its agent session ends (closed, crashed, Chauffeur restarted) is reported stopped (decided with Justin 2026-10-16).

From `SPEC.md`, story 12:

> A subagent's own tasks (commands it left running, its own subagents) sit under it in the bar, in start order, indented per level (up to three) with a guide line; one whose parent is unknown or ended stays at the top. The header counts every task, as the sidebar does; folded, the bar lists the top-level ones with "+N" for those under each. Each keeps its own Stop: stopping a subagent also stops the commands it left running (Claude Code does so), not the subagents it started (decided with Justin 2026-10-18).

From `SPEC.md`, story 12:

> Interrupting the chat ends its turn only: background tasks keep running, each stopped with its own Stop, and a background subagent's open request stays open; a subagent's last step is Claude Code's progress summary; a permission request from a subagent names it on its card (type, background, description) (decided with Justin 2026-10-17).

From `SPEC.md`, story 12:

> Clicking a subagent (its Agent call in the chat, its line in the tasks bar) opens its conversation in a panel over the right of the chat, Esc closes it: type, description, status, duration, tool uses, Stop while it runs; the prompt it got; its text, thinking and tool calls as the chat shows them; its report; a nested subagent opens there, with a breadcrumb back. Its text and thinking come live but never into the chat itself nor the event log; once it finished, or after a reload or a restart, they are read from Claude Code's transcript of the subagent (`chat.subagent_messages`, `chauffeur subagent`) (decided with Justin 2026-10-18).

From `SPEC.md`, story 12:

> A chat is working only while its agent runs a turn: a background subagent going on alone leaves it idle (for the sidebar, notifications, `wait`), its task running in the bar; its requests make the chat wait for the user, and can be answered across the agent's turns; the turn Claude Code starts when the subagent hands back is the agent working (decided with Justin 2026-10-18).

## Consequences

Task progress and a subagent's live text never enter the event log; a finished subagent is read from Claude Code's transcript. A background subagent going on alone leaves its chat idle.
