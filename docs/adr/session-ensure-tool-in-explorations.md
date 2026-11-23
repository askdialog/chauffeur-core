# An exploration's agent ensures a declared login through a tool, not the CLI

- **Date:** 2026-11-23
- **Status:** accepted

## Context

Repos tell their agents to run `chauffeur session ensure <name>` before work that needs a login ([declared-sessions](declared-sessions.md)). An exploration's agent cannot. It has no `chauffeur` CLI and no `CHAUFFEUR_HOME`, and its sandbox keeps the daemon's socket out ([0002](0002-explorations.md)). So the command fails with "a sandbox blocked the connection to the Chauffeur daemon's socket". Escalating is not an option either: the exploration sandbox never retries outside it. An exploration that needed AWS to answer a question was stuck (Justin, 2026-11-23).

## Decision

Decided with Justin 2026-11-23:

- An exploration's agent gets a `session_ensure` tool beside `list_repos`, `add_repo` and `delegate`. The sidecar serves it the way it serves those tools: Claude's in-process `chauffeur` MCP server, Codex's dynamic tools. The daemon's socket stays closed to the exploration's commands, and the exploration still gets no CLI.
- The tool takes a session's name and does what `chauffeur session ensure <name>` does: `session.ensure` for the chat's own workspace, its request asked in the calling chat. It returns at once while the session holds. Otherwise it waits for the user to log in, and fails with the daemon's reason: declined, not answered within 15 minutes, or an error such as an unknown session. It reaches only the sessions the chat's workspace declares (its repos' and the user's own), never another workspace's. The agent never answers the request itself.
- The exploration's generated instructions tell its agent to use this tool wherever a repo's instructions say `chauffeur session ensure`. Workspace chats keep the CLI and do not get the tool.
- Design chats get neither: they have no Chauffeur tools at all, and their work (screens in their own folder) needs no declared login.

## Consequences

- The sidecar protocol gains `tool.session_ensure` (`{chat_id, name}` → `LoginSessionInfo`). The daemon refuses it in a design chat. A Claude exploration allows the tool by rule, as it does Chauffeur's other tools.
- An exploration's agent can now make the daemon run a session's `check`, and show the user a login card. That is all it gains: `check` and `login` are commands the repo or the user declared, and only the user starts a login. The rest of the daemon stays out of reach.
- An interrupt ends the tool's wait, but the request stays open in the chat until the user answers it or it times out, as with the CLI.
- A Codex thread keeps the tools it started with: an exploration thread started before this change gets the tool only in a new thread.
- Not done: a Vibe exploration. Vibe cancels a tool call after 60 s, so the tool needs the follow-up message `add_repo` uses there. Until then its agent asks the user to log in from the Processes panel, as its instructions say.
