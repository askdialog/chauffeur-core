# 0030. Transfers of a workspace to another machine and back

- **Date:** 2026-10-23 (also 2026-10-24, 2026-10-26, 2026-10-29)
- **Status:** accepted

## Context

Within the handoff scope ([0027](0027-cloud-workspace-handoff-scope.md)), the first whole transfer, the way back, who drives a transfer and how it waits for the agents had to be settled.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, Cloud workspace follow-up:

> First whole transfer (decided with Justin 2026-10-23 and 2026-10-24): a CLI
> (`chauffeur cloud transfer`) drives the source, a controller and the destination
> daemons over their private sockets, before the app, the phone and GCP do. After the
> commit the source keeps the workspace read-only, marked moved; the way back is later
> work. Each worktree's `origin` comes back on the destination without credentials, and
> pushes use the destination's own. Until the credential vault provisions accounts, the
> destination's agents use that machine's own logins: the user runs `claude login` /
> `codex login` once on it, never an API key chosen silently. So restored conversations
> go into the destination's normal native homes (`~/.claude`, `~/.codex`, or the
> `CLAUDE_CONFIG_DIR` its agents get) as new files, never replacing one. On the way back
> (2026-10-26), as the journal's next transfer after the one that retired a copy, the
> returning version replaces that retired copy: same folder and
> workspace, conversations continued; the copy is removed only once the return is
> restored, and a failure before puts it back. A transfer the app starts is driven by the
> Mac's daemon, in the background (2026-10-26), to a machine declared in Settings → Transfer (a
> name and a daemon socket each, often forwarded over SSH, beside the controller's socket) or to a
> ready GCP worker, which Chauffeur adds itself (below).
> Automatic local controller setup (Justin, 2026-10-29): the current Mac transfer path must
> require no terminal command, controller socket or manual service startup. With no custom
> controller configured, the Mac's daemon starts a private local controller on first use, with a
> separate protected identity and persistent journal under its state folder. It restarts that
> controller after failure or the Mac's restart, and stops it with the daemon without deleting its
> journal. An explicitly configured controller remains authoritative. The app offers Cloud from
> default settings; controller sockets are an advanced override. This automates the existing local
> transfer path; the separate relay-hosted controller in the wider cloud scope remains future work.

From `SPEC.md`, Cloud workspace follow-up:

> A transfer waits for the end of the workspace's agents' turns before stopping anything,
> showing "Waiting for the end of turns", and "Interrupt and transfer" interrupts them first;
> several workspaces picked in the sidebar (⌘-click, ⇧-click, Select in a row's menu, a bar
> under the list) are confirmed together and transferred as one transfer each, at most two
> past the wait at once, followed in the batch's view, which the sidebar reopens; the phone
> shows the wait and offers "Interrupt and transfer", a batch from the phone is later work
> (decided with Justin 2026-10-26, [waiting](../cloud-workspace-transfer.md#waiting-for-the-end-of-turns),
> [a batch](../cloud-workspace-transfer.md#a-batch)).
> The phone follows, starts and brings back transfers, and resumes or cancels a failed one,
> as the desktop app does (2026-10-26): its host admits starting and cancelling one, with
> machine names only (the one workspace mutation it forwards), its status, and a
> subscription to one workspace that passes only its transfer events
> ([on the phone](../cloud-workspace-transfer.md#on-the-phone)).

## Consequences

The Mac's daemon drives transfers in the background and starts a local controller itself; a relay-hosted controller is future work. Details: [`docs/cloud-workspace-transfer.md`](../cloud-workspace-transfer.md), [`docs/cloud-workspace-return.md`](../cloud-workspace-return.md).
