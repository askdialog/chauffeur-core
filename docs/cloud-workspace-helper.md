# Archive helper process supervision

`cloud_archive_helper` connects the durable archive job manager to a finite, trusted
helper process. It resolves only `chauffeur-cloud-archive` beside the daemon; it
accepts no client executable, shell command, PATH lookup or environment override.
The selected executable must be a regular, unlinked executable owned by the endpoint
user or root and must not be writable by group/others. Installation directories
remain trusted; this is not isolation from malicious processes with the same OS user.

**The archive helper executable is now included by the reviewed build/install paths**
(see `cloud-workspace-runtime.md`). No installation or service launch is performed
by this change. The owner/worker companions do not expose handoff dispatch. Fresh
authenticated controller admission is provided separately by `cloud-workspace-approval.md`. Protected
configuration lookup and the finite host core are described in
`cloud-workspace-configuration.md`; the executable and production adapters are
provided by `cloud-workspace-runtime.md`. Coordinator/listener wiring remains
pending. A missing approved helper refuses before task acceptance. This supervision API is internal and exposes no
public transfer action or process-launch RPC.

## Private launch and lifetime

```text
persist archive job
  → persist launching process record
  → spawn approved helper in its own process group
  → observe and persist direct-child PID + kernel generation
  → send bound request on stdin; close stdin
  → collect bounded receipt and successful exit
  → verify group absence; persist process outcome
  → daemon rehashes retained ciphertext; persist job outcome
```

The generated private pipe request contains version, a fresh request id, the complete
operation/artifact binding and source/destination role. It contains no archive key,
relay token, provider credential, executable or output path. The helper must wait
for this request before doing work, fetch only approved configuration through the
private daemon, authenticate fresh journal/enrollment state during transfer and close
its Unix capabilities before reporting completion. The finite host core implements
that sequence; its production adapters and executable are implemented separately
in `cloud-workspace-runtime.md`. Coordinator/listener wiring remains pending. The
pipe metadata itself is no authorization or source-stop proof.

The daemon derives the working directory and `CHAUFFEUR_HOME` from its own job
manager. It clears inherited environment and supplies only that home and a minimal
system PATH. The executable/constant private-mode flag are the only command arguments.
Request and stdout receipt are each limited to 8 KiB. Stderr is discarded; arbitrary
child diagnostics never become job error text. A receipt must have the current
request id/version and exact expected ciphertext head, with no unknown fields or
additional JSON value, followed by successful child exit.

The daemon owns the child independently of any observer. Cancellation or deadline
signals only a still-owned, unreaped child group, first TERM and then KILL after a
grace period. It waits for reaping before reporting task settlement; a stubborn
kernel task can therefore outlive the nominal deadline without becoming a false
successful stop. Spawn failures retain an explicit stopped/no-PID record. A helper
error, invalid receipt or failed observation leaves the job uncertain and all files
and keys retained.

## Interrupted and incomplete cleanup

Process records are kept separately from job outcomes. A group still present after
the leader exits leaves an uncertain process record. That record prevents retries,
new operations after a stored job, and successful publication even if a callback
reports matching bytes. A descendant deliberately leaving its process group is
outside this group observation: the trusted helper must own/reap its children and
must not daemonize. These checks grant no general native-process or workspace-stop
proof and release no execution fence.

After restart, the job remains uncertain. `observe_stopped` checks the saved root
generation and group absence without signalling any historical PID or group. Only
a positive observation clears the process retry gate. Missing PID/stamp information,
unreadable observations or a surviving group remain blocked for explicit recovery.
Retained process records also prevent root reinitialization if their job rows and
the endpoint's root pin are missing. No automatic failover, relaunch or cleanup is
attached.

Ten tests use private temporary Python stand-ins, never model agents or production
daemons. They prove the PID is durable before stdin is consumed, environment/argument
separation, strict receipt bounds/bindings, TERM/KILL/reaping, cancellation, observer
detachment, surviving children, spawn failure, installation refusals, root-pin
protection and observation of a historical process without killing it.

Private job dispatch and the finite controller admission listener are described in
`cloud-workspace-dispatch.md`. The trusted configuration coordinator, source
stop/preparation, readiness, activation, cleanup and desktop/mobile dispatch remain
integration gates before real handoff.
