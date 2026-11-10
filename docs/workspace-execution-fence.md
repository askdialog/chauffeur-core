# Local workspace execution admission fence

`workspace_execution.rs` adds one shared admission gate per daemon. Ordinary local
workspaces have no fence. Trusted future handoff code can install a validated
`stopping` journal state; no client RPC, timer or network event installs one yet.
Callers must authenticate the controller and the exact source/operation/generation
before installation or release. The journal state alone is not authentication.

## Guarded paths

- Chat creation and message delivery, including existing sessions and direct
  session starts from workspace-ready/environment/briefing paths.
- Request responses, configuration, forks and MCP connection requests.
- Explicit and automatic queued-message delivery; admission precedes removal
  from the queue and continues through delivery/session registration.
- Managed setup/run/archive script launch through `ProcessManager::spawn`.
- Terminal creation and input to an existing terminal.
- Edits of the workspace and its chats a transfer package carries: workspace rename,
  tickets, added repos, agent worktrees added or removed, promotion and archive; chat queue edits, rename, close,
  reopen, deletion and design-brief regeneration. Archive holds its admission until
  it ends, so a fence cannot settle under an archive in progress. Closing or deleting
the last open chat opens a new one under the same admission, never refused halfway. Pin and section
  stay usable: they are this Mac's own sidebar layout.

Stops, interrupts, terminal close and observational APIs remain usable. The guard
does not fence every filesystem/registry write. Files/editor operations, workspace
mutation/archive orchestration, external programs and services need coordinated
handling before a handoff can safely capture a complete snapshot.

## Persistence and admission ordering

A nullable `execution_fence` column belongs to the workspace row in the main
trusted local database. Existing rows migrate to unfenced. After a cancellation,
the column retains the cancelled operation/revision while admitting local work.
A delayed old stopping message cannot erase that decision; a new stop must use
a fresh operation and a later revision with the same source generation/binding. Missing workspace
metadata is an error, not an unfenced workspace. Malformed or mismatched fence
metadata fails closed. The local field is not reconstructed from cloud listings,
ordinary snapshot contents or phone connectivity.

Installation and admission share a mutex within the single active daemon. A fence
write uses EXTRA synchronization for its acknowledged WAL commit, then restores
the connection's ordinary policy. Database errors never clear the fence. Later
admissions consult persisted state and fail; admission tokens already issued stay
alive through launch/send/registration completion. The wait subscribes before
checking counters to avoid a lost wakeup. A timeout/drop of the waiter leaves the
fence installed. A reopened daemon reads the same fence before admitting work.

Only the exact authenticated pre-commit cancellation can release it: workspace,
owner/controller bindings, source/target peers, operation and source generation
must match. A forward commit cannot unlock the source: it replaces the fence with
the committed state, and the source reads as moved
([activation](cloud-workspace-activation.md)). Verified return/unlock is not
implemented here. Never downgrade a daemon with active fences to a version
that ignores this column, and never restore old main-database rows as an ownership
recovery mechanism.

The registered-stop coordinator now adds a durable pending-attempt guard before
stop dispatch. Cancellation also requires that marker to be settled and no stop
task in flight. A lost caller does not cancel host work, and reopening the daemon
cannot clear a pending attempt whose handles were lost. See
[Registered source stops](registered-source-stop.md). Settlement covers registered
stop calls only; it does not issue source-stop evidence or automatically release
the execution fence.

## Admission drain is not process-stop proof

A finished admission means the launch/send call ended, not that its process
stopped. A lost sidecar response can leave an unregistered/uncertain launch;
previous agents can continue autonomously, a script can keep running and a shell
can have detached children. Restarting the daemon loses its in-memory admission
counters; it does not prove old processes stopped. This primitive issues no source
stop receipt and never advances the controller journal.

Before recording source-stop, integration must retain/reconcile launch intent,
stop or fence every existing execution source, verify process identities and
termination, and coordinate service/database writers. Authentication of receipts,
complete native context, encrypted snapshot transport and a verified destination
commit remain prerequisites for actual handoff. There is no automatic failover.

## Validation

Unit tests cover pre-fence admission drain, waiting timeout, persisted reopen,
exact cancellation, retained cancellation checkpoints/stale-stop refusal and
corrupt/missing metadata refusal. A real disposable daemon
workspace starts a script and terminal, then verifies blocked chat/script/terminal
operations and preservation of a queued message. It deliberately observes that an
old script still runs after admission drain, then stops it explicitly and releases
only a matching cancellation. No real workspace is fenced by these tests.
