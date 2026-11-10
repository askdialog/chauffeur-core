# Coordinating registered source stops

`workspace_source_stop::stop_registered` is internal source-host plumbing for an
explicit, authenticated handoff. Trusted callers must authenticate the exact
controller, source peer and operation before invoking it. No public RPC, phone
connectivity event or background timer invokes it. It does not advance the
controller journal or issue a source-stop receipt.

The host installs the persistent execution fence, waits for pre-fence admissions
to drain, and records a synced pending stop attempt before dispatching any stop.
One attempt per workspace may own those calls. The task belongs to the host:
dropping its caller does not cancel its work. Admission timeout leaves the fence
installed without dispatching stops.

The attempt concurrently stops registered native sessions, managed script groups
and terminal children. Every category is attempted even if another fails. Native
sessions are selected from current live chats and durable unresolved launches,
including launches whose chat row was removed. The existing private-pipe generation
and exact launch IDs must validate a nonempty stop receipt; legacy empty replies
remain uncertain. Read-only kernel-identity reconciliation can settle registered
old roots only after their original sidecar and root both disappear. Missing PID
registrations never imply exit.

Successful native stops release the live session, requests and tracked tasks;
they preserve the chat's tab, conversation identity, attachments and persisted
message queue. Failed stops keep their session reservation and evidence. Scripts
retain their observations, and terminals are forgotten only after confirmed
reaping. Other workspaces are unaffected.

While fenced, ordinary terminal close still requests a stop but retains the child
handle for checked reaping; ordinary script cleanup retains group observations.
Pre-fence cleanup holds admission through removal, so the stop coordinator drains
it before collecting handles. An unreadable fence also retains evidence. This
prevents a same-daemon retry from mistaking forgotten uncertain handles for exit;
it does not reconstruct processes forgotten before the fence existed.

## Cancellation and interrupted attempts

A separate local `workspace_stop_attempts` row binds the pending marker to the
exact persisted fence and daemon generation. It contains no provider credentials,
conversation text or filesystem contents. A guard prevents overlapping attempts.
Only completion of all registered stop calls with no unresolved launch evidence
persists settlement. An error, panic or dropped guard leaves the marker pending.
Settlement does not release the execution fence: the exact controller-authorized
pre-commit cancellation is still required to resume local admissions.

Cancellation cannot overtake a pending or in-flight stop. Otherwise a delayed stop
could kill work that the user just resumed. A retry in the same daemon may settle
retained handles and exact receipts. A new daemon cannot settle a pending attempt
using empty in-memory registries: explicit recovery of the lost process/stop-call
evidence is still required and is not implemented here. This deliberate refusal
also covers a crash after commands completed but before settlement was durable.
A previously durable settled marker remains valid across store reopen. New
operations can replace a settled checkpoint, never an uncertain older generation.

Do not downgrade a daemon with pending source-stop attempts to one that ignores
this table; that would invalidate cancellation ordering. No ordinary shutdown,
archive, chat close or source-stop task automatically clears the fence.

## This is not complete workspace quiescence

`Ok(())` describes registered stop calls and native-root observations only. It is
not permission to capture a snapshot, commit ownership or start a worker. Native
roots and terminal children can leave detached descendants; scripts cover their
original groups only. External writers, unregistered legacy processes, declared
services, database exports and launches lost across daemon generations still need
complete inventory and fencing. The remaining source-stop coordinator must prove
those boundaries before issuing authenticated source-stop evidence. This module
adds no workspace transfer, provider export or GCP operation.

The [process sweep](workspace-process-sweep.md) now follows the registered stop: it
ends detached descendants and lost launches found in the operating system's process
table and refuses on external writers. Services, database writes after an export and
the composed receipt remain. [The complete source stop](workspace-source-stop-receipt.md)
now composes every step into the receipt the journal's `Stopped` state names.

The source-local capture/seal owner in `cloud-workspace-preparation.md` now requires
an independently authenticated complete-stop acknowledgement. It does not upgrade
these registered observations into full quiescence evidence.

`stop_registered_and_services` now composes those observations with the explicit,
reconstructible-only Docker stop in `workspace-service-stop.md`. It drains registered
launchers before the service phase and retains a durable preparation barrier on
uncertain Docker calls. Neither API issues complete source-stop evidence.

## Validation

Gate tests cover drain ordering, duplicate attempts, cancellation races, abandoned
guards, reopened pending/settled markers and changed evidence. A disposable daemon
with a separate stand-in sidecar, real script and real terminal verifies partial
failure, legacy replies, retained queue/tabs, unaffected workspaces and successful
retry. A held private-pipe stop verifies that aborting the caller leaves the host
task running and blocks cancellation until durable settlement.

After building the sidecar, the opt-in real native proof is:

```sh
cargo test -p chauffeurd --test lifecycle real_registered_source_stop -- --ignored --nocapture
```

It initializes real Claude and Codex conversations, waits for a completed turn,
requires actual kernel-identity registration, invokes the registered-stop
coordinator and verifies exact root-stop evidence with the native conversation
identity retained. It always shuts down the isolated sidecar and uses the safe
transcript cleanup helper for its temporary fixture before removing it.
