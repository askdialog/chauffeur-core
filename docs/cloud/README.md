# Cloud workspaces

A workspace can leave the Mac for another machine, a GCP worker most often, keep running there while the Mac sleeps, and come back. Everything is on the user's request: Chauffeur never fails over on its own. This page is the architecture as built on 2026-10-29 and the map of the `cloud-*.md` documents. The approved scope is [`cloud-workspaces.md`](../cloud-workspaces.md) (settled with Justin 2026-10-19); the decisions taken since are ADRs [0027](../adr/0027-cloud-workspace-handoff-scope.md), [0030](../adr/0030-transfers.md)–[0035](../adr/0035-worker-backups.md) and [credential-vault-removed](../adr/credential-vault-removed.md).

## Actors

```
                      the user's gcloud (create, start, stop, delete VMs)
                                   │
 Mac ────────────────────────────────┼─────────────────────────────── GCP project of [workers]
 ┌──────────────────────────────┐   │    ┌──────────────────────────────────────┐
 │ app, CLI, phone (via relay)  │   │    │ worker VM chauffeur-worker-<name>     │
 │            │                 │   ▼    │  chauffeurd (systemd user service),   │
 │ chauffeurd (drives transfers)├── SSH ─┤  sidecar, Claude Code, Codex,         │
 │  ├ local controller          │ forward│  the agents' own logins               │
 │  │  (2nd chauffeurd, journal)│ of its │            │                         │
 │  └ backup keys (Keychain)    │ socket │            ▼ every 5 min, encrypted   │
 └──────────────────────────────┘        │  GCS bucket <project>-chauffeur-backups│
                                         └──────────────────────────────────────┘
 relay VM: mobile relay (opaque, E2E) · share service
 Office server: a VM of its own beside it
```

- **The Mac's daemon** drives every transfer in the background (`cloud.transfer.start`), from the app, the CLI or the phone. It talks to the other daemons over their private Unix sockets: a GCP worker's is forwarded over SSH and kept up by the daemon; a machine declared by hand in Settings → Transfer names its own (often an SSH forward too).
- **The controller** keeps the transfer's journal (prepare, stop, snapshot, restore, commit). With default settings it is a second `chauffeurd` the Mac's daemon starts, restarts and stops itself, with its own identity and journal under `transfer-controller/` (ADR [0030](../adr/0030-transfers.md), 2026-10-29). A controller socket in `[transfer]` is an advanced override. A controller hosted on the relay VM was built for the credential vault, never used by a transfer, and removed with it on 2026-10-31; one for transfers would be built anew.
- **A GCP worker** is a Compute Engine VM the Mac's daemon creates with the user's own `gcloud`, labelled `chauffeur-worker`, running Chauffeur's Linux bundle of the Mac's version ([`cloud-workers.md`](../cloud-workers.md)). Its agents keep their own login, signed in from the Mac: no API key, no copy of the agents' credentials. Its `gh` and AWS CLI get the Mac's GitHub token and AWS SSO sign-in on each send ([ADR](../adr/worker-github-and-aws-access.md)), and its agents the Mac's Claude Code configuration when it changed ([ADR](../adr/worker-claude-code-configuration.md)).
- **The relay** (`relay/`) carries the phone's end-to-end encrypted sessions to the Mac's mobile host. Transfers do not go through it; the phone only starts and follows them through the Mac ([`mobile.md`](../mobile.md)).
- **The share service** (`crates/chauffeur-design-share`) hosts design and workspace shares; it is no part of a transfer.

## Flows

**Transfer.** The destination is checked first (sidecar, `git`, free space, its agents signed in), so that a refusal stops nothing. The transfer waits for the end of the agents' turns (or interrupts them on request), then the source stops completely (registered processes, a sweep of what is left, Docker services, PostgreSQL exports with a no-write proof) and records a receipt. It captures the workspace into a package (Git snapshots, notes, chats and their events, native Claude and Codex conversations, database exports), seals it (AES-256-GCM) and stages it. The driving daemon copies the ciphertext to the destination in 256 KiB blocks over the sockets, sixteen ahead of their answers, and hands it the archive key; its log (`chauffeurd.log`) has a timed line for each step. The destination restores it under quarantine, the journal commits, the destination activates it and runs each repo's setup script, and the source keeps a read-only copy marked moved. Steps: [`cloud-workspace-transfer.md`](../cloud-workspace-transfer.md#steps).

**Piloting.** While it runs elsewhere, the Mac's app pilots the workspace as a local one: the daemon relays its calls and events and tunnels its whole port block; an idle reminder comes after an hour ([`workspace-relay.md`](../workspace-relay.md)).

**Return.** A transfer back to the machine it left replaces the retired copy there: same folder, conversations continued. It refuses, naming what changed, if that copy was edited meanwhile (`--keep-local-copy` keeps it beside); once restored, the machine left deletes its own copy ([`cloud-workspace-return.md`](../cloud-workspace-return.md)).

**Backup.** While a workspace runs on a GCP worker, the worker captures it every five minutes without stopping it, seals it with the Mac's per-workspace key and writes it to the backup bucket, keeping the three latest. Manual recovery on the Mac first deletes the worker's VM with its disks kept and proves it gone ([`cloud-workspace-backups.md`](../cloud-workspace-backups.md)).

**On-demand workers.** "On-demand Cloud" (`--to cloud`) reuses a worker this daemon created, or creates one after showing its estimated cost. Since then, the last return of its workspaces **stops** the VM with its disk instead of deleting it, so that its agents' logins survive between round trips (about $5.80 a month for the disk); the next send starts it again and reinstalls the Mac's version. Deleting a worker is always the user's action ([`cloud-workers.md`](../cloud-workers.md#on-demand-lifecycle)).

## What the shipped path does not use

The credential vault (native credentials stored end to end, refresh delegation, worker enrollment and bootstrap delivery, Secret Service custody on the worker), the controller hosted on the relay VM and the Settings → Cloud section were removed on 2026-10-31 ([ADR](../adr/credential-vault-removed.md)): workers keep their own login, signed in from the Mac, and their keys in owner-only files ([`cloud-workers.md`](../cloud-workers.md#the-workers-keys)). What transfers used of it, the daemon's endpoint root and its database, is described in [the handoff journal](../cloud-workspace-handoff-journal.md#the-endpoint-root-and-its-database).

One stack built before the path above is not wired into it:

- **The archive helper**: `chauffeur-cloud-archive`, archive delivery over a pinned Noise channel, its protected configuration, admission and jobs. The `cloud.archive.start` RPCs exist but nothing shipped calls them, and the Linux worker bundle does not carry the helper. The driver copies the ciphertext itself; it holds the archive key while it does.

## The documents

The status is the same as in [the index](../README.md): **decided** (its text records a decision with Justin), **draft** (built or proposed, no decision of its own beyond the approved scope), **superseded** (describes a stack the shipped path does not use).

### The path in use

| Document | Status | What it covers |
|---|---|---|
| [cloud-workspaces.md](../cloud-workspaces.md) | decided | The approved scope (2026-10-19). Parts are outdated: the controller on the relay VM and the credential vault (removed 2026-10-31), deleting workers after a return |
| [cloud-workers.md](../cloud-workers.md) | decided | GCP workers: creating, signing in, reaching, opening in an editor, stopping, deleting |
| [cloud-workspace-transfer.md](../cloud-workspace-transfer.md) | decided | Driving a transfer: machines, the wait for turns, batches, resume and cancel, the app, the phone |
| [cloud-workspace-return.md](../cloud-workspace-return.md) | decided | A workspace coming back: the retired copy replaced, edits checked, the copy left deleted |
| [cloud-workspace-backups.md](../cloud-workspace-backups.md) | decided | Encrypted backups from a worker and manual recovery on the Mac |
| [cloud-workspace-handoff-journal.md](../cloud-workspace-handoff-journal.md) | draft | The controller's journal of a transfer, from prepare to commit |
| [cloud-workspace-snapshots.md](../cloud-workspace-snapshots.md) | draft | A worktree's Git snapshot and its verified restore |
| [cloud-workspace-package.md](../cloud-workspace-package.md) | draft | The transfer package: worktrees, notes, chats, native conversations, database exports |
| [cloud-workspace-preparation.md](../cloud-workspace-preparation.md) | draft | Capture, seal and staging on the source under a preparation barrier |
| [cloud-workspace-archives.md](../cloud-workspace-archives.md) | draft | The AES-256-GCM archive format. Its key-delivery part (an endpoint-only channel) is not what ships |
| [cloud-workspace-artifacts.md](../cloud-workspace-artifacts.md) | draft | The daemon's store of staged and received ciphertext |
| [cloud-workspace-bridge.md](../cloud-workspace-bridge.md) | draft | The `cloud.archive.*` handle RPCs the driver copies blocks through. Its Noise half is unused |
| [cloud-workspace-restoration.md](../cloud-workspace-restoration.md) | draft | Quarantined restore on the destination (its one-login-per-machine rule was settled with Justin) |
| [cloud-workspace-activation.md](../cloud-workspace-activation.md) | draft | Lifting the quarantine after the commit; the source retired, read-only |

### Superseded

| Document | Status | Why |
|---|---|---|
| [cloud-workspace-configuration.md](../cloud-workspace-configuration.md) | superseded | The archive helper's protected configuration, published only by tests |
| [cloud-workspace-delivery.md](../cloud-workspace-delivery.md) | superseded | Archive delivery over Noise; the driver copies over the sockets |
| [cloud-workspace-dispatch.md](../cloud-workspace-dispatch.md) | superseded | `cloud.archive.start` and the admission service, which nothing calls |
| [cloud-workspace-helper.md](../cloud-workspace-helper.md) | superseded | Supervising the helper, which is never launched |
| [cloud-workspace-jobs.md](../cloud-workspace-jobs.md) | superseded | The helper's durable jobs |
| [cloud-workspace-runtime.md](../cloud-workspace-runtime.md) | superseded | The helper executable and its GCS checkpoint storage |
| [cloud-workspace-approval.md](../cloud-workspace-approval.md) | superseded | Controller admission of one ciphertext, published only by tests |

### Building blocks outside `cloud-*.md`

The source stop and the services a transfer carries are in `workspace-*.md`; the native conversations in the context-bundle documents. All are wired into the transfer. Their statuses are in [the index](../README.md#architecture).

## Build log of slices without a document of their own

Moved from `docs/PLAN.md` on 2026-10-29, unchanged, except the credential vault's slices, removed with it on 2026-10-31. The other slices PLAN.md listed are described in the documents they pointed to (`docs/cloud-*.md`, `docs/workspace-*.md`).

Portability follow-up: run text and native-tool scenarios by default. Verify
Read/Write for Claude and Code Mode shell calls for Codex, preserve prior tool
call/result pairs, and carry the explicit fixture file through both boundaries.
This is not yet general filesystem, credential or full-session portability.

Compaction follow-up: seed a source conversation, invoke native manual compaction,
then verify its exact compacted state and subsequent messages across Linux and
Mac resumes. Reject deliberately incomplete compaction snapshots before inference.
The fixture does not evaluate summary quality or Jev hooks. A separate explicitly
opted-in hosted Codex proof uses native ChatGPT external auth in memory, verifies
a real encrypted compaction item and continuation across Mac→Linux→Mac, and
deletes all generated state. It does not establish durable login refresh/revocation
or enable cloud execution.

Process sweep correction: on a daemon running outside an app (LaunchAgent), macOS
no longer attributes a process to the program that started it, so the sweep's
"responsible process" saw every workspace process as another program's and refused.
macOS now attributes a process working in the workspace by descent from the daemon
or by its session (leader gone, descended from the daemon, or an orphan working in
the workspace); a living session outside the daemon is external. Scripts start in
their own session. The daemon no longer relaunches itself at start. See
`workspace-process-sweep.md`.
