# Destination workspace restoration

`cloud_workspace_restore::restore` is an internal daemon API for manual handoff.
Packages can first be received through the
[encrypted archive API](cloud-workspace-archives.md). Restoration accepts a package, an authenticated `snapshot_ready` journal state and the
authenticated local destination peer. It does not expose an RPC, provision
credentials, commit ownership or start agents, services, scripts or terminals.

## Binding and quarantine

The daemon verifies every package payload and matches the stopping operation,
owner/controller binding, source generation and source/destination peers against
the published state. It encodes the package's SHA-256 bytes as base64url without
padding to compare with the journal's snapshot digest. The source package API
exposes those same bytes in hex. Wrong digests or peers refuse registration.

A durable transaction reserves the original workspace ID and slug with a source
fence and an independent quarantine in `workspace_restores`, before workspace
files are created. Every execution admission checks that quarantine, including
after restart. Cancelling the source fence cannot release it. The destination
remains quarantined until authenticated ownership and readiness coordination
explicitly permits execution; a restoration receipt is not execution authority.

The workspace folder must be new. Private clones and database exports live in
`<CHAUFFEUR_HOME>/restorations/<operation>/` (the receipt's `native_root`). Existing
workspaces are never overwritten, nor any file of the machine's native homes. Worktree paths must match their declared origin and
group; nested, duplicate and reserved layouts are refused.

## Files and native context

Each snapshot becomes a private clone and a registered linked worktree at the
destination-relative path. The workspace folder, its worktrees' files and its notes
are created as local ones are, under the user's umask (a `0755` script stays
readable by a container running as another user); the restoration's own folder,
with the clones and database exports, stays owner-only. HEAD, branch, index, tracked deletions, executable
flags, local files and Git status are checked. Materialization disables Git
hooks. Notes are copied with their hashes; generated instructions and config use
the destination's paths and CLI. No repository code executes.

The private sidecar `context.restore` checks each selected bundle's length,
SHA-256 and session identity. Claude sessions are grouped by the SDK project key
of the destination cwd, including separate design folders; identical shared
sessions are installed once. Codex forks share their parents: repeated parent
rollouts merge only when their bytes agree or one is an exact prefix of the
other. Conflicting prefixes and multiple paths for one session ID are refused.
Native records, opaque compaction state and historical paths retain their bytes.

Conversations go into the destination's own native homes, where its agents read
their login, resolved as they resolve it from the daemon's scrubbed environment:
`CLAUDE_CONFIG_DIR` when set, else `~/.claude`; `~/.codex` (the scrub drops every
`CODEX_` variable, so agents Chauffeur starts never get `CODEX_HOME`). A home is
created private when missing and only when something goes there, and resolved
through symlinks. The user
logs in once per machine (decided with Justin 2026-10-24). No file is replaced or
rewritten: missing files are added; one already there that starts with the carried
bytes is kept (a retry, or a conversation this machine went on with); one that is
their start gets the rest appended (a conversation that came back longer); any other
file in the way refuses the restoration, which takes back every file and folder it
added and cuts every file it extended back to its size before. It returns the files and
folders it created (never one it kept or extended, nor a home it had to make), recorded as
the restoration's `native_files`: deleting the copy after a return removes exactly those
([return](cloud-workspace-return.md)). Bundles are
validated before the first write; files in the way are found while adding. The workspace's agents get no home of their own.
Credentials and provider configuration are not copied. Managed account and
environment readiness remain prerequisites for activation. A private clone gets
back the `origin` the package carries, without credentials, and no other remote.

## Publication and recovery

After filesystem synchronization and package revalidation, one durable database
transaction imports worktrees, chats, fork chains, queues and events, marks the
workspace ready and records the operation-bound receipt. Events receive local
sequence numbers; timestamps and per-chat order remain. Streams are hashed again
during import. Attachments in queues and history point to copied notes; missing
attachments refuse publication. Existing workspace, chat and queue IDs are never
replaced. Pinning and sidebar layout remain local.

A source-blocked chat keeps its explanation and native resume ID. It has error
status and a persistent per-chat resume block, so later activation cannot turn
it into an empty conversation. A new conversation requires a new explicit chat.

Failure after reservation leaves `cleanup_failed`, retained files and persistent
quarantine. No partial chat import commits or recoverable copy is deleted; an
uncertain sidecar result never authorizes cleanup. Repeating a completed operation
returns its durable receipt. An incomplete operation refuses overwrite and asks
for explicit reconciliation. Incomplete-operation cleanup/retry is subsequent coordinator work;
[activation](cloud-workspace-activation.md) lifts the quarantine after the commit.

Limits include package budgets, 64 MiB per imported event and 512 MiB of native
bundle JSON per restoration request. Native adapters also enforce their decoded
content budgets. Unsupported sizes fail explicitly.

## Validation

Temporary-daemon tests cover indexed/working-tree changes, untracked files,
linked-worktree registration, notes, attachments, queues, history, blocked chats,
exact snapshot/destination checks, durable receipt retry, quarantine after store
reopen and source cancellation, and retained files without partial publication
on native failure. Sidecar tests cover real Claude/Codex bundle adapters, cwd
relocation, shared Codex parents, conflicting-prefix refusal, corruption, and
restoring into a machine's homes: a missing home created, the machine's own files
kept, a retry accepted, a different file in the way refused with what the attempt
added taken back. No personal conversations or native
model sessions are used.
