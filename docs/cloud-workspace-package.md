# Workspace transfer package

`crates/chauffeurd/src/cloud_workspace_package.rs` captures one stopped workspace
into a single private folder whose manifest digest is what the
[handoff journal](cloud-workspace-handoff-journal.md) records as its snapshot. It
is the step between the [registered source stop](registered-source-stop.md) and
`snapshot_ready`. No RPC, app or phone action calls it yet. The destination daemon can now
[restore the package under quarantine](cloud-workspace-restoration.md).
[Endpoint encryption](cloud-workspace-archives.md) is available; host-to-host
delivery and ownership activation remain coordinator work.

## Contents

```text
manifest.json            version, stopping journal state, workspace, repos, chats, notes, profiles
repos/<i>/               one Git snapshot per active worktree (cloud-workspace-snapshots.md)
notes/<path>             every regular file of the workspace's .chauffeur/ (attachments, plans, designs)
loose/<path>             the workspace folder's other files, in no worktree and outside the notes
events/<chat>.jsonl      each chat's stored events: the history the apps show
contexts/<chat>/<id>.json the native conversation of each session of the chat
database_exports/<i>/    a declared PostgreSQL dump and its canonical source receipt
```

The manifest also keeps each chat's row (agent, model, permission mode, title, kind,
parent chat), its queued messages, the sessions it was forked from and the folder
its agent ran in. Each worktree keeps its repo, folder, branch, base branch and
commit, origin and group, and its `origin` URL when another machine can use it
([activation](cloud-workspace-activation.md#origin)). The package digest is the
SHA-256 of `manifest.json`; the manifest hashes file payloads, and each Git snapshot's manifest covers its own
bundle, index and files. Canonical database receipts are embedded in the manifest
and must match their packaged copies byte for byte.

`CLAUDE.md`, `AGENTS.md`, `.claude/` and `.mcp.json` at the workspace's root are
not carried: the destination's daemon writes them again for its own paths and CLI.
Any other regular file of the workspace folder outside a worktree and the notes (at its
root, such as Playwright MCP's `.playwright-mcp/` written where the agents start, or under
`.worktrees/` beside the registered agent worktrees) is carried under `loose/`, listed in
the manifest's `loose`, and restored at the same path, up to 20,000 files and 1 GiB;
`node_modules/` and `.venv/` folders among them stay behind, as a repo's ignored files do
(decided with Justin 2026-11-03, see [the ADR](adr/transfer-carries-files-outside-the-repos.md)).
A link, a special file, a Git repository the workspace does not have (a `.git` there, such
as a stale worktree's) or files past those limits refuse the capture, naming one, rather
than be left behind silently; a backup leaves them out and names them. So do a `.chauffeur`
that is not a folder, and a symlink, a special file or a name Git snapshots cannot carry
(`:`, `\`, `.git`) in the notes folder. Empty folders are not carried.

## Native conversations

For each chat, the daemon asks the sidecar's private `context.capture` to write the
conversation of its session, and of the sessions it was forked from, to a new file
of the package. The sidecar reads the folder the chat's agent uses (its
`CLAUDE_CONFIG_DIR` / `CODEX_HOME` in the chat's environment over the sidecar's,
else `~/.claude` / `~/.codex`), with the strict
[Claude](claude-context-bundles.md) and [Codex](codex-context-bundles.md) capture:
Claude's session and subagent files under the project key of the exact folder the
session ran in, Codex's rollout found by its id under `sessions/` or
`archived_sessions/` and the parents it was forked from. It refuses a chat with a
session it is running, and the daemon refuses one it still has live.

A conversation whose history is missing, damaged or unsupported comes back
`blocked` with its reason, and nothing is written. The chat is kept in the package
with that reason and no conversation: per the SPEC it cannot resume elsewhere, its
native history stays on the source, and a new conversation with a summary needs an
explicit user action. A failure of the capture itself (the sidecar, a file the
daemon cannot hash, a sidecar file that differs from what it reported) fails the
whole package instead, to be retried.

## Preconditions and limits

`capture` first requires, through `ExecutionGate::require_settled_stop`, that the
workspace carries exactly the given stopping fence, that its registered stop attempt
for that fence settled, and that no admission or stop task is in flight. It checks
again after writing the manifest. While fenced, the user's edits of the workspace and its chats
are refused ([execution fence](workspace-execution-fence.md)); chat rows, queues and
the last stored event are still read again after the package is verified, and any
change the daemon itself made fails the capture, to be retried.
This is evidence about registered agents, scripts
and terminals only: detached descendants, services, database writers and external
programs are not covered, and the capture's rereads are a tripwire, not a proof
against a concurrent writer.

The output folder must be new and outside the workspace. On any failure the folder
this call created is removed; the workspace, its fence and the native histories are
never touched. The fence stays installed after a successful capture.

`verify` rereads the manifest, checks every file it lists, checks each Git snapshot
against its own manifest (`cloud_workspace_snapshot::verify`, also run before every
restore), and refuses any file nothing lists. An unblocked chat must carry the
conversation of exactly its sessions, those it was forked from then its own. Hashes detect corruption, not tampering: the package is
private plaintext on the trusted source and must be encrypted and authenticated
before it leaves the Mac. Limits: 100,000 notes files and 4 GiB of notes, 512 MiB
per conversation file, 1 GiB of events per chat (read in pages of 100, ended before a payload
would take the page past the remaining budget), a 64 MiB manifest, plus each Git
snapshot's own limits.

## Declared database snapshots

Version 3 carries verified [PostgreSQL logical exports](workspace-postgres-exports.md).
The current daemon-owned profile requires exactly one completed export for every
declaration under the exact stopping fence and profile head. Missing, stale,
duplicate or unresolved exports refuse capture. Completed exports from other
operations remain in the source store; they cannot satisfy a current declaration.
Imported historical snapshots remain provenance and are carried on later hops.
Every retained PostgreSQL declaration must have its corresponding snapshot;
legacy profile-only packages containing such declarations now refuse rather than
silently losing the required data.

The manifest covers the dump's full SHA-256 and size, original source receipt,
profile and fixed index. Each inner receipt must match its exact canonical JSON
bytes, so whitespace or content alterations fail verification. Unlisted files,
unsafe paths, repeated receipts, changed payloads and profile/source mismatches
refuse. Capture rereads its bounded metadata inventory after final verification;
copying and hashing large payloads run outside the async executor.

Workspace restoration copies these snapshots into independent private directories
under the destination daemon's restoration root, verifies and syncs their bytes,
and installs their metadata in the same transaction as the completed quarantine.
Deleting or moving a received transfer package does not remove these retained
copies. No SQL runs during workspace restoration. The separate internal
`restore_packaged` entry point selects a retained index for an explicitly
configured [quarantined target](workspace-postgres-restores.md); historical snapshots
from an earlier operation cannot authorize the current restoration.

The package retains at most 32 distinct database snapshots, bounded by 16 GiB
total dump bytes. The complete package must also fit the existing encrypted
archive format: 16 GiB including file framing and UTF-8 paths, 200,000 files,
200,000 directories and separate 64 MiB file/directory path budgets. Metadata
preflight checks this before payload verification. A dump approaching 16 GiB may
therefore not fit alongside repositories, context and framing; the package refuses
without deleting the original export. No archive wire limit is raised.

Versions 1 and 2 remain supported for packages without declared PostgreSQL data.
Older daemons refuse version 3. Logical export custody does not establish final
writer quiescence, complete meaningful-data coverage, application-level restore
validation or permission to stop meaningful-data services or activate a worker.

## Not covered yet

Host-to-host delivery and key distribution, destination activation and incomplete-restore reconciliation,
other development data and services, ignored files the development profile must
select, and the journal's `snapshot_ready` advance by an authenticated coordinator.

## Validation

A disposable daemon with a stand-in sidecar creates a workspace with an edited
worktree, a plan in its notes and two chats, one with a queued message. Capture is
refused before the registered stop, then after it packages the repo, the plan, both
chats' rows, queue and history, and the conversation of one; the other comes back
blocked and is kept without a conversation. A file in `.playwright-mcp/` travels, a
`node_modules/` beside it does not. Tampered and extra files fail `verify`;
an existing output folder, a link at the root, a stale worktree under `.worktrees/`, a
`.chauffeur` file, a symlink in the notes, a sidecar failure and a queue edited
while the conversations are captured each refuse the capture and leave no folder.
A changed Git payload, and an unblocked chat stripped of its conversation, fail
`verify`. Sidecar tests cover the real
Claude and Codex capture through `context.capture`: the chat's own config folder,
rollouts found once (archived or not), refusal of a running session, a missing
history reported as blocked, and no file ever replaced.
