# Add connectors from the composer

- **Date:** 2026-11-19
- **Status:** accepted

## Context

A chat can need a connector that its agent has never been configured to use. The composer already helps reconnect known MCP servers.

## Decision

Add a `+` to the composer for attaching files and adding connectors. Follow Conductor's local model: use the selected agent's native MCP configuration and authentication for Claude Code, Codex and Mistral Vibe.

## Consequences

Setup opens the agent's command in the workspace terminal and saves a remote MCP server in the agent's user settings. A Slack message link suggests the Slack endpoint. A Vibe chat also offers the agent’s own connector manager for Mistral’s Slack integration. An already started chat can be closed and reopened to load the changed configuration, keeping its draft and history. The agent handles authentication and token storage. Connector setup applies to local workspaces using hosted agents.
