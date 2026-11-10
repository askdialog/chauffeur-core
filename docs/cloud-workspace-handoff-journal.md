# Workspace handoff journal

`cloud_workspace_handoff.rs` adds controller-side durable **metadata** for the
approved manual handoff. No client RPC, mobile button, agent launch guard, process
stopper or cloud service invokes it yet. This is not an execution lease or proof
that agents have stopped. It must not be used to bypass those integration gates.

## State and progression

A workspace is explicitly registered with its pinned vault-owner/controller
identities, initial execution peer and generation one. Registration requires the
existing protected endpoint root; a read never registers or initializes state.

```text
preparing → stopping → stopped → snapshot_ready → restored → committed
      \          \          \                \          \
       └──────────┴──────────┴────────────────┴──────────→ cancelled
```

Preparation records a new unique operation and destination, leaving the source as
the recorded execution peer. From `stopping` through `restored`, the planning API
returns no execution peer. Only commit changes the recorded peer and advances the
workspace generation. The relay/controller identity is never an execution target.

Recording source-stop requires the expected source identity and receipt digest.
Snapshot publication follows source-stop; destination restoration must name the
expected destination and the exact snapshot digest. Commit compares the same
snapshot and restoration receipt. Trusted callers must first authenticate each
actor and verify the complete signed, operation-bound evidence, including all
required files, native context and environment/data. A digest alone proves none
of these facts; this journal does not validate signatures or processes.

Pre-commit cancellation keeps the source/generation and retains any artifact or
receipt references already recorded. Cancellation after commit is refused; return
must use a new operation from the current peer/generation. Workspaces have separate
state and outcomes, so a failed/cancelled member does not undo another workspace.

## Durability and uncertain outcomes

The journal uses the endpoint metadata database with EXTRA synchronization. Current
state, retained operation metadata and generation changes commit atomically under
an immediate SQLite transaction. Each mutation requires the exact current revision,
operation ID and source generation. A stale revision returns no mutation, while a
wrong operation/generation/binding or skipped phase is refused. No automatic retry
or takeover is attached to this API.

A lost commit response is resolved by reading retained state; repeating its old
revision cannot commit another transfer. A new preparation requires a new operation
ID, even after cancellation. Old operation records retain artifact/receipt digests
through later transfers for explicit reconciliation/cleanup; this module deletes
neither snapshots nor operation history.

Reads compare the current revision with retained operation history and check its
matching operation metadata in one consistent
SQLite read transaction. Restoring only an old current-state row cannot hide later
retained progress. This is not protection against rollback of the entire trusted
metadata database. Missing/inconsistent history or a missing protected root
fails closed. A missing current row with retained history cannot be reinitialized.
The root replacement guards now count both handoff tables, including orphaned
operation history. These trusted local pointers must not be restored from ordinary
workspace backups or reconstructed from untrusted cloud-object listings.

## The endpoint root and its database

Each daemon has a protected 64-byte endpoint root (`cloud_vault_keys.rs`): an Ed25519
identity seed and a separate 256-bit recovery key, stored as one versioned secret in the
macOS Keychain or the Linux Secret Service (owner-only files under `secrets/` for a custom
home or a worker, see [the worker's keys](cloud-workers.md#the-workers-keys)). Its first
transfer creates it (`cloud.transfer.peer`); the libp2p peer ID derived from the seed is the
daemon's identity in every journal, and the archive preparation keys are bound to the root.
The root never leaves the daemon: no RPC returns it.

`cloud-vault-heads.sqlite` in the daemon's home (`cloud_vault.rs`, 0600, EXTRA
synchronization, apart from the main database) holds a domain-separated SHA-256 fingerprint
of the root, never its bytes, beside this journal's tables and the archive preparation keys.
If a fingerprint is pinned, a missing, corrupt or changed root fails closed, even on
creation. A missing fingerprint with existing journal or preparation-key rows also blocks
creation. After an interruption between creating the secret and pinning it, creation adopts
that same root only when no such rows exist. There is no reset or delete. The file and its
names come from the credential vault this database first served, removed since (ADR
[credential-vault-removed](adr/credential-vault-removed.md)); opening it drops the vault's
tables.

## Required integration before execution

All host launch/write entry points still need a durable local fence and
coordination with already running agents, terminals, scripts and services. Source
stop receipts must follow that fence and verified process termination; partial or
unregistered launches cannot count as stopped. The source remains blocked after
an uncertain commit until current ownership is reconciled. The destination must
verify the authenticated commit and complete restored context before execution.
`recorded_execution_peer()` is planning metadata, not permission to run a process.

Snapshot encryption/authenticated transport, native context, managed registration,
consistent development-data snapshots, conflict-aware return and cleanup tracking
remain separate work. Phone/Mac network loss and elapsed time do not advance or
cancel a journal operation. There is no automatic failover.
