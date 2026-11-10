# Activating a committed transfer

After the controller's journal commits a transfer, two local steps finish it. Both
live in the execution gate (`workspace_execution.rs`), beside the cancellation that
releases a source before a commit. Trusted coordinator code calls them with the
authenticated `Committed` state: for now the private RPCs
[`chauffeur cloud transfer`](cloud-workspace-transfer.md) drives.

## Destination: `activate_restored`

A restored workspace arrives quarantined (`workspace_restores`) and fenced with the
source's `Stopping` state ([restoration](cloud-workspace-restoration.md)). Activation
lifts both, in one durable transaction, only when everything names the same transfer:

- the state is `Committed`, and this daemon's peer is its target and its new
  execution peer;
- this daemon restored the workspace for that operation and snapshot, and its stored
  receipt (operation, generation, target, snapshot, workspace) hashes to the
  `destination_receipt_digest` the journal committed;
- the fence the workspace arrived with is that transfer's `Stopping` state, and no
  preparation of it is pending.

The committed state is kept in `workspace_restores.activated`; admission skips an
activated restoration. Activating again with the same state succeeds and changes
nothing; any other state refuses. The private clones under
`restorations/<operation>/` stay: the workspace's worktrees use them.

Activation does not log in. Claude and Codex on the destination use the user's own
login there (`claude login`, `codex login`, once per machine): restoration put the
conversations in that machine's native homes. Never an API key chosen silently.

## Source: `retire_committed`

The stopped source keeps its fence for good. Its `Stopping` state is replaced by the
`Committed` one, only for the same transfer and a later revision, once the stop and
every preparation have settled. Admission then refuses with "This workspace moved to
another machine with a committed transfer; it is read-only here, work on it there". No cancellation
can reopen it and no new transfer can fence it: only the workspace coming back
replaces it ([a workspace coming back](cloud-workspace-return.md)). Until then its
files, chats and native histories stay where they were.

## Origin

The package carries each worktree's `origin` URL
([package](cloud-workspace-package.md)): credentials written into an HTTP(S) URL are
dropped, and a local path or a transport helper (`file://`, `ext::`) is not carried.
Restoration sets it again on the private clone, so the destination fetches and pushes
with its own credentials. A package carrying a remote is refused by older daemons
(unknown field); one without is unchanged.

## Validation

A lifecycle test runs a whole transfer in one process with three disposable daemons:
the controller's real journal (`initialize`, `prepare`, every `advance`) drives the
source's complete stop, capture and sealing; the ciphertext crosses in bounded blocks
through the daemon artifact store; the destination opens and restores it, the
controller commits, and the destination activates. Before activation, and with the
published state or another peer, the destination refuses to run the workspace; after
it, its edited file, plan, chat session and credential-free origin are there. The
source then reads as moved and refuses cancellation and a new fence.
