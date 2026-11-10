# Workspace Git snapshot primitive

`crates/chauffeurd/src/cloud_workspace_snapshot.rs` is the daemon-side file/Git
building block for the approved manual handoff. It is not exposed through RPC,
the app or mobile, and does not grant either host execution ownership.

## Captured state

- One selected worktree, including a `.git` file pointing to external metadata.
- A Git bundle of HEAD ancestry only; unrelated branch tips are not exported.
- HEAD and its branch, or a detached HEAD; exact staged binary changes.
- Regular tracked and non-ignored untracked files, including binary bytes,
  executable bits, unusual UTF-8 names, and tracked files missing from disk.
- Symbolic links, stored as target bytes in regular payload files and recreated without reading
  their targets; relative, dangling and external links retain their original targets.
- SHA-256 component/file digests and a versioned manifest with the expected Git
  status. The index and status must match after restoration, otherwise it fails.

The snapshot does not copy `.git`, remotes, hooks or arbitrary Git configuration.
Ignored files, repository-local secrets excluded by Git, local data, caches and
native Claude/Codex context are not implicitly selected. Required ignored data
and credentials must be handled by the explicit development profile and selected
cloud connections; this primitive alone is not a complete workspace snapshot.
Files tracked in Git may themselves contain secrets: the snapshot is private
plaintext on the trusted source and must be encrypted/authenticated before any
upload. Digests detect corruption; they do not authenticate a modified manifest.

## Source and destination boundaries

The caller must coordinate and stop all source writers and establish execution
fencing before committing a handoff. A second content/index/status read detects
ordinary concurrent changes, but is not a source-stop receipt or a consistent
snapshot guarantee against an active writer. No heartbeat or file hash authorizes
execution on another host.

Capture and restore perform blocking file I/O on Tokio's blocking pool, with Git
subprocesses using the daemon's Git CLI wrapper. Roots and files are private, files
and directories are synchronized, and the manifest is written only after capture
checks. This does not replace the future durable ownership/transfer journal.
Snapshots support up to 100,000 entries and 4 GiB including bundle/index/payload;
manifest and staged-patch metadata are limited to 32 MiB. Exceeding a limit fails.

Restore validates hashes and paths, creates a new private directory atomically,
fetches only the bundled HEAD, reconstructs the index without checkout filters,
and materializes verified payloads. Git hooks and automatic maintenance are
disabled during restoration. Any resulting difference in HEAD, branch, staged
patch or status fails instead of silently claiming a faithful restore. It never
merges into or overwrites an existing path; return conflict resolution and managed
worker/worktree registration remain integration work. On failure only its newly
created incomplete directory is removed; the source and snapshot are retained.

Paths must be canonical relative UTF-8 names without traversal, backslashes,
colon or `.git` components (case insensitive). Root and ancestor symlinks and
special entries are refused. Leaf symlinks are preserved, never followed. Restore rejects
overlapping manifest paths and creates links after regular files, so a link cannot redirect
payload writes. Retained-copy comparisons include link type as well as target bytes.
Before stopping a source, transfers check repo entry types and index support; capture repeats
these checks once writers have stopped. Submodules, unmerged indexes, sparse or
assume-unchanged entries, unborn repos and SHA-256 Git object format require
explicit future support; none are silently omitted. Platform-specific Git
semantics such as ignored file mode changes can cause a restore refusal.

## Validation

Real temporary repositories exercise linked metadata, staged/unstaged binary
changes, additions/deletions, detached HEAD, executable/untracked files and newline
names. Tests check that ignored data, remote configuration and unrelated branch
commits do not travel; corruption, unsafe paths, symlink escapes, existing
destinations and Git-status mismatches refuse restoration without overwriting.
These tests establish the local primitive, not a Mac–GCP–Mac handoff, encrypted
transport, portable native conversation state or execution fencing.
