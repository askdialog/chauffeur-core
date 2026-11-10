# Native session stop confirmation

The sidecar now reserves a chat until its agent stop resolves. A failed stop keeps
the same session available for an explicit retry and refuses replacement. Concurrent
stops share the same confirmation; an early `host.ended` callback during that stop
cannot free the chat. A failed startup similarly attempts cleanup and retains the
session if cleanup fails. These are local in-memory reservations, not durable
workspace ownership.

## Claude

Each chat supplies the SDK's supported `spawnClaudeCodeProcess` hook. Its local
Node child keeps the SDK command, arguments, environment, working directory and
forwarded abort signal; stderr continues to the existing logger. The child exit
event is observed independently of the SDK query iterator. A spawn error without
a PID created no process; an abort error with a PID is not exit evidence.

Shutdown seals the spawn hook before awaiting anything, so an SDK callback that
arrives after stop cannot create a late process. It waits for native root exit,
escalates a lingering root to KILL and waits a further two seconds. Session stop
also requires the SDK consumption loop to finish. A timeout is an error, never a
successful acknowledgement; a retry observes the same child. Unexpected iterator
termination only releases the chat after root exit is confirmed too.

The custom hook uses plain native exit events, as allowed by the pinned SDK
contract. It does not preserve the built-in transport's optional stderr-tail grace
before emitting exit; stderr is still forwarded independently. Ordinary query
controls, native account authentication, arguments and session resumption remain
SDK-owned.

## Codex

Codex already owns its app-server process. Session stop now waits for both close
and the server's `exited` promise, within a bounded deadline. Returning from close
alone cannot release the session. Unknown outcomes remain stopping and retryable;
concurrent callers cannot receive an early acknowledgement. The existing app-server
exit promise also waits for its stdout reader, so an inherited pipe which stays
open can conservatively prevent confirmation even if the root is already gone.

## Limits before handoff

No protocol/RPC field is added and no source-stop receipt is issued. The existing
`session.stop` success now has a stronger local meaning, but absence of a session
is still a no-op, not proof that no prior process exists. Killing/restarting the
sidecar loses these reservations and launch observations. Daemon callers still
need durable launch intent, authenticated operation/generation binding and
reconciliation of lost responses before relying on this for handoff.

Root exit is not a proof about detached descendants, external MCP services,
workspace/database writers or containers. A coordinator must retain the durable
workspace admission fence, stop/reconcile every execution source and verify a
complete snapshot before committing destination execution. Phone connectivity and
timeouts never authorize a transfer.

## Validation

Tests cover an SDK iterator which refuses to end, Codex close without exit,
retained failed stops/failed startup cleanup, concurrent stop acknowledgements,
and retries. Real local child tests cover late-spawn sealing, KILL escalation and
an SDK abort signal followed by an actual root exit. Real Claude/Codex end-to-end
checks cover ordinary chat, permissions, queued delivery, context resumption,
background tasks, delegation, exploration and archive.
