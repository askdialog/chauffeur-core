# 0033. GCP workers: creation, on-demand lifecycle, sign-in and remote editor

- **Date:** 2026-10-26 (also 2026-10-27, 2026-10-29)
- **Status:** accepted

## Context

A transfer needs a machine to go to; the Mac's daemon creates one in GCP with the user's own `gcloud`, and its agents need logins without copying the Mac's.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, Cloud workspace follow-up:

> GCP workers (decided with Justin 2026-10-26, [GCP workers](../cloud-workers.md)): the Mac's daemon
> creates a worker with the user's own `gcloud` (project and zone in `[workers]`: no project by default,
> zone `europe-west9-a`), installs Chauffeur on it and keeps its daemon's socket
> forwarded over SSH, so that a ready worker is a transfer machine by its name. Chauffeur is
> installed from a Linux bundle (chauffeur, chauffeurd, sidecar) the release publishes beside the
> disk image, the one of the daemon's own version; an unreleased build names its own. Claude Code
> and Codex are installed too, signed in from the Mac (below).
> The worker keeps its protected keys in owner-only files of its state folder, a headless VM having
> no Secret Service (a vTPM-sealed keyring is later work). Three machine types (e2-standard-2/4/8, 4 by default), with an estimated hourly cost from a dated
> table of Paris prices, shown before anything is created. Only VMs labelled `chauffeur-worker`
> are deleted, on request, never while a workspace runs there; the project's other services
> (relay, Office) are never touched, and no firewall, IAM or project metadata changes. Daemon,
> CLI (`chauffeur worker`) and app (Settings → Transfers) together.
>
> On-demand workers (Justin, 2026-10-27): sending a local workspace to Cloud reuses a worker
> created automatically by this daemon, or creates one with the default machine type and the
> user's gcloud. The app and CLI show its estimated cost before sending. Concurrent sends share
> one creation; the existing worker limit still requires explicit confirmation. A successful return
> of its last workspace to this Mac, including deletion of the remote workspace copy, stops the
> VM with its disk (Justin, 2026-10-29: deleting it lost the agents' logins at every round trip):
> stopped, only the disk is billed (about $5.80 a month for 50 GB), its forward stops, and its SSH
> entry, account and backup access stay. The next send to Cloud starts it again and installs this
> Mac's Chauffeur on it whatever version its disk kept, its agents still logged in; a cancelled
> preparation leaves it stopped. Deleting it, VM, disks, SSH entry and backup access, is the
> user's explicit action (Settings → Transfers, `chauffeur worker delete`). A worker never
> installed (its preparation failed and was cancelled) is still deleted. Workers
> created manually stay manual. Durable reservations protect failed/interrupted transfers and
> survive daemon restarts; cancellation releases a preparation only once no handoff can remain.
> A failed stop or deletion is retried. Backup recovery's deliberately retained disks are never removed
> by this cleanup. An idle workspace still on the worker keeps it alive: there is no idle timeout.
>
> Signing a worker's agents in (decided with Justin 2026-10-29, [signing its agents
> in](../cloud-workers.md#signing-its-agents-in)): it always happens from the Mac, never on the VM:
> the user opens no terminal or SSH there. The worker keeps its own login, one per machine as
> above: the login runs on the worker's CLI, through its forwarded socket, with a flow whose page
> needs no callback to the worker (Claude Code's manual page and the code it shows, pasted back in
> the app or the CLI; Codex's device code, typed on OpenAI's page), and the user only approves it
> in the Mac's browser and app. The credentials go straight to the worker's CLI: only Claude Code's
> single-use authorization code, which the worker's CLI alone can exchange, passes through the Mac,
> never kept or logged. The Mac's Keychain or `~/.codex/auth.json` are never copied (refreshing
> one machine's token would log the other out), and no API key or Vertex is ever used, not even as
> an option (per-token billing refused). A transfer to a worker whose agent is signed out fails
> before anything stops with a distinct state (`needs_login`: the worker and its agents); the
> transfer's header, the batch view and Settings → Transfers offer "Sign in to Claude Code on
> <worker>" / Codex, `chauffeur worker login <name>` does it from a terminal, and the Mac's daemon
> resumes the transfer once every agent it needs is signed in. Workers stopped rather than deleted
> keep their login: signing in is needed for a first transfer and when a login expired.
>
> A workspace moved to a GCP worker opens in this machine's Cursor or VS Code over SSH (decided with
> Justin 2026-10-27): Chauffeur writes one SSH entry per worker, only in a file it manages
> (`ssh_config` under its state folder), included once from `~/.ssh/config` by one `Include` line it
> adds at the first worker, saying so; a worker's entry goes with it, and nothing else in `~/.ssh`
> is touched. The Open button comes back for such a workspace and runs `cursor` or `code --remote
> ssh-remote+<host> <folder>`; for a machine declared by hand it stays hidden.

## Consequences

Only VMs labelled `chauffeur-worker` are touched. An on-demand worker is stopped, not deleted, after its last return (2026-10-29), so that its agents' logins survive; each worker keeps its own login, signed in from the Mac, never an API key or a copy of the Mac's credentials. Details: [`docs/cloud-workers.md`](../cloud-workers.md).
