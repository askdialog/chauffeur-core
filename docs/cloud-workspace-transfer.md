# Driving a transfer

A daemon drives each transfer of a workspace, in the background: it moves the
workspace from a source daemon to a destination daemon, a third daemon keeping the
controller's [journal](cloud-workspace-handoff-journal.md), over their private Unix
sockets. The Mac's daemon drives what its app starts (decided with Justin 2026-10-26),
so closing the app, or stopping the CLI that asked, does not stop it. One end can be the
driving daemon itself, called in process; the others are reached on their sockets.

With default settings, Chauffeur starts the local controller itself on the first transfer. Its
protected identity and journal are separate from the source's, in `transfer-controller/` under
the daemon's state directory. It comes back with Chauffeur after a restart, and is restarted if
its process exits. Stopping Chauffeur stops it, retaining its journal for the next run. No command,
extra LaunchAgent or controller path is needed. A configured custom controller is used as before,
never replaced, started or stopped by this manager.

## Machines

The machines a workspace can go to are declared in `settings.toml` (Settings → Transfer in the
app, decided with Justin 2026-10-26), or are [GCP workers](cloud-workers.md) Chauffeur created,
named the same way:

```toml
[transfer]
# Optional advanced override; omitted, Chauffeur manages its local controller.
# controller = "~/.chauffeur-controller/chauffeurd.sock"
[[transfer.machines]]
name = "vm"
socket = "/tmp/vm.sock"   # forwarded: ssh -N -L /tmp/vm.sock:/home/me/.chauffeur/chauffeurd.sock vm
```

Each machine is named once; sockets are absolute or under `~/`.

## Starting and following one

`cloud.transfer.start {workspace, source, destination, controller?, keep_local_copy, interrupt}`
takes each end as `here`, a `machine` of `[transfer]` or a `socket`, and the
controller from `[transfer]` unless named. What can be checked at once refuses there:
an unknown machine, a daemon that does not answer or runs another Chauffeur version
than the driving one (the transfer's messages change between versions), an unknown
workspace, a transfer of it already running. It returns the transfer as started; `cloud.transfer.status
{workspace}` gives the latest one, and each step is sent as `WorkspaceEvent::Transfer`
(log lines, then done or failed with its error, the workspace's new path, a kept copy).
A transfer running when the daemon stops reads as failed at its next start: each
machine kept where it was, and transferring again goes on from the journal. Sockets and
a named controller are accepted from the private Unix socket only (the CLI); apps name
machines.

A workspace transferred away reads `moved_to` (the machine of the last transfer that
moved it, or "another machine"); bringing it back is a transfer from that machine to
`here`.

```sh
chauffeur cloud transfer <workspace>... --to <machine or socket> [--interrupt] [--controller <socket>] [--from <source socket>]
chauffeur cloud transfer <workspace>... --back [--keep-local-copy] [--interrupt]
chauffeur cloud transfer <workspace>... (--to <machine or socket> | --back) --cancel
chauffeur cloud transfer <workspace>... --status
```

The CLI asks its daemon (or the one `--from` names) to drive, as the source, then prints
each step as the daemon records it; with several workspaces, each line starts with the
workspace's name, the command ends with each one's outcome and fails when one did not go
through. `--back` asks this daemon to bring the workspace
back from the machine it moved to. On the way back that replaces a retired copy (the
journal's next transfer, to the machine that copy is on), `--keep-local-copy` keeps
that copy beside the returning one even if it was edited since it left, instead of
refusing; it overrides nothing else ([return](cloud-workspace-return.md)). `--status` prints
the workspace's latest transfer this daemon drove, with a copy its source has not deleted
yet.

## Waiting for the end of turns

A transfer prepares while the workspace's agents work, and stops the source only once
their turns ended (decided with Justin 2026-10-19 and 2026-10-26). After the destination's
check and the journal's preparation, the driving daemon asks the source for the workspace's
chats every second: one working, starting, or waiting for the user's answer (an approval, a
question) is in a turn. While one is, the transfer reads `stage: waiting_for_turns`, with
`working_agents`, and nothing of the workspace is stopped or fenced: the user can still talk
to its agents, and a message sent meanwhile starts a turn the transfer waits for too.

- **Interrupt and transfer**: `cloud.transfer.interrupt {workspace}` (or `interrupt: true`
  at the start, `--interrupt` in the CLI) interrupts each agent in a turn once, then goes on
  as soon as their turns end (`interrupt` in `TransferInfo`).
- **Cancel**: `cloud.transfer.cancel` on a transfer that has not begun stopping its source
  (stages `preparing`, `waiting_for_turns`, `queued`) ends it as `cancelled` at its next look
  at the agents (at once while it waits; once its preparation ended while it prepares): the
  workspace runs on the source, nothing of it stopped, and its journal preparation is
  cancelled too. Agents already interrupted by **Interrupt and transfer** stay interrupted:
  cancelling does not start their turns again. A destination that refuses first ends it
  `failed`, nothing stopped either.

The source's stop begins right after the last look found no turn: a message sent in that
instant starts a turn the stop interrupts, as before.

## A batch

Several workspaces transferred together are one transfer each, started together
(`cloud.transfer.start` per workspace): each has its own progress and outcome, one refused
or failing leaves the others going, and nothing that went through is undone; a failed one
is resumed or cancelled as usual. A daemon drives at most two transfers past the wait for
their agents at once (stopping, sealing, sending, restoring, activating: decided with
Justin 2026-10-26); the others read `stage: queued` until a place frees, and look at their
agents again once placed, since a turn may have started meanwhile. The wait for turns and
the destination's setup scripts take no place.

A running transfer's `stage` says where it is: `preparing` (the destination's check),
`waiting_for_turns`, `queued`, `stopping` (stop, capture, seal), `sending`, `restoring`,
`activating`; none once it ended.

## Resuming or cancelling one

A transfer that fails, or whose driver stopped, leaves each machine where the journal
says it is. Starting it again between the same two daemons goes on from that step
("going on with the transfer left at snapshot ready"); any other transfer of the
workspace is refused while one is in flight. Each step can be asked again:

- the source keeps its capture in memory until sealed, and once sealed hands back the
  ciphertext it staged instead of sealing again: its binding is read through no-follow,
  bounded descriptors and checked against the journal, and the archive must open whole
  (staging cut short is refused, to cancel);
- the destination keeps what it received and restores once; a restoration that failed
  part way, never activated, is removed and restored again from the start. Restorations
  of one workspace run one at a time in a daemon, so a retry never removes one still
  running; one that was replacing a retired copy (a return) is never removed;
- a transfer committed whose run did not end done here (its driver stopped, even after
  retiring the source) is finished: activation and retirement are idempotent (setup
  scripts run again), and the copy a return leaves goes as usual. One that ended done is
  never run again: transferring the moved workspace from its old source is refused.

A preparation (capture, seal) left pending by an earlier run of the source's daemon
(it stopped part way) is settled when the transfer is resumed or cancelled: that process
is gone, so none of its work can still act. A capture that failed here is settled as it
fails: its task ended, so the transfer can be resumed or cancelled at once (the first real
trial on a GCP worker, 2026-10-29, found it waiting for a restart). A seal left pending by the
running daemon is not: a task of this daemon may still run it, so the source stays fenced
until its daemon restarts, then resume or cancel. A source stop that was interrupted needs
its explicit reconciliation, as before.

Cancelling a return leaves the workspace in the same ownership epoch on the worker. Its backups
resume, and the Mac's retired copy still admits the next return, even after repeated cancellations.
The cancelled fence stays on the worker to reject delayed stop messages; a later ownership epoch
or a transfer still in flight never qualifies as that same stay.

`cloud.transfer.cancel` (same parameters as `start`; `--cancel` in the CLI) cancels a
transfer that did not commit, holding the workspace's transfer slot in the driving
daemon so that no transfer of it starts meanwhile. The controller records it
`cancelled`; the source settles a capture it holds, drops its fence
(`release_cancelled`), so the workspace runs there again, and what the transfer staged; the destination drops what it received and a restoration it never
activated (`destination_abandon`). A destination that does not answer keeps those files
until the cancellation is asked again; the source never waits for it. The journal is
read under the owner each transfer records (`TransferInfo.journal_owner`), so cancelling
a return works without the machine it was going to. A resumed transfer asks again what
the failed one asked (`keep_local_copy`), unless one refused for an edited copy on its
destination is brought back keeping that copy ([the return](cloud-workspace-return.md)). A committed
transfer cannot be cancelled: the workspace runs on the destination, and bringing it
back is a new transfer. Cancelling a return that the original machine had restored
already removes that restoration: the workspace runs on the machine it was returning
from, which keeps it.

A transfer that failed before it asked the controller to prepare it, with no other transfer
of the workspace in flight (a refused folder, a destination that cannot take it, a worker
without a signed-in agent), stopped, fenced and staged nothing: it is recorded `untouched`.
Cancelling it, or any failed transfer the controller holds no journal of, only ends it on the
driving daemon (`cancelled`, "nothing of it had stopped"), without asking the controller
for a transfer it never recorded.

## In the app

- **Settings → Transfers** edits `[transfer]`: the controller's socket and one row per
  machine (name, socket), saved whole once a row has both.
- A workspace's menu has **Transfer…** (a ready workspace that runs here) and, once it
  moved, **Bring back here**. The dialog lists the declared machines (or offers to open
  the settings when there are none) and starts the transfer; bringing back offers to keep
  this Mac's copy if it was edited since it left. When an agent of the workspace works, it
  says the transfer waits for the end of its turn and offers **Interrupt and transfer**
  beside the main action.
- Several workspaces are picked in the sidebar as in Finder (decided with Justin
  2026-10-26): ⌘-click adds or removes one (the first also picks the workspace open),
  ⇧-click adds the rows up to it, **Select** in a row's menu does it from the keyboard, and
  a plain click or Escape clears the selection. A bar under the list says how many are
  picked, with **Transfer…** and a way to clear them. The confirmation lists the picked
  workspaces (those already moved or not ready greyed, with why; those whose agents work,
  that they wait), then the machine, then **Transfer N to <machine>**, and **Interrupt and
  transfer** when an agent works.
- Confirmation closes immediately, including while the startup request is pending, so the app stays usable. The sidebar’s Transfers button opens progress for a single workspace, a return or a batch: a line per workspace with its stage
  (checking, waiting for the end of turns, queued, stopping, sending, restoring, starting),
  then moved, cancelled, or the refusal or failure, with **Interrupt and transfer**,
  **Cancel**, **Resume**, **Retry** and **Cancel the transfer** where they apply, under a
  summary ("2 moved · 1 failed"). Closing it stops nothing; a button in the sidebar's footer
  ("Transfers 2/3") opens it again until the batch is dismissed, and each workspace's
  header follows its own transfer.
- The workspace's header follows the transfer the daemon drives (`cloud.transfer.status`
  when it opens, then `WorkspaceEvent::Transfer`, which is not kept in the history): the
  current step, then a refusal or failure with its message, or the return with a kept
  copy's folder; the log is under Details. A failed transfer between this Mac and a
  declared machine offers **Resume** and **Cancel the transfer**, which name the same
  ends again (`TransferInfo.from`, `to`; absent when an end is a socket). An `untouched` one
  offers **Retry** and **Dismiss** instead, saying the workspace still runs on its source;
  **Dismiss** cancels it on the daemon, so it does not come back when the header opens again. A moved workspace shows "on <machine>" in the
  sidebar and "runs on <machine>" with **Bring back here** in its header; its chats and
  scripts are unavailable here; Archive brings it back, its agents interrupted, then archives it here. Back here, until the machine it came from has
  deleted its copy, the header says "Its copy on <machine> is not deleted yet (<why>);
  <machine> tries again every 10 minutes" (`TransferInfo.discard_pending`, which this daemon
  follows until the copy is gone; [return](cloud-workspace-return.md)).
- While a transfer has not stopped its source, the header shows its stage; waiting for the
  end of turns, it offers **Interrupt and transfer** and **Cancel**.
- When a transfer this Mac drives ends moved, back or failed (asked by Justin 2026-10-29: be
  told once it is transferred), the daemon sends `RegistryEvent::TransferEnded` to every
  client. The app says it: a macOS notification with its sound while the app is not in front
  ("billing is transferred: it now runs on cloud-…", "billing is back on this Mac", "Could not
  transfer billing: <why>"), else a toast unless the user is looking at the workspace, whose
  header says it already. A cancelled transfer, which the user asked for, says nothing. The
  phone's push does not carry it yet: the relay's push service sends only the kinds it knows.
- Copy is French or English, as the Cloud settings are.

## On the phone

The phone reaches the Mac's daemon through the mobile host, which uses the daemon's
private Unix socket: the host's allowlist (`relay/src/bridge.ts`) is what keeps sockets
and a named controller the CLI's there. It admits:

- `cloud.transfer.status`, and `cloud.transfer.machines`: the names of the declared
  machines, with the default controller prepared automatically (the phone never reads settings);
- `cloud.transfer.start` and `cloud.transfer.cancel` between `here` and one `machine`, by
  name, with no controller and no other field than `keep_local_copy` and `interrupt`;
- `cloud.transfer.interrupt {workspace}`;
- `events.subscribe` to one workspace (no chat, no replay), of which only `transfer`
  events reach the phone: its other events carry terminal and script output.
- `previews.list` with the workspace only, and the tunnels of a moved workspace's declared
  previews ([relay](workspace-relay.md#previews-on-the-phone)).

The workspace's screen shows what the header shows on the Mac: the current stage (status,
then events, read again every 3 s while it runs), with **Interrupt and transfer** and
**Cancel the transfer** while it waits for the end of turns, a refusal or failure with
**Resume** and **Cancel the transfer**, the return with a kept copy's folder, the log under Details, and
for a moved workspace "runs on <machine>" with **Bring back to the Mac**. **Transfer…**
opens a sheet listing the declared machines, as the desktop dialog does; bringing back
offers to keep the Mac's edited copy. The list shows "on <machine>"; a moved workspace, or
one whose transfer runs or failed, opens on its screen rather than its only chat, and a
long press opens any workspace's screen. A moved workspace's chats read, without a
composer. A Mac whose host predates this refuses these methods: the phone shows nothing,
or asks to update Chauffeur on the Mac.

## Steps

```text
destination cloud.transfer.destination_check     can it take the workspace? else refuse, nothing stopped
controller  journal: read (register the workspace on first use), prepare, BeginStop
source      cloud.transfer.source_stop      complete stop → receipt digest      → SourceStopped
source      cloud.transfer.source_capture   package, kept in memory             → SnapshotPublished
source      cloud.transfer.source_seal      encrypted, staged; returns the key
relay       cloud.archive.source_open/read → receive_begin/write/commit, 256 KiB blocks,
                                                  16 reads and 16 writes sent ahead of their answers
destination cloud.transfer.destination_restore   open + restore (quarantined)  → DestinationRestored, Commit
destination cloud.transfer.destination_activate  activation, then each repo's setup script (held for
                                                  a GCP worker: destination_setup below); drops
                                                  the transfer's package and archive
source      cloud.transfer.source_retire          (moved, read-only); drops its package and archive
destination cloud.backup.configure                to a GCP worker only: its backups' key and bucket
                                                  ([backups](cloud-workspace-backups.md))
source      cloud.transfer.source_discard         only when the destination replaced a retired copy
                                                  (a return): the source deletes its own copy
destination cloud.transfer.destination_setup      to a GCP worker only, its setup held at activation:
                                                  the toolchains each repo declares, then the setup
                                                  scripts ([workers](cloud-workers.md#the-toolchains-a-repo-declares))
CLI         waits for the setup scripts, says how each ended
```

Every step is the daemon's own module, with its own checks: the
[complete stop](workspace-source-stop-receipt.md), the
[preparation](cloud-workspace-preparation.md), the
[artifact store](cloud-workspace-artifacts.md), the
[restoration](cloud-workspace-restoration.md) and the
[activation](cloud-workspace-activation.md). The CLI carries journal states, digests,
ciphertext blocks and the archive key from the source to the destination; it never
writes a daemon's files. `cloud.transfer.peer` gives each daemon's endpoint identity,
created on first use.

## Checking the destination first

Before the controller plans anything, the destination says whether it can take the
workspace (decided with Justin 2026-10-25). It refuses, naming what to do, when it
already has the workspace or a folder of its name, when it has no sidecar or no
working `git`, with less than 1 GiB free where workspaces go, or when an agent of the
workspace's chats (open or closed) is not logged in there: "Claude Code is not logged
in on vm: run `claude login` there, then retry". An agent whose login it cannot read,
and `gh` logged out (no pull requests from there), only warn. Nothing stops on the
source until the check passes; the CLI prints the warnings.

The source's folder is checked too, before anything stops: a file the capture would refuse (a
link outside the repos: "stray-link in the workspace folder is a link, which a transfer cannot
carry; move it into a repo or remove it, before transferring") fails the transfer saying nothing
was stopped. The folder's other files outside the repos and notes travel
([package](cloud-workspace-package.md)). The driving daemon checks its own folder before the
destination's check, and before preparing a GCP worker for a send to Cloud, and every source
checks again as it is asked to stop, since an agent may write one while the transfer waits for
its turn to end. The first real trial found it refused only at the capture, once the source had
stopped.

To a GCP worker of the Mac's daemon, the agents are signed in from the Mac instead (decided
with Justin 2026-10-29, [signing its agents in](cloud-workers.md#signing-its-agents-in)), and one
agent signed in there is enough, Claude Code or Codex, whichever the chats use (decided with
Justin 2026-10-30, [ADR 0048](adr/0048-one-agent-signed-in-on-a-worker.md)): the driver asks the
check about both agents with `sign_in_from_caller`, which lists the logged-out ones apart
(`logged_out`). With both logged out, the transfer fails before anything stops, saying "no agent
is signed in on w1: sign Claude Code or Codex in from Settings → Transfers (or `chauffeur worker
login w1`)", and keeps `needs_login` (the worker and both agents). The app does not show that
text: its notification, the transfer's header and the batch view say to sign one in, with "Open
settings" (Settings → Transfers) and "Sign in to Codex on w1" per agent. Once one is signed in,
the Mac's daemon resumes the transfer by itself, as Resume would. With one signed in, the
transfer goes on, its log saying that the other agent's chats wait for a sign-in there.

## Preparing it after the move

Activation gives the workspace, and each of its agent worktree groups, a port block of the
destination's own (`CHAUFFEUR_PORT`, a group's to its scripts; those it kept, coming back;
none for an exploration) and writes its `CLAUDE.md` and agent settings again, which name
the workspace's own block. It then starts the setup script each repo declares in its `.chauffeur/settings.toml`
(`[scripts] setup`), for every worktree, as a new workspace does: installing
dependencies is the destination's job, since ignored files never travel. The CLI waits
for them and says how each ended; a failed one prints its last lines and leaves the
workspace moved, its script to run again there.

Every private `cloud.transfer.*` method is admitted only over the private Unix socket,
like the archive bridge: the socket's OS user is trusted, and the public WebSocket
refuses them. A source capture waits in the daemon's memory for the controller to
publish its snapshot; the plaintext package itself is written under the daemon's home
(`workspace-preparation-<operation>/`, owner-only) and stays there after a failure, until the
operation's next capture replaces it. A daemon restarted in between loses the capture, and its
preparation stays pending for explicit recovery, its package kept.

The controller refuses a workspace that, by its journal, no longer runs on the
source: a moved source cannot be transferred again. The journal keeps the workspace's
first owner, so the CLI reads it under the source's identity, then the destination's
(a way back), and registers the workspace only when neither knows it. Transferring it
back to the machine it moved from, as the journal's next transfer, replaces the copy
retired there ([a workspace coming back](cloud-workspace-return.md)). A transfer step
that fails leaves what the module leaves (fence installed, files and keys retained)
until the transfer is resumed or cancelled ([above](#resuming-or-cancelling-one)); neither
happens on its own. Only the last step, deleting the copy a return leaves, is retried by
its daemon until it succeeds.

## Piloting it from here

A moved workspace's chats are piloted from the machine it left: its daemon relays the calls
about them to the machine it runs on. See [piloting a moved workspace](workspace-relay.md).

## Development services

A workspace's declared Docker services start again on the destination from their compose
files, and its exported PostgreSQL databases restore into them, before the commit; activation
makes them its declared services there. Coming back, the volumes of the containers the
workspace left there are set aside and those containers removed first, and the machine it
left removes its own containers and volumes with its copy. See
[development services](workspace-development-services.md).

## After the move

The destination runs the workspace with the conversations it carried, restored into
that machine's native homes. Agents use the user's login there (`claude login`,
`codex login`, once per machine; a GCP worker's signed in from the Mac); no API key is set
up in their place.

## Validation

`scripts/e2e-cloud-transfer.sh` starts three throwaway daemons, creates a workspace
on the source with an edited file, a plan and an `origin` holding a token, and runs
the command. The destination then shows the workspace ready, with the edit, the plan,
the origin without its token and a chat it can open; the source keeps its files,
refuses a chat ("moved to another machine") and refuses a second transfer before
stopping anything. With `REAL=1` (this machine's Claude login, a few cents), a Haiku
chat remembers a word on the source and recalls it on the destination after the move;
its transcripts are forgotten afterwards. That run found that a chat at the workspace's
root reached the sidecar with a trailing slash (`root.join("")`), refused as not
canonical: no real conversation had been restored before. The script also checks the
destination's refusal (a folder of the workspace's name there, then Claude logged out)
with the source untouched, and that the destination runs the repo's setup script once
it activates. It then makes the destination's workspaces folder read-only: the
transfer fails at the restoration with the source stopped (a command there is refused);
`--cancel` gives it back to the source; failing again, the transfer is resumed once the
folder is writable and goes on from "snapshot ready" to the move. Last, a batch of two
workspaces, one of whose folder the destination already has: the other moves, the refused
one still runs on the source untouched, the command fails naming it, and alone once the
folder is gone it moves too. A lifecycle test serves a destination and a controller on their
sockets and starts a transfer while an agent works: it waits with nothing fenced, interrupts
the agent when asked, is cancelled with the workspace as it was, and once started again
stops the source as soon as the turn ends; unit tests cover the wait, the interruption of
each agent once, the cancellation and the two places. The lifecycle test checks the destination's answer before and after the
restoration and that activation starts `setup repo-a`. A daemon test checks that the
public WebSocket refuses every `cloud.transfer.*` method.

## Recovery and preparation feedback

A transfer now keeps the computer awake under its existing power/battery policy while its driver is active. Closing the desktop window is safe; keep the driving computer reachable and awake until completion. Mobile control still uses this computer after the workspace reaches its destination.

A partial archive can restart from byte zero under the same authenticated binding once its former connection releases its writer. It does not resume an unverified prefix. Complete archives still require the full digest; another binding, an oversized file or a corrupt complete file remains refused. Response deadlines surface uncertain operations for explicit resume/reconciliation, never ownership changes.

`TransferInfo.preparation` distinguishes running setup, readiness and preparation needing attention from the committed transfer. Setup failure, missing outcomes and a 15-minute observation deadline preserve the destination's ownership and expose logs. Toolchain failures are reported separately from successful script exits. Processes offers an explicit retry of a setup in its original worktree; a setup already running there is reused.

The progress state reports elapsed stage time and acknowledged archive bytes, not a guessed overall percentage. Revisions order progress independently of log length. Before stopping, source file/Git/history sizes feed a conservative destination capacity check (four times the estimate plus 1 GiB). Actual archive size is checked again before restoration. Native context, live changes and external data can change this requirement; it is not a filesystem reservation or a guarantee against concurrent disk use.

Private restoration clones hold only Git objects. Captured file contents, index changes and untracked files are materialized and verified once in the linked worktree; file and directory durability checks remain in place.
