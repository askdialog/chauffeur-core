# Daemon-owned archive jobs

`Daemon::archive_jobs` adds internal host-task acceptance, lifetime and durable
outcomes. The [process supervisor](cloud-workspace-helper.md) can launch an approved
installed archive helper and retain its root generation and cleanup outcome; the
helper implementation/installation remains pending. It exposes no user action. Host
dispatch still must authenticate the handoff journal and enrollment, prepare the
sealed source, provision the endpoint channel and supply the trusted transfer task.

```text
validate binding + protected endpoint setup
  → persist running job before spawning its owned future
  → transfer task runs independently of its observer
  → task settles, or panics/errors
  → daemon rechecks actual retained ciphertext
  → persist stored / uncertain outcome
```

The job is bound to one workspace, operation, endpoint role and exact ciphertext
reference. Metadata lives in the FULL-synchronous endpoint database; it contains
no archive key, provider credential, executable path or raw task error. A saved
job also prevents implicit creation of a new vault root if its trusted pin is lost.
An existing manager whose database disappears refuses instead of creating empty
history. Reopening a home with a retained lock and missing database also refuses.

The manager admits one live task per workspace and keeps attempts for explicit
reconciliation. Different workspaces progress independently. An exact retry of an
uncertain attempt must name the previous job and retain the same context, role and
ciphertext reference. An uncertain operation cannot be replaced by a different
operation; a new operation follows only a stored previous job and names that job.
Successful attempts are inspected through `read`/`latest`, not silently rerun.

Dropping the acceptance receiver detaches an observer; it neither drops the host
task nor cancels the transfer. Explicit cancellation is durably recorded before
signalling the task's token. The task remains `cancelling` until it actually settles;
the manager never drops a future to pretend its children or I/O stopped. Other
workspaces are unaffected. Daemon shutdown signals child cancellation tokens and
blocks new acceptance. Host dispatch must retain and reap all child resources inside
the supplied future before it resolves. The supervisor provides finite process/group
tracking; the helper's semantic dispatch and authorization integration remain pending.

A matching reported head alone is insufficient for `stored`: the daemon independently
verifies and synchronizes its local artifact. Verification runs outside the global
job gate so hashing a large archive does not block another workspace's status or
cancellation. Cancellation, failure, panic, mismatched results, missing/partial or
changed bytes produce `uncertain`. No job outcome deletes files or keys.

`stored` is a ciphertext task outcome, not a signed remote restoration receipt,
environment readiness, source-stop proof or execution lease. The source role checks
its own retained source artifact and the trusted task's reported result; the task
must use the authenticated transport for destination confirmation. Ownership still
requires the existing separate handoff validation and fences.

A single manager holds an exclusive OS lock per daemon home. It explicitly unlocks
when dropped so inherited open descriptions cannot prolong the old manager's
ownership. Reopening marks persisted `running`/`cancelling` rows uncertain, never
relaunching them. Source files, partial destinations and execution fences remain
untouched. A restarted daemon must reconcile old host processes separately; a stale
row or missing heartbeat cannot authorize a replacement execution host.

Eight tests cover detached observers, independent workspace cancellation, actual-byte
verification, exact lost-confirmation retry, preserved partial data, panic handling,
shutdown, exclusive manager ownership, interrupted-row recovery, missing databases
and missing-root protection. They use synthetic tasks and temporary homes; no model
session or production daemon runs.

Installed helper launch and private channel dispatch are described in
`cloud-workspace-dispatch.md`. Authenticated controller journal orchestration,
source stop/preparation, environment/data readiness, activation,
cleanup and desktop/mobile actions remain integration gates.
