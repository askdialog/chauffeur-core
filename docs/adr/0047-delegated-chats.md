# 0047. A chat started by `delegate` is a delegated chat

- **Date:** 2026-10-29
- **Status:** accepted

## Context

[0025](0025-subagent-chats.md) called the chat that an agent starts with its `delegate` tool a "subagent chat". Claude Code's own subagent (the `Agent` tool) is a different object: it runs inside its chat, has no tab and is read in a panel. Two things with almost the same name were hard to tell apart for the user, in the app, in `chauffeur chats` and in the specification.

## Decision

A chat started by an agent's `delegate` tool is called a **delegated chat**, in the app, the CLI, the messages agents read and the docs. "Subagent" alone means Claude Code's subagent. A child workspace (`chauffeur new --parent`) keeps its name. Decided with Justin 2026-10-29.

## Consequences

The tab tooltip reads "delegated by “<parent>”", `chauffeur chats` prints `(delegated by <id>)`, and the delegated chat's system prompt starts with "This chat is a delegated chat". Identifiers (`parent_chat`, `parent_tool_use`, `subagent_notice`, test ids) keep their names: renaming them touches the store and the wire, and is left for later.

[0025](0025-subagent-chats.md) keeps its text, written with the old name; its pointer to the SPEC's "Subagent chats" is now the SPEC's "Delegated chats (story 49)".
