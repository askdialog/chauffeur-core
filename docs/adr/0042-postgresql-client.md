# 0042. PostgreSQL inside Chauffeur

- **Date:** 2026-10-29
- **Status:** accepted

## Context

Justin asked for a database client inside Chauffeur, in place of a separate one such as TablePlus, similar to IntelliJ's SQL client, alongside a workspace's code and chats.

## Decision

Asked and approved by Justin on 2026-10-29: support PostgreSQL only. A Database workspace view contains a schema/table tree and tabs for table data and SQL consoles. Table browsing includes filtering, sorting, pagination, cell inspection and staged edits applied explicitly. SQL consoles provide schema completion, history and copyable results.

Connections include local databases attached to a workspace and manually configured staging, production or other databases available across workspaces. The user supplies credentials. Connection settings stay on the machine and passwords use its protected secret store. The environment is visibly identified; production connections default to read-only, explicitly disabled in their settings.

## Consequences

The daemon owns connections, SQL execution, credentials and history. Database methods are available to the user's app and CLI, not to workspace agents or shared-workspace guests. An explicit Add to chat action copies selected context into the user's draft.

The first implementation connects to supplied TCP hosts and ports, with system-verified TLS or explicit TLS disablement for local development. It does not manage SSH tunnels or automatically infer passwords from repositories. PostgreSQL enforces read-only transactions; explorations and inactive workspaces always use them.

Consoles execute one statement or selected statement at a time on a fresh connection, committing it on success. Manual transaction sessions, additional engines and advanced administration are outside this slice. Results and history are bounded. Grid edits require a primary key and compare original values, rolling back the complete batch when any row changed.
