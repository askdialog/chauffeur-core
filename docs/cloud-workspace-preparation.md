# Source-local workspace preparation

`cloud_workspace_preparation` connects capture, protected key reservation,
encryption and verified ciphertext staging under one daemon-owned preparation
barrier. It exposes no client RPC and does not change the controller journal.

## Authenticated boundaries

The trusted coordinator supplies both the original `Stopping` state and its exact
authenticated `Stopped` successor. Capture refuses unless that successor's
`source_stop_digest` names a receipt this daemon stored under the same fence with
[the complete source stop](workspace-source-stop-receipt.md). Success from
`stop_registered` alone supplies no such receipt. Authenticated remote evidence
remains integration work.

Capture first requires the exact settled local stop and reserves a durable
preparation attempt. It writes a new private operation folder under the daemon
home, outside the workspace. The returned capability contains the snapshot digest
and blocked-chat reasons. The source and conversations remain intact; blocked
conversations are carried as blocked, never silently omitted or replaced.

The coordinator must authenticate the exact `SnapshotReady` successor before
sealing. The source verifies its selected digest against the captured package,
reserves a protected encryption key, seals through the private sidecar, stages the
ciphertext into the existing descriptor-based artifact store, and independently
rechecks it. Only then does this local preparation settle. The returned binding
is a candidate for authenticated controller approval, not an admission receipt,
ownership commit or permission to run the destination.

## Lifetime and cancellation

The persistent `workspace_preparation_attempts` history binds each attempt to its
exact fence, daemon generation and unique attempt ID. While pending, cancellation
cannot reopen local execution, another stop cannot overtake capture, and no newer
fence can replace its operation. SQL checks serialize these races across gate
instances as well as within one daemon.

Capture and seal tasks retain their guard after the observer disappears. Error,
panic, discarded capture capability and interrupted daemon leave a pending marker;
neither the same daemon nor a restarted one silently adopts it. Explicit recovery
of these uncertain preparations remains to be implemented. Never downgrade a
daemon with pending preparations to a version that ignores this table.

A live completed capture capability can explicitly accept an authenticated
pre-commit cancellation: it settles its now-idle preparation before invoking the
existing cancellation fence release. It retains package files for separately
authorized cleanup. Successful sealing also leaves the execution fence installed;
only the existing exact cancellation or future ownership reconciliation can
change execution authority.

## Key reservation and retained files

`PreparationKeys` stores an operation-bound key in the protected endpoint backend
before any encryption request. SQLite retains only the exact context, local source
identity and digest pin. Root identity is required, and these reservations prevent
root reinitialization after missing metadata. The protected value is versioned,
bounded and bound to the same root/context; diagnostics redact key material.

An existing reservation is read explicitly, never regenerated. Missing, malformed
or substituted secrets/pins refuse. A crash between exclusive protected creation
and the SQLite commit leaves an orphan secret, which also refuses implicit adoption.
Failed seal requests retain the key, original package, source and partial output.
There is no automatic reseal, retry-in-place, cleanup or head replacement.

Isolated protected-file backends now refuse links, special files, multiple hard
links, wrong ownership, permissions other than `0600`, and oversized values.
System Keychain/Secret Service behavior is unchanged. This protects test/custom
daemon homes without falling back to a personal configuration file.

The source helper now reads this exact reservation through the source-only private
Unix `cloud.archive.preparation_key` method. It requires an existing protected
configuration, the rooted source identity and independently verified staged
ciphertext. Missing state is never initialized, adopted or replaced; TCP requests
are refused. This local method grants no remote admission or execution authority.

After fresh authenticated controller approval, the helper opens the explicitly
selected existing recovery vault, reads the reserved key and checks approval again
before retaining it durably. A retained different key refuses without replacement.
Approval is checked again after retention and before artifact access. Targets never
read this source reservation; they retain keys received through the existing
authenticated endpoint channel. Decoded temporary key buffers are cleared.

Creation of the operation's recovery vault is still explicit coordinator work;
the helper cannot create one when a head or capsule is missing. Remote controller
publication, routing/configuration orchestration, full quiescence proof, destination
readiness/activation and user-facing handoff remain gated.

## Evidence

Temporary fixtures cover exact state acknowledgements, preparation/cancellation
and stop races, abandoned guards and restart refusal, protected-key persistence
and substitution, actual archive encryption/decryption and verified staging,
lost capture observers, explicit completed-capture cancellation, and interrupted
seals retaining their keys and partial files. No real user conversations, model
sessions, production daemons, cloud services or e2e scripts are used.
