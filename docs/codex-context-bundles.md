# Selected Codex context bundles

`sidecar/src/agents/codex/context-bundle.ts` captures native history for one stopped
Codex thread. The Mac–Linux–Mac compatibility proof now uses this adapter instead
of copying a single rollout. It has no production RPC or chat launch caller.

## What travels

The selected rollout travels byte for byte, including opaque native compaction
items, original tool records and historical paths. A fork or revert can name a
`session_meta.payload.history_base`: the bundle recursively includes that parent
and its ancestors, each truncated at the exact referenced `end_byte_offset`.
Later parent turns and unrelated conversations do not travel. A dependency may be
in `sessions/` or `archived_sessions/`.

Capture scans filenames to locate unique thread IDs, not the contents of other
conversations. It verifies each selected file's native metadata ID and resolves
all dependencies. Missing or ambiguous parents, cycles, offsets outside a file or
in the middle of a record, incomplete JSONL, invalid UTF-8 and unsafe paths fail.
Unknown native record contents are preserved, not rewritten into a summary or
replayed by this adapter. The real runtime must still validate and resume them.

No auth file, SQLite store, shell snapshot, configuration, skill or personal home
is copied. Account access and environment preparation remain separate explicit
flows. Claude continues to use the existing synthetic proof's session-store
adapter; production Claude context export is not supplied by this module.

## Boundaries and restore

A bundle contains its version, selected session ID, relative file paths, base64
bytes and SHA-256 digests. These are integrity checks, not authentication or E2E
encryption. The bundle is sensitive plaintext. A trusted daemon-owned caller must
bind the complete artifact to the authenticated handoff and encrypt it before
transport/storage. No cloud upload is added here.

Capture requires an explicit canonical source home and a selected rollout path;
it never defaults to the user's personal Codex home. Limits are 33 files (the
selected thread plus up to 32 ancestors), 64 MiB per file, 256 MiB total, 100,000
scanned entries and bounded directory depth. Symlink sources/ancestors and
hardlinked selected files are refused. Repeated reads and file identity checks
catch ordinary concurrent changes; they do not establish writer quiescence.
The source must already be stopped and fenced, including other writers of its
native history. This module cannot issue a source-stop receipt or grant ownership.

Restore validates all hashes, canonical encoding, paths, native IDs and the
complete dependency chain before creating any files. Extra unrelated threads and
extra parent suffixes are refused even if their hashes are valid. It creates only
a **new** home with private directories/files (0700/0600), synchronizes files and
directories, and removes only that newly created incomplete home on failure.
An existing destination is never merged, overwritten or deleted. Caller-owned
execution/transfer journals must prevent any runtime using the home until restore
and all other handoff validations finish.

## Evidence

Eight isolated tests exercise nested archived parents, prefix selection, opaque
compaction bytes, corruption, missing/extra/duplicate files, cycles, invalid
boundaries, symlinks, limits, existing destinations and private modes. The existing
history reader sees the same model items before and after restoration.

On 2026-10-21, `scripts/e2e-session-portability.sh` passed all scenarios with the
pinned runtimes: text, tools and native compaction for Claude and Codex, plus a
real Codex fork whose rollout references a separate parent. Each runs on Mac,
network-isolated Linux Docker and a returning fresh Mac home. Codex uses this
capture/restore adapter on each hop. Native session IDs and ordered context stay
unchanged; tools retain all eight prior results and compaction is not repeated.
A copy with the fork's parent removed is refused before inference. Existing
negative compaction controls still refuse missing native compaction state.
Inference is a local deterministic fixture, not a hosted provider compatibility
or subscription authorization test. This does not establish full workspace
transfer, GCP deployment, Claude production export or conflict-aware return.
