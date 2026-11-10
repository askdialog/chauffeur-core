# Private archive dispatch and admission service

Private Unix RPC now connects approved archive configuration to the daemon's
durable job supervisor. These methods are refused on TCP and accept an exact
workspace/operation/peer/artifact binding, never an executable or filesystem path.

- `cloud.archive.start` reads the existing protected configuration, refuses expired
  state, and launches only the installed sibling helper. Its deadline is bounded
  by configuration expiry and six hours. Acceptance returns the durable job;
  losing the caller or its response does not cancel that job.
- `cloud.archive.status` reads the latest workspace job. Only genuine absence is
  empty; another operation or role conflicts. Recovery reads do not require a
  still-valid configuration, token or controller connection.
- `cloud.archive.cancel` requires the exact job ID and binding. `cancelling` is an
  acknowledgement, not settlement. Poll status until the owned task settles.
- `cloud.archive.observe_stopped` checks a historical helper without signalling
  it. Observation permits existing explicit recovery rules; it never establishes
  storage success or releases the workspace execution fence.
- `cloud.archive.preparation_key` reads only the exact source key reserved before
  sealing, after protected configuration, rooted source identity and actual staged
  ciphertext checks. Configuration is checked again after verification. It cannot
  reserve a key, create recovery storage or act for the target. The source helper
  requires fresh controller approval around this read and durable retention into
  the existing vault, before opening ciphertext handles. Missing or conflicting
  material refuses without replacement; neither source nor retained keys are erased.

Starting another job requires the exact previous job ID. Existing job rules still
refuse running/unobserved processes and retries outside the retained uncertain
operation. Only verified local ciphertext yields `stored`; failure, interrupted
launch and cancellation retain uncertain state, keys, source and partial output.

## Finite controller listener

`chauffeur-controller serve-archive` opens an existing installed controller and a
root-owned private `/etc/chauffeur-controller/archive-admission.json` plan. It does
not initialize a vault. The strict version-1 `CloudArchiveAdmissionPlan` selects
source and target approvals for one identical immutable context/head/revision,
their separate relay channels and tokens, and an expiry within 24 hours. The
existing controller configuration and independent owner pin still apply.

An operator must provision the plan explicitly (root ownership, private mode such
as `0640 root:chauffeur-controller`). The service authenticates both endpoints and
checks live enrollment and the daemon journal for every admission. Closing one
channel leaves the other available for final checks. Expiry, cancellation or setup
failure closes all resources; there is no reconnect or automatic replay. Listener
completion means only that both channels ended, never a storage or handoff receipt.

Controller artifacts include `chauffeur-controller-archive-admission.service`.
Installation copies this finite unit but neither starts nor enables it. It has no
restart policy or install target; an operator must select each operation explicitly.
The installer refuses replacement while this service is active.

Tests cover lost observers, exact/stale IDs, configuration loss during recovery,
restart uncertainty, private transport refusal, independent channel closure,
revocation, expiry and cancelled setup. No real daemon, native session, GCP service,
installation or e2e script is started for these checks.

Validation on the current 0.14.0 base: 814 Rust tests passed (one native-session
test intentionally ignored), with warning-free Clippy and format checks. Cloud,
relay, sidecar and app typechecks/tests also passed; the controller bundle and
installer syntax were checked without installing or starting services.

Source stop/preparation, authenticated configuration publication and remote job
coordination, destination readiness/activation, cleanup and desktop/mobile actions
remain integration gates. These private controls alone do not authorize execution.
