# Daemon-owned workspace archive artifacts

`cloud_workspace_artifact` implements the local filesystem portion of archive
delivery inside the Rust daemon. These blocking internal APIs sit behind the
[private Unix host bridge](cloud-workspace-bridge.md): host dispatch
must run them off the async executor and authenticate current operation/enrollment
state before each operation. No public listener, execution permission or
automatic cleanup is added.

## Retained layout and binding

Each endpoint keeps one operation in its private daemon home:

```text
workspace-archives/                  0700
  <operation UUID>/                  0700, exclusive descriptor lock
    binding.json                    0600, immutable operation/reference binding
    archive.bin                     0600, selected ciphertext only
```

The binding contains the complete snapshot-ready transfer context, local endpoint
identity and authenticated ciphertext digest/length. The daemon checks published
state and the source/destination role before touching storage. The controller
cannot occupy either execution endpoint. No key or native account credential is
stored here; protected per-operation key retention remains in the endpoint vault.

`stage_source` copies an endpoint-local sealed file into this layout, verifies the
actual staged ciphertext and synchronizes it. The original sealed source stays in
place. `source_reader` opens only an existing staged source with the exact binding;
it pins the verified descriptor and refuses mutations or invalid read ranges.

`receive` returns either a new sequential writer or an independently reverified,
synchronized completed receipt. `allow_new=false` cannot create even an empty root
or operation folder. An existing operation with a missing binding, missing archive
or partial/incorrect ciphertext always refuses, even with `allow_new=true`. It
never resets an operation from a metadata claim or truncates a previous file.

## Filesystem and durability checks

The configured daemon home is resolved once; subsequent walks use pinned directory
descriptors, `openat`, `O_NOFOLLOW` and exclusive creation. Every retained directory
must be owned by the current endpoint user and have mode 0700; regular files must
have mode 0600 and one hard link. Symlinks, FIFOs and other special entries refuse
without following or blocking on them. The retained manifest is bounded to 8 KiB.

An exclusive nonblocking directory lock prevents two cooperating host jobs, including
separate processes, from using the same operation. Dropping a handle explicitly
unlocks it, even if an unrelated concurrent subprocess briefly inherited the open
description before exec; it never unlinks any entry. Failed/uncertain writes
remain reserved and require explicit reconciliation. Complete ciphertext left by a
lost receipt can be rehashed and synchronized to recover that receipt.

Writes require sequential offsets, a nonempty block of at most 256 KiB and the bound
total length (at most 16 GiB plus 36 bytes). Any failed write poisons that handle.
Commit independently reads and hashes the actual file, checks its metadata before
and after verification, synchronizes the file and retained parents, then checks
metadata again. Pinned inode checks detect removal, replacement or relocation of
the root, operation folder, binding or archive. Binding mutations and source changes
also refuse. These checks detect ordinary concurrent local changes; they do not
isolate the daemon against malicious processes with the same OS identity.

A stored receipt proves ciphertext durability at that point only. No AEAD decoding,
package verification, native readiness, source-stop proof or ownership commit is
implied. Later reuse must independently reverify the file, and normal authenticated
archive opening and quarantined restoration still follow. The source is never
removed by these APIs.

## Proof and remaining integration

Unit tests cover staged multi-block transfer, exact retry without inode replacement,
lost receipt, partial/missing files, binding substitutions, source mutations,
overlapping handles, limits, links, modes, special files and renamed entries. The
existing isolated restoration test now composes real encryption, this descriptor-based
staging/reception, archive verification and quarantined linked-worktree restoration.
It uses temporary state and fake native contexts, with no native model session.

The TypeScript Noise delivery library now has a private host bridge with scoped
handles, host-owned connections, cancellation and deadlines. Installed host dispatch,
channel provisioning,
authenticated controller journal orchestration, full environment/data readiness,
activation and explicit retention cleanup remain integration gates before real
workspace handoff.
