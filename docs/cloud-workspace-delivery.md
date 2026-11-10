# Endpoint-to-endpoint workspace archive delivery

`packages/cloud/src/workspace-archive.ts` adds a host-only delivery protocol for an
already sealed archive. It does not install a listener, expose a daemon/client RPC,
change ownership or enable agent execution. Host orchestration must supply current
authenticated operation state, approved endpoint identities and daemon-owned artifact
I/O. No personal accounts or real workspaces were used for its proof.

## Identity and authorization

A dedicated `chauffeur-workspace-archive-delivery-v1` Noise domain pins the source
and destination Ed25519 identities. Its prologue binds the complete package transfer
context and exact ciphertext digest/length. Workspace, owner, controller, operation,
source epoch, source/destination and snapshot substitutions fail authentication.
The controller cannot be an endpoint. Bootstrap, refresh and native-job channels
cannot speak this protocol, even with the same peer identities or relay credentials.

The required `authorize` callback checks fresh authenticated journal state and live
enrollment. It runs before key delivery, during every data block and before completion.
A caller must verify the expected stopping/snapshot evidence and operation/generation,
not turn a heartbeat or planning metadata into source-stop permission. Noise proves
identity, not these prerequisites. Revocation or inability to validate authority
stops further progress and leaves recoverable sources and partial outputs intact.

Relay administration and endpoint transport credentials remain separate. Operators
must provision a suitable direct endpoint channel; this slice neither deploys nor
reconfigures the relay. Normal WSS configuration/TLS validation remains unchanged.
The protocol accepts an already connected stream and supports either transfer direction.
A phone UI connection is not its transport or lifetime owner.

## Protected key retention

`WorkspaceArchiveStore` uses a separate protected endpoint vault per operation.
The source commits the archive key before delivery. The destination commits that
same key under its own protected root and trusted checkpoint before acknowledging
readiness to receive data. The relay, controller and blob storage see no plaintext
key. Existing different keys, operation contexts or ciphertext heads cannot replace
retained material. The capsule contains no other provider credentials or home data.

Explicit create writes an empty operation manifest. Explicit open must recover that
manifest from the endpoint's trusted current checkpoint; missing/corrupt state never
becomes an empty store or a new identity. Reopening preserves selected keys and stored
status. Encrypted blob listings and ordinary workspace backups cannot supply or reset
the trusted checkpoint. Protection has the existing vault's whole-metadata-rollback
limitations; this is not an execution lease.

## Bounded streaming and durable receipts

```text
destination ready → source offer (bound head + selected key)
  → destination persists key and reserves artifact → ack(0)
  → chunk(offset, bytes) → write → ack(next offset)       [repeat]
  → source verifies full digest → finish
  → destination verifies full digest and durable artifact
  → destination persists stored status → stored(head)
  → source persists acknowledgement
```

Each chunk carries at most 256 KiB, with only one outstanding block. Exact offsets,
canonical base64url, packet version/fields, total size and hashes are checked; there
is no unbounded pending-write queue or full-archive buffer. Limits inherit the sealed
archive's 16 GiB framed plaintext plus 36 bytes of ciphertext overhead. A host deadline
covers handshake, metadata and I/O (six hours by default, configurable up to 24 hours).
Explicit host cancellation closes transport without changing ownership or cancelling
another workspace. No automatic retry, failover or source cleanup is attached.

`WorkspaceArchiveReader` and `WorkspaceArchiveSink` are trusted daemon-owned adapters,
not permission for a client/relay to write files. They refer to one operation-bound
artifact, never a peer-supplied path. Sink reservation must be exclusive, existing
partial output must refuse overwrite, and commit must verify actual stored bytes and
sync the file and parent directories before resolving. The Rust
[daemon artifact store](cloud-workspace-artifacts.md) now implements this filesystem
contract, and the [private host bridge](cloud-workspace-bridge.md) connects it to
the Noise library through connection-scoped handles. Installed host dispatch remains
integration work. A pending I/O
callback may settle after disconnect; outputs must remain retained for reconciliation.

The stored receipt proves only durable ciphertext delivery through that adapter. It
is not an AEAD/package validation, native restoration receipt, environment readiness,
a source-stop proof or permission to activate. The existing archive open and quarantined
restoration stages still follow. Transfer archives/keys are not deleted here.

## Uncertain outcomes and verification

After a lost acknowledgement, explicitly reopen protected stores and inspect the
artifact. Identical completed output must be independently reverified; a retry can
then recover the receipt without retransmission. A missing or partial artifact is
not recreated/truncated because a metadata row says stored. Failures retain source
bytes, selected keys and pending outputs. Do not remove the last recoverable copy.

Tests cover protected key commit before data, encrypted retention/reopen, corruption,
wrong operations/artifacts/keys, failed storage, partial writes, lost acknowledgements,
revocation during a source read, cancellation, deadlines and cross-protocol refusal.
Real local relay sockets carry the actual encrypted archive format in both directions,
then decrypt it into isolated temporary files. Ciphertext observation copies bytes at
the WebSocket boundary before Noise can consume or decrypt its backing buffers; it
asserts the key and private contents do not appear on the wire. Tests use synthetic
identities/data and no native model sessions or production daemon.

Host-job/channel provisioning and dispatch, authenticated controller
journal orchestration, readiness, destination activation, archive cleanup and UI
remain explicit integration gates before real workspace handoff.
