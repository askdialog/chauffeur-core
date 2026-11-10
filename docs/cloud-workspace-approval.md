# Fresh archive transfer admission

Archive transport admission binds a stopped-workspace handoff to one selected
ciphertext. It grants permission to move encrypted bytes, never permission to run
the destination or release the source fence.

## Immutable selection and current journal

The internal Rust `cloud_archive_approval::publish` accepts an already authenticated
snapshot-ready handoff state and a verified sealed artifact head. Publication reads
the current journal in the same immediate transaction and refuses stale state. It
selects one exact ciphertext head per operation; retries are idempotent and a
replacement is refused. The trusted caller must obtain real source-stop/snapshot
evidence and verify the sealed artifact first. Publication has no client RPC.

`cloud.archive.approval` is a private Unix-only read RPC. Each call reads the pinned
root, full current handoff journal/history and selected ciphertext in one database
snapshot. Both endpoints must match the operation, source epoch, snapshot digest,
ciphertext digest/length and revision floor. Missing, cancelled, advanced, corrupt
or substituted state refuses. It neither advances the journal nor acquires an
execution lease. Approval rows also prevent silent root reinitialization.

## Pinned controller channel

Each source or destination connects to its pinned controller through a distinct
Noise domain covering the full selected binding, its endpoint identity and minimum
revision. Other archive/control/bootstrap protocols cannot join it. Wire packets
contain only a version, request id and accepted journal revision; keys and artifact
bytes use the separate archive delivery channel.

`ControllerRuntime.serveArchiveAdmission` requires its protected active installation
and the installed owner/worker pair. Before and after every private journal read it
rereads the worker enrollment, checks the exact installed challenge/certificate and
verifies the signed approval and expiration. A durable revocation tombstone refuses
further checks. The owner can be offline; no cached grant substitutes for current
controller state.

The client waits for an authenticated ready packet and round-trips on every
`authorize()` call. It accepts one outstanding request, monotonically increasing
ids/revisions and bounded packets. A denial, timeout, cancelled signal, malformed
reply or disconnected channel closes admission. Late replies cannot recover a
closed channel. Callers pass `authorize` into archive delivery's existing per-key,
per-block and final checks. Cancellation after a check may allow its already
admitted block; it never authorizes activation or deletion of recovery material.

## Evidence and integration gates

Four Rust tests cover immutable publication, both endpoint roles, cancellation,
revision/binding substitution, corrupt selection, missing history/root and root
reset protection. Ten channel/delivery tests cover fresh reads, revocation before
and during journal reads, response loss, concurrent checks, revision rollback,
Noise domain separation and cancellation during a multi-block transfer. The latter
retains both keys and partial ciphertext, with no storage receipt. A controller
runtime test uses its real protected enrollment store: admission works while the
owner endpoint is destroyed, and a later signed revocation refuses the next check.
The private RPC transport test also refuses public TCP access.

This provides the authorization primitive and controller runtime integration.
Protected configuration lookup and the finite host core are now implemented in
`cloud-workspace-configuration.md`; the executable/adapters are provided by
`cloud-workspace-runtime.md`; finite controller listeners and private job dispatch
are described in `cloud-workspace-dispatch.md`. Source stop/preparation,
destination readiness, activation, cleanup and desktop/mobile actions remain
integration gates before real handoff.
