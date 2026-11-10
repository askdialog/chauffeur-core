# Protected archive host configuration and dispatch

The archive host core now connects a daemon-bound job to current controller
admission and daemon-owned ciphertext I/O. Its inputs contain one operation binding
and role, never arbitrary paths, executables or relay credentials.

## Protected local selection

`ArchiveConfigurations::configure` is an internal trusted API. Its caller must
already authenticate the exact operation, ciphertext and routing evidence. It
selects one configuration per operation, including local role, revision floor,
expiry, recovery vault location and separate transfer/admission channels. Version 2
also requires explicit storage authority; see `cloud-workspace-runtime.md`. Syntax
validation is not remote authentication. No public or private RPC can publish or
replace this selection.

Relay tokens and configuration are stored through the existing protected-secret
backend, with a per-operation name derived only after canonical UUID validation.
Production uses Keychain/Secret Service without plaintext fallback; isolated test
homes use only their own private secrets folder. `SecretName` now borrows its
service/file names so this operation name need not be leaked into static memory;
existing static names and backend behavior are unchanged.

SQLite retains only the operation id and a digest covering the canonical protected
configuration and endpoint root. The table also blocks implicit root reset. The
secret is durable before the digest pin. An identical explicit publication retry
can finish a missing pin; read-only lookup cannot repair it. Missing, corrupt,
changed or unpinned protected material refuses without overwriting or deleting it.
Debug formatting omits relay tokens.

`cloud.archive.configuration` is Unix-only and returns one exact binding/role,
never a listing or default. Expiry is mandatory and at most 24 hours away. Channels
must use WSS origins, canonical channel ids/tokens and independent credentials.
A selected configuration is immutable, including expiry; changing or renewing it
requires explicit operation recovery, which this slice does not implement.
Configuration grants no source-stop proof, handoff transition or execution lease.

## Finite host core

`runArchiveHost` validates the private helper request before opening any connection:
version, canonical request id, exact fields, binding, source/destination role and
an 8 KiB serialized limit. It then:

1. Reads the exact protected configuration over the private daemon connection.
2. Opens the existing endpoint identity and checks it matches the local binding.
3. Authenticates the separate pinned controller admission channel and obtains a
   fresh admission before opening checkpoint storage.
4. Opens the already-created recovery vault. Missing keys/checkpoints never trigger
   initialization or listing-based reconstruction.
5. Opens a different private daemon connection for ciphertext handles and joins
   the selected transfer channel in the correct source/destination role.
6. Uses fresh admission for every archive key/block/final check and returns the
   exact bounded pipe receipt only after closing its capabilities.

The job deadline includes configuration/identity/storage/channel setup. Configuration
expiry also aborts the job. Private connections close on cancellation, rejecting
pending daemon RPCs; supplied transport/storage factories must honor the abort
signal and settle pending work. Lost replies, refusal or a later-block cancellation
retain keys, partial output and the source. This core never starts native agents,
spawns children, reads provider credentials or grants activation.

## Evidence and remaining gates

Six Rust tests cover source/destination recovery after manager reopening, redacted
diagnostics, secret-versus-SQL custody, immutable publication, lost-pin explicit
retry, binding/role substitutions, bounded syntax/expiry, damaged/missing secrets
and root reset protection. The existing private transport test now also refuses
public TCP configuration calls.

Seven host tests compose protected fixture identities/checkpoints, controller Noise
admission and daemon-handle I/O. They cover full multi-block delivery and exact
receipts, unknown/oversized requests, substituted configuration/local identity,
missing recovery storage, initial journal refusal, interrupted later blocks,
configuration-read timeout and expiry during storage setup. Test vaults are reopened
for recovery: an already-open vault correctly refuses a stale trusted head after
another instance advances it. No real native sessions or production daemons run.

The executable and production transport/storage factories are now implemented in
`cloud-workspace-runtime.md`; private job dispatch and finite controller admission
listeners are described in `cloud-workspace-dispatch.md`.
Authenticated configuration publication/coordinator,
source stop/preparation, destination readiness/activation, cleanup and client actions
remain integration gates. The helper does not discover owner GCP credentials or
silently substitute worker metadata credentials on a Mac.
