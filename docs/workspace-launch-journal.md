# Durable ordinary-chat dispatch intents

Every daemon `Sidecar::call(SessionStart)` now writes an intent to the main local
SQLite store with EXTRA synchronization before writing the request to the private
sidecar pipe. The intent binds a unique launch UUID to the workspace, chat and a
fresh UUID for that sidecar process generation. The sidecar receives `launch_id`
as an optional additive field. No prompt, credential or provider token is stored
in this journal.

A successful start response changes `launching` to `acknowledged`; neither phase
proves process exit. An error, dropped future, broken pipe, lost reply or daemon
restart leaves the intent unresolved. The journal is independent of the chat's
visible status and in-memory live-session map. Rows are retained when a chat or
workspace is removed; deleting UI metadata cannot manufacture process absence.

## Exact local root-stop observations

The new sidecar retains dispatch metadata by launch ID. A repeated ID cannot
start another native process. Only successful session shutdown or the session's
verified natural root termination marks an observed launch stopped. A failed
startup cleanup stays unresolved while its session is retained. Rejection before
entering `session.start` proves that this particular intent created no process.
Completed tombstones release their references to SDK/session objects and retain
only the launch ID, chat ID and stopped state.

`session.stop` and a successful `session.close_idle` optionally return
`confirmed_launch_ids`. Unknown sessions emit no invented IDs. Legacy/untracked
sessions emit no launch receipt. The daemon validates all returned IDs against the
exact chat and the generation of the private pipe that supplied the response,
then atomically records their root-stop observations. A new sidecar cannot settle
an old generation, even with an exact old ID. Duplicate, unknown, mismatched or
oversized receipt batches fail without partially changing rows; repeated valid
receipts are harmless. A late start acknowledgement cannot reactivate a stopped
intent, including across independent SQLite connections.

A stop racing a not-yet-delivered start cannot acknowledge that unseen launch:
its ID is absent from the sidecar's observed metadata. A stop racing initialization
must finish the tracked session shutdown before it can return its ID. Receipts
for old launches cannot settle a newer intent of the same chat.

## Compatibility and limits

Wire additions are optional. An old sidecar may ignore `launch_id` and return its
ordinary empty stop result. Normal chat calls remain compatible, but those replies
leave the durable intents unresolved. Existing sidecars and agents launched before
this migration are not retroactively inventoried. Downgrading to a daemon which
does not journal launches invalidates any assumption of complete coverage.

This combines a durable dispatch journal with the native-root identity observations
described below; it is not a workspace process controller. It does not
automatically reconcile unknown launches, stop descendants, block
ordinary local chat resumption or authorize destination execution. Local resumptions
keep their existing behavior; unresolved older generations remain recorded. The
sidecar's in-memory tombstones disappear on its restart, while their daemon-side
unresolved intents remain. Completed journal metadata is retained; safe retirement
and garbage collection of old generations remain separate work.

An empty `workspace_launches::unresolved` list is **not** a source-stop receipt.
Legacy agents, uncertain launches, detached children, services, external writers,
native context and database consistency must still be reconciled. An authenticated
handoff coordinator must keep the workspace execution fence and verify complete
source quiescence before snapshot/commit. This module exposes no public client RPC
or user-facing transfer action, and performs no GCP operation.

## Validation

Three journal tests cover persistence/reopen, exact and atomic receipt binding,
corrupt metadata, missing chats, late acknowledgement, and concurrent acceptance
versus confirmation across independent SQLite connections. A real disposable daemon
and a separate stand-in sidecar verify that the persisted intent exists before the
request arrives, legacy replies do not settle it, a lost start response survives
store reopen and a new sidecar cannot settle the old generation. Sidecar tests
cover tracked versus untracked sessions, failed cleanup, busy idle-close, retries,
replayed IDs and successful stop receipts.

## Persisted native-root identities

Successful new sidecars optionally return their owned `native_pid` in the private
`session.start` result. The daemon observes the kernel start identity itself and
requires that root to be a direct child of the exact sidecar process it spawned.
The sidecar's identity is checked before and after the child observation. Linux
also rechecks the child's start ticks and parent after reading its UID. Request
PIDs are never used as signal targets. Missing metadata, early process exit or
failed observation leaves an intent without a registration; a successful ordinary
chat is not replayed because registration was unavailable.

A separate retained table binds one immutable pair of process identities to each
launch. `reconcile_roots` can explicitly inspect that evidence after reopening the
store. It records `stopped` only when both the exact sidecar and its native root
are absent or replaced. A live sidecar may still deliver a delayed request; a dead
sidecar can leave a live orphan. Either condition leaves the intent unresolved.
Unregistered intents are never cleared, even if the daemon or sidecar disappeared.
Read failures propagate; no heartbeat, PID alone, timeout or empty registry proves
exit. Repeated registration must match the original identities exactly.

This remains a native-root observation, not descendant/service quiescence. The
reconciliation function is internal plumbing for the future coordinator; it adds
no public command, automatic handoff or new execution authorization. Startup
failures and lost start responses before PID registration still require explicit
recovery, and existing local chat resumption behavior is unchanged.

Disposable-process tests cover a reopened store, unrelated PID rejection,
wrong-generation refusal, a live sidecar after its root exits, an orphan surviving
its sidecar and unregistered launches. The private-pipe integration verifies the
actual child PID is captured by the daemon and only the registered root can be
reconciled after the fixture exits. Real-agent E2E assertions check durable PID
registration for Claude and Codex.
