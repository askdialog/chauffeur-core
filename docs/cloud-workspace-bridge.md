# Private host bridge for workspace archive delivery

The installed endpoint helper can now compose the pinned Noise delivery protocol
with the daemon's retained archive store. `deliverDaemonWorkspaceArchive` and
`receiveDaemonWorkspaceArchive` own one dedicated local Unix connection per job.
The helper never creates, truncates, writes or deletes archive files itself.

```text
source host helper                    destination host helper
  source_open → daemon reader            receive_begin → daemon writer
  read(handle, offset, length)            write(handle, offset, ciphertext)
          └──── pinned Noise delivery ────┘
                                         commit(handle) → rehash + fsync
  close(handle)                          stored receipt → endpoint vault
  close dedicated Unix connection        close dedicated Unix connection
```

## Private contract

Six generated `cloud.archive.*` methods are admitted only over the daemon's private
Unix socket. The authenticated public WebSocket refuses all six, as it does the
other endpoint trust methods. Ordinary RPC dispatch cannot invoke them without
the connection's registry. Requests accept operation/context bindings, opaque
handles, offsets and ciphertext blocks, with no source/output path or archive key.

`source_open` opens only a previously staged source with the exact retained binding;
it does not prepare, capture or seal a live workspace. `receive_begin` reserves a
new bound output or independently verifies and synchronizes complete existing
output. `allow_new=false` cannot recreate missing data. `read`, `write`, `commit`
and `close` use handles belonging to that connection alone. Handles are random
256-bit values and are never persisted or recovered from filenames.

The bridge checks canonical workspace/operation UUIDs, endpoint roles, safe epochs,
digests and ciphertext bounds before creating an artifact. The local helper supplies
its selected endpoint identity and authenticated context. This routing metadata is
not a signed controller journal, source-stop evidence or execution authority. The
host still must authenticate fresh operation state and enrollment using the delivery
protocol's required authorization callback. The private socket's OS user is trusted;
this API does not authenticate mutually hostile local clients running as that user.

## I/O ownership, bounds and uncertain outcomes

Filesystem work runs on blocking workers, serialized within each connection in the order
its calls came, so a driver sends a handle's blocks ahead of their answers; at most eight
live handles. Reads/writes are limited to 256 KiB; encoded payloads
are checked before decoding, offsets remain exact and the filesystem store rehashes
the actual received file before synchronizing it, its binding and parent directories.
An invalid write consumes its handle and preserves the reserved output. Missing or
partial previous artifacts cannot be truncated or silently reset.

Connection closure first prevents new work, then releases descriptors after any
in-flight I/O settles. It publishes no late handle/receipt to the disconnected job.
Explicit handle closure releases its lock; closing a connection also releases a
handle whose allocation succeeded but whose RPC response was lost. No closure
unlinks files, deletes keys, changes ownership or clears execution quarantine.

The TypeScript adapters serialize requests, copy each outgoing block before awaits,
and wait for in-flight calls before closing known handles. Host wrappers own and
close the dedicated Unix transport on success, error, cancellation and timeout.
That transport must reject pending requests when closed. The deadline covers
authorization, opening and hashing as well as Noise/data transfer. A blocked callback
may settle later; cancellation leaves its files retained for reconciliation.

A commit reply lost after durable storage is an uncertain outcome, not permission
to overwrite or activate. Both endpoint keys remain available. An explicit fresh
connection can reverify the complete ciphertext, recover a stored receipt and finish
the existing protected endpoint operation without retransmission. An incomplete
artifact still refuses and needs explicit reconciliation.

## Proof and remaining gates

Rust tests exercise scoped handles, capacity, bounded blocks, late closure, staged
source reads and rejected substitutions. A real temporary Unix WebSocket proves
cross-connection refusal and receipt recovery after disconnect; a separate token-
authenticated TCP WebSocket proves all six methods refuse outside Unix without
creating the selected operation. No production daemon or native model session runs.

TypeScript tests compose both host wrappers with Noise and synthetic daemon RPCs,
including durable key retention, lost commit response/retry, pending begin/write
closure, binding snapshots, deadlines and a lost source-open response. Existing
real relay tests and Rust encryption/restoration coverage remain complementary
proofs; this is not an installed cloud round trip.

The daemon's [archive job manager](cloud-workspace-jobs.md) now provides durable
acceptance and host-owned task lifetime independent of the observer. Installed
helper/channel dispatch, authenticated controller journal orchestration,
source preparation/stop coordination, complete environment/data readiness, activation,
cleanup and UI remain integration gates before real workspace handoff.
