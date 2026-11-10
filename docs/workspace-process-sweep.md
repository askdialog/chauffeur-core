# Sweeping a source workspace's remaining processes

`workspace_source_stop::sweep_processes` follows the registered stop
([registered-source-stop.md](registered-source-stop.md)). The registered stop ends
what the daemon knows: native sessions, script groups and terminal children. It
cannot see a background job an agent detached, a dev server started with `&` that
outlived its shell, or a launch a previous daemon generation lost. The sweep looks
for them in the operating system's process table and ends them, and it refuses
when a program the workspace did not start works or writes in it. No public RPC
calls it yet; trusted handoff code runs it under the exact `Stopping` fence.

## Which processes belong to the workspace

`workspace_processes` scans every process of the daemon's user but the daemon
itself, natively (libproc on macOS, `/proc` on Linux, about 0.1 s for 700
processes). A process *touches* the workspace when its working directory, or a
descriptor it opened for writing, lies under one of the workspace's roots: the
workspace folder (worktrees and notes), worktrees outside it and the private folder
of a restored workspace (its clones). Read-only descriptors do not count, so Spotlight or a `tail`
does not block a transfer.

- **Linux.** Agents, scripts and terminals start with `CHAUFFEUR_WORKSPACE_ID`. A
  process whose initial environment (`/proc/<pid>/environ`) holds that entry is the
  workspace's, whether it touches the workspace or not.
- **macOS.** The system no longer shows another process's environment, even a
  child's, and nothing else follows an agent's commands: Claude Code runs each Bash
  command in a new session and passes none of its descriptors, and the kernel's
  "responsible process" is not inherited once the daemon runs outside an app (seen
  on a LaunchAgent daemon: every process was its own). So a process that works or
  writes in the workspace is the workspace's when:
  - it descends from the daemon (a running agent, script or terminal); or
  - its session is not a living program's: the session's leader is gone (the
    shell of an agent's command, a closed terminal, a finished script), descends
    from the daemon, or is itself an orphan of launchd working in the workspace (a
    background command's shell that outlived its agent).

  A process whose session leads to a living program outside the daemon (a terminal
  tab's `login`, an editor, an app launched by launchd, which works in `/`) is
  external. Scripts start in their own session (`setsid`) so that what they
  detach is told apart from the daemon's session. Like archiving, which ends every
  process working in a workspace, this counts a job the user detached from a
  terminal since closed, or a tmux server started in the workspace, as the
  workspace's.

Any other process touching the workspace is an **external writer**. A process whose
descriptors cannot be read counts as one too. Descriptors whose path the system
withholds (EPERM on its data vaults under `/var/folders/…/0/`) or that were revoked
or unlinked are skipped: none can be in a workspace folder.

Outside this view, as stated in the module: other users' processes and, on Linux,
those that changed credentials (`(sd-pam)`, `ssh-agent`: the kernel marks them not
dumpable and hides their folder and descriptors as for another user), writes through
memory mappings without a descriptor, a macOS process of the workspace that neither
works nor writes in it, a Linux one started with the marker removed. On macOS a
process another workspace's agent started that works in this one descends from the
daemon too and is ended with it. The sidecar runs in the daemon's home
folder, never in the folder the daemon was started from, so it is in no workspace.
Containers are not processes of this table; their mounts are a separate gate.

## Stopping

1. The sweep reserves the preparation barrier after the settled registered stop and
   scans once. An external writer, or a scan that fails, refuses here with the
   names: `Programs Chauffeur did not start use this workspace: Code Helper (pid
   4242) works in …; … Close them, then retry the transfer`. Nothing was signalled;
   the attempt settles and can be retried.
2. Otherwise the plan (fence, roots) is stored durably in `workspace_process_sweeps`
   before the first signal.
3. The workspace's processes are frozen with SIGSTOP, scanning again until no new
   one appears (at most 16 rounds), so a supervisor cannot restart what is being
   stopped. Every frozen process then gets SIGTERM and SIGCONT, whatever happened
   while freezing: nothing stays frozen.
4. After the grace (5 s), every remaining process of the workspace gets SIGKILL,
   again in rounds; the kernel has 2 s to end each.
5. A last scan must find no process of the workspace and no external writer. The
   receipt (`terminated`, `killed`, each with pid, start stamp and name) is stored
   with the plan.

Before every signal the process's identity is checked again: same pid, same start
time, same user, not a zombie. A process that changed is never signalled. The
window between that check and `kill` remains: macOS has no descriptor to signal a
process exactly. Names are the kernel's short command names; arguments and
environments are never recorded.

Every return settles the attempt, failures to store the plan or the receipt
included: all signals are sent synchronously inside the host task, so none can land
after it, and a retry is safe. A process still alive after the grace is killed by
identity, even if it left the workspace meanwhile. A dropped observer
does not cancel the task; a panic leaves the attempt pending, as for the other
preparation steps. A sweep, failed or not, leaves the execution fence installed.

`stop_registered_and_services` now runs the sweep between the registered stop and
the selected Docker services, so a detached process cannot start a container again
behind the service check. There the sweep does not refuse on other programs: a
container engine's file server (OrbStack, Docker Desktop) holds bind-mounted files
open for writing until its container stops. Once the services stop, a last scan
refuses on any program still working or writing in the workspace; the services stay
stopped and the fence installed.

## Part of the complete source stop

A standalone sweep's receipt says that, at its last scan, no process of the
workspace ran and no other process of the user worked or wrote in it. The
[complete source stop](workspace-source-stop-receipt.md) runs it after the registered
stop with that external check deferred (container file servers write until their
services stop), and makes the check itself with a last scan once services and
containers are checked.

## Validation

Unit tests spawn real processes: descending from the daemon (macOS) or with the
marker (Linux) they are the workspace's; under another daemon, a process of a
living session working in the folder or holding a file open for writing is
external, a reader is not; on macOS an orphan of a dead session and an orphan
leading its own session in the workspace are the workspace's, one whose living
leader sits in `/` is not; a changed identity is never signalled. A lifecycle test
on a disposable daemon detaches a process that leaves the workspace on SIGTERM and
runs on, one that keeps forking, and one orphaned from a dead session as an agent's
background command is, runs the registered stop, then: an external program (led by
a living session outside the daemon) makes the sweep refuse without signalling or
freezing anything; once it is gone the sweep kills the first, terminates the
others and their children, leaves another workspace's process running, keeps the
fence and stores its receipt; a second sweep finds nothing. The combined stop stops
its services before refusing on a program still writing in the workspace. Both run
on macOS and in the Arch Linux machine.
