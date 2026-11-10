# 0024. Repos the agent proposes to add

- **Date:** 2026-10-19
- **Status:** accepted

## Context

A slice can turn out to span a repo the workspace lacks; the agent should propose it rather than work around it, and only the user adds it.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, story 48:

> As a developer, I want the agent to propose adding a repo the work needs and the workspace lacks ("Add analytics to this workspace?"), added only once I say yes, so that a slice that turns out to span another repo goes on in the same workspace (decided with Justin 2026-10-19). The agent lists the organization's repos (`list_repos`: name, description, those already in; never asked) and proposes one with its reason (`add_repo`). The proposal is a card of its own: the repo, its description, the agent's reason, Add and Not now; asked each time in every permission mode (bypass included), never Always allow, and answered as any request (the app, the phone, `chauffeur respond`, a parent workspace for its children). Add adds the repo as the app does (`workspace.add_repo`: on the workspace's branch, with its files to copy and setup script; a detached checkout in an exploration) and tells the agent where; Not now tells it to go on without the repo and not to propose it again unless I bring it up. A repo already in the workspace or unknown, or a workspace not ready, is refused without asking. In workspaces and explorations, Claude and Codex; not in design chats. Once a repo is added (by the agent, the app or `chauffeur add-repo`), every chat of the workspace takes it in from its next message: its session, idle now or once its turn ends, is resumed with the repo's folder, instructions (CLAUDE.md, AGENTS.md), hooks, skills and Codex layer, as when its tickets change, and the chat says so. An agent's own `chauffeur add-repo` is refused with a pointer to the tool, as `git worktree add` is.

From `SPEC.md`, Repos the agent proposes:

> **Repos the agent proposes (story 48).** Two tools in a `chauffeur` namespace, served by the sidecar, which asks the daemon for what they need (`tool.list_repos`, `tool.add_repo`, scoped to the chat's workspace). The sidecar puts `add_repo` to the user itself, before asking the daemon, so that no permission mode, rule or Always allow can skip it: the request is a `permission_request` for the tool, titled with the repo, its input the repo, the reason and the repo's description, which the app and the phone draw as their own card. Claude gets them from an in-process MCP server (`mcp__chauffeur__list_repos`, `mcp__chauffeur__add_repo`, always loaded rather than behind tool search), allowed by the chat itself (an allow rule and the permission callback) since the tool asks. Codex gets them as dynamic tools (`dynamicTools` at `thread/start`, in Codex's experimental API: `experimentalApi` is on in a session's `initialize`), which a thread keeps when resumed; the sidecar answers its `item/tool/call` once the user did, and shows its calls as `mcp__chauffeur__*` so that the app tells no agent apart. A Codex chat started before has no such tools, and its instructions say to ask the user to add the repo from Chauffeur. The `git worktree add` guard (Claude's and Codex's `PreToolUse`) also denies `chauffeur add-repo`, naming the tool. A repo added to a workspace (the tool, the app, the CLI) closes the workspace's idle sessions and marks the busy ones to close once idle, as a change of tickets does, so that each resumes with the repo (decided with Justin 2026-10-19).

## Consequences

`add_repo` is asked every time, in every permission mode, never Always allow; an agent's own `chauffeur add-repo` is refused with a pointer to the tool.
