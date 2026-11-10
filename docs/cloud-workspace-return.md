# A workspace coming back

After a committed transfer the source keeps a retired copy, read-only and marked moved
([activation](cloud-workspace-activation.md)). When the workspace comes back to that
machine, the returning version replaces the copy (decided with Justin 2026-10-26):
same folder, same workspace in the app, its conversations continued. The copy is
removed only once the return is restored; until then a failure puts it back.

`cloud_workspace_return.rs` holds the steps; `cloud_workspace_restore::restore` calls
them when the workspace already exists here.

## Which copy a return replaces

Only a copy retired by the journal for this very return: its fence is the `Committed`
state of the same binding, that commit moved it from this machine to the source of the
coming transfer, and the coming transfer is the journal's next one (its source
generation is the commit's). A workspace here in any other state refuses, as before:
"archive it before receiving it again". The destination check
([transfer](cloud-workspace-transfer.md)) applies the same rule before anything stops:
the CLI sends it the controller journal's current state, which must be that commit or
a later cancelled departure with the same binding, execution peer and ownership epoch.
Cancelling a departure advances the revision without moving the workspace, so repeated
cancellations still allow a return to replace the folder it finds. A pending transfer or
a different ownership epoch does not.

## Order

```text
reserve      one transaction: the retired row becomes the arriving one ('creating', the transfer's
             Stopping fence), its restoration row is replaced, and what it was is kept in
             workspace_returns (fence, state, folder, old restoration row, worktrees)
set aside    <workspaces>/<slug> → <workspaces>/.<slug>.replaced-<operation>
restore      as for a new workspace, into <slug>: private clones, worktrees, notes, conversations
import       one transaction: the old chats, queues, history, worktrees and imported
             declarations give way to the arriving ones; title, branch and tickets follow
clean up     each old worktree is relinked to the folder set aside (`git worktree repair`), its
             ignored files moved into the returning worktree, the folder removed, its clones
             pruned, and an old restoration folder removed
attach       each returning worktree moves onto the user's clone the copy's was linked to, and its
             private clone is removed (below)
```

A failure before the import puts the copy back: what the attempt made of the folder
is kept as `.<slug>.failed-<operation>`, its private folder as
`restorations/<operation>.failed`, and the folder, row, fence and restoration row come
back as they were. The copy reads as moved again and the transfer can be retried. What
the clean-up cannot remove is logged and left. A daemon stopped between the set-aside
and the import leaves the `workspace_returns` record for explicit reconciliation; it
is not resumed automatically.

The relinking matters: the arriving worktrees sit at the very same paths, linked to the
restoration's private clones, so `git worktree prune` alone would keep the old records
of the user's clones, which would go on holding the workspace's branch.

## Edits made to the copy while it was away

The copy is read-only for Chauffeur, not for an editor or a terminal: a file written in it,
a commit made in its worktree or a note changed there would be overwritten by the return.
So source preparation records what the workspace held when it was captured to move, as
the package carries it (`workspace_departures`: each worktree's commit, branch, staged
changes and every carried file's SHA-256 and mode, each notes file's SHA-256, all read
from the verified package). A return reads the copy the same way, writing nothing, and
any difference refuses it, the first five paths named. A file added in the workspace
folder outside its worktrees and notes counts too: no package carries it.

```text
The copy on <machine> changed since the workspace moved (alpha/local.md, alpha/ (its commit
or branch)); transfer again with --keep-local-copy to keep it beside the returning one
```

The destination check refuses before anything stops on the other side, and restoration
checks again before setting the copy aside. Once the return is in, the copy is read a last
time before it is removed: an editor may still have written to it meanwhile. Changed then,
or holding an ignored file that could not move into the returning worktree, it is kept as
`<slug>-local-copy` (when that name is free) and the CLI says where, as on request. A copy with no record (captured before this
check existed) refuses the same way. With `--keep-local-copy` the return goes on and the
copy is set aside as `<slug>-local-copy`, beside the workspace, instead of removed: its
worktrees stay linked to the user's clones and to an old restoration's, which are kept for
it. The CLI says where it is; the user removes it once compared.

The refusal is typed, at either check: the destination answers it as a conflict whose data
holds `local_copy_changed`, which the driving daemon keeps as it passes another daemon's error
on, and the failed transfer records it (`TransferInfo.local_copy_changed`). The app's transfer
header and batch view then offer "Bring back keeping this copy" beside Resume: the same transfer
asked again with `keep_local_copy`, the only way from the app to keep a copy edited in the
middle of a return.

Ignored files (an `.env`, dependencies, build output) never travel and are no edit: once
the return is in, each moves from the copy into the returning worktree, unless something
already is in its place, so the user's local settings and caches stay.

## The copy the workspace leaves

A return is the end of a round trip, so the machine the workspace leaves deletes its
own copy (approved scope: "after verified return, delete the cloud workspace"). Once
the source retired it, the CLI calls `cloud.transfer.source_discard` with the commit;
`cloud_workspace_discard` refuses unless the copy is retired by exactly that commit,
then records the deletion in `workspace_discards` before it starts: the folder, the
links in the clones it was not restored into (`git worktree prune`), its restoration
folder (private clones, database exports) and every row of the workspace go, and the
apps are told it is removed. A failure leaves the copy retired and the deletion
pending: the CLI says so ("its copy on <machine> could not be deleted yet (…); it is
retried there") and the daemon tries again at start and every ten minutes. The
workspace already runs where it went; nothing waits for the deletion. Coming back
again before it happened, the return replaces the copy and drops the pending deletion.

The machine that drove the return follows a pending deletion: the transfer reads
`discard_pending` (why), its source is asked again (the ask is itself a try) at the
daemon's start, every ten minutes and when a client reads the transfer (at most every ten
seconds), and once the copy is gone the field clears, a line "deleted its copy on <machine>"
is logged and the apps are told. The workspace's header in the app says "Its copy on
<machine> is not deleted yet (…); <machine> tries again every 10 minutes" until then, and
`chauffeur cloud transfer <workspace> --status` prints it. Only the workspace's latest
transfer is followed.

The deletion also removes the conversation files the copy's restorations wrote into that
machine's native homes (`~/.claude/projects/<key>/…`, `~/.codex/sessions/…`), and only
those. Each restoration records what the sidecar created there, files and the folders made
for them, in `workspace_restores.native_files`; a return that replaces a copy keeps the
copy's record too, since those files are the workspace's still. Never removed: a file that
was already there (kept or extended by the restoration, the machine's own), a home the
restoration had to create, anything outside the recorded homes, a path that is no longer a
regular file or is reached through a symlink, and a folder that is not empty (a
conversation the machine started there since stays, with its folder). A conversation file
that cannot be removed leaves the deletion pending; a recorded folder that cannot be is left
behind without blocking it.

Both sides of every committed transfer also try to remove its transfer files once done:
the destination at activation (the plaintext package it opened, the ciphertext it
received), the source at retirement (the preparation folder and staged ciphertext).
This is best effort: a file that cannot be removed is logged and stays, with no retry.

## Conversations

Carried conversations go to this machine's native homes, where the copy's own already
are ([restoration](cloud-workspace-restoration.md)). A file that comes back longer than
the one here (the destination went on) is extended; one already there that starts with
the carried bytes is kept; anything else in the way refuses. A refusal takes back what
the attempt did, extensions cut back to their size before; a return that fails after the
sidecar wrote them tries to remove the files it recorded as created (not the copy's) as it
puts the copy back, logging what it cannot.

## After a return

Restored, the workspace's worktrees are linked to the private clones of the return's
restoration (`restorations/<operation>/repos/N`). Once the copy is removed, each moves back
onto a user's clone, so that Git, pull requests and archiving behave as for a local
workspace: the clone the copy's worktree at the same path was linked to, or, for a worktree
the copy did not have (an agent's, or a copy that had arrived by transfer itself), the
repo's clone here as a new workspace would find it. In that clone:

```text
prune        stale worktree records (a kept copy's folder removed by hand) let go of the branch
fetch        from the private clone: the branch (forced in the copy's own clone, whose branch the
             returning one replaces; else only fast-forwarded) and the index's tree as a
             temporary commit, so staged blobs come too
link         a worktree registered at a scratch path without checkout takes the index, then the
             returning worktree's `.git` points at it (`git worktree repair`)
check        HEAD, branch, the index's tree and the tracked status read as before
```

The files are never touched. The worktree's row then names the user's clone and its private
clone is removed. A Git step failing puts back what it did (the link, the clone's branch) and
leaves that worktree on its private clone, logged, as before this step existed; recording the
row or removing the private clone afterwards is best effort, logged. A daemon stopped between
the import and this step leaves the worktrees on their private clones, as it leaves the
clean-up undone. A copy kept
beside the returning workspace (on request, or because it changed meanwhile) holds the
branch in the user's clones with its own worktrees: then the returning worktrees stay on
their private clones.

## Validation

The lifecycle test of the first transfer goes on with the way back: the destination edits
a file, the controller's journal prepares the next transfer, the destination stops,
captures and seals, the archive comes back, a first restoration fails on purpose (the
copy is back as it was, still moved), the second replaces it: the edit and the plan are
there, the chat kept its session, the folder set aside is gone, the user's clones no
longer list the old worktrees, the workspace runs here and is retired on the
destination. Before it, a file written in the copy after it moved refuses both the check
and the restoration (named, the copy still moved), and an ignored `.env` written there
is no edit and is found in the returning worktree. `scripts/e2e-cloud-transfer.sh` brings
the workspace back with a title given on the destination: a file written in the copy
refuses the return before anything stops, and `--keep-local-copy` brings it back with the
edited copy kept beside it as `moving-local-copy`; with `REAL=1` a Haiku conversation carried out and back still
remembers its word, its file here extended rather than duplicated.

After the return, the worktrees name the user's clones again, which list each once, hold
the commit made on the destination and its staged change, and the return's private clones
are gone. Then the test removes the destination's copy: a commit other than the one retiring
it is refused; a folder that cannot be removed leaves the deletion pending and the copy
retired; the next try deletes folder, restoration folder, rows and the conversation file the
restoration wrote into the destination's Claude home (a file of the machine's own beside it
stays), and a third finds nothing. The returned workspace then archives with its snapshots in
the user's clones. A unit test checks what a deletion never removes (a symlink, a path
through one, a file outside the homes, a home, a folder not empty). The activated and retired sides no longer hold the return's archive or
preparation folder. The script checks the machine left no longer lists the workspace and
that neither side keeps a transfer package, archive or preparation folder. A second
workspace then goes and comes back without a kept copy: its worktree is the user's clone's
again, with the commit and the staged change made away and no private clone left; a folder
the destination cannot empty keeps its copy pending, which the return's JSON and
`--status` say, and once unlocked the next read clears it and the copy is gone; archived,
its snapshot is in the user's clone. With `REAL=1`, the conversation file the destination's
restoration wrote is gone with its copy.
