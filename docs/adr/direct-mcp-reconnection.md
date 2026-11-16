# Reconnect MCP servers directly from their warning

- **Date:** 2026-11-16
- **Status:** accepted

## Context

The usage panel reports broken MCP servers but only links to a chat, where another panel holds the recovery action. Idle sessions can keep an outdated reading, and a pending server can be announced as connected. Justin asked for the direct recovery experience of Claude Code and approved this improvement.

## Decision

Decided with Justin 2026-11-16: offer Log in or Reconnect directly in the usage warning, sharing the composer's action and its in-progress state. Confirm success only when the session reports `connected`. Keep the composer's panel open to show that result. Follow native MCP status during inactivity as well as after turns and failed tool calls, with more frequent readings during connection attempts. Reconnection remains specific to the selected chat's session. Missing environment variables keep their existing guide.

## Consequences

Running sessions read native status every 30 seconds; pending servers are read every three seconds for their first minute, and explicit recovery attempts every three seconds for up to five minutes. A status read failure preserves the last reading and retries. Opening an OAuth page does not finish the attempt. Duplicate clicks across panels are blocked until confirmation, failure or a five-minute timeout. Chauffeur keeps using the agents' own authentication and reconnection controls.
