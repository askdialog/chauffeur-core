# Documents

Every document of `docs/`, by subject, with its status on 2026-10-29:

- **decided**: approved with Justin, and said so in its text (or, where noted, in `SPEC.md` or an ADR that points to it).
- **draft**: built or proposed, with no decision of its own on record. Saying it follows the approved cloud scope does not count. When in doubt, a document is a draft.
- **superseded**: said to be replaced by another document, or describing a stack the shipped code does not use.

Many drafts describe code that runs today; the notes say so, and where their text is out of date.

## Product spec

| Document | Status | What it is |
|---|---|---|
| [SPEC.md](SPEC.md) | decided | The reference for how Chauffeur behaves: user stories and implementation decisions, settled with Justin from 2026-10-13 |
| [PLAN.md](PLAN.md) | draft | The build stages and their status, the repository's layout and conventions |
| Database (in the Database extension's repository) | draft | The integrated PostgreSQL client's behavior, limits, CLI and validation; approved scope in ADR 0042 |
| Office (in the Office extension's repository) | draft | The Office, a 3D village of the team's agents: shipped, approval by Justin not recorded; what shipped without a recorded decision is listed for him |

## Decisions

| Document | Status | What it is |
|---|---|---|
| [adr/README.md](adr/README.md) | decided | Index of the architecture decision records and how to write a new one |

## Architecture

| Document | Status | What it is |
|---|---|---|
| [source-repositories.md](source-repositories.md) | decided | Core as the source; a distribution packages it with official extensions and its own defaults |
| [extensions.md](extensions.md) | decided | Core, official bundled extensions, marketplace activation and Core-only builds; see the official-bundled-extensions ADR |
| [platforms.md](platforms.md) | draft | What differs between macOS and Linux in the code, Linux's known limits, why Windows is not supported |
| [workspace-relay.md](workspace-relay.md) | decided | Piloting a moved workspace from the Mac: calls, events, ports, idle reminders, previews on the phone |
| [workspace-development-services.md](workspace-development-services.md) | decided | Declaring a workspace's Docker services as data or cache, and starting them on arrival. Lines 18–19 and 44 still call the restart "the next slice" |
| [workspace-development-profiles.md](workspace-development-profiles.md) | draft | The daemon's versioned development profile of a workspace. Lines 44–48 list as future work what exists |
| [workspace-execution-fence.md](workspace-execution-fence.md) | draft | The per-workspace fence that blocks new local work during a transfer. Used by the transfer; lines 4–5 say nothing installs it yet |
| [workspace-launch-journal.md](workspace-launch-journal.md) | draft | Durable launch intents of chat sessions and their native root processes. Used by every session start |
| [registered-source-stop.md](registered-source-stop.md) | draft | Stopping a workspace's agents, scripts and terminals under the fence. Used by the transfer; lines 5–6 say no RPC calls it |
| [managed-process-stop-confirmation.md](managed-process-stop-confirmation.md) | draft | Confirmed exits of scripts and terminals. Used by the transfer; lines 4–6 say nothing invokes them |
| [native-session-stop-confirmation.md](native-session-stop-confirmation.md) | draft | The sidecar holding a chat until its agent's process exits. Used by the transfer |
| [workspace-process-sweep.md](workspace-process-sweep.md) | draft | Ending a workspace's leftover processes before a transfer. Used; lines 9–10 say no RPC calls it |
| [workspace-service-stop.md](workspace-service-stop.md) | draft | Stopping a workspace's reconstructible Docker services. Used by the transfer |
| [workspace-postgres-exports.md](workspace-postgres-exports.md) | draft | Logical `pg_dump` exports of a workspace's declared databases. Used by the transfer |
| [workspace-postgres-quiescence.md](workspace-postgres-quiescence.md) | draft | Proving no write happened after an export. Used; line 15 says no RPC calls it |
| [workspace-postgres-restores.md](workspace-postgres-restores.md) | draft | Restoring an export on the destination under quarantine. Used by arrival |
| [workspace-source-stop-receipt.md](workspace-source-stop-receipt.md) | draft | The complete source stop and its receipt. Used; line 10 says no RPC calls it |
| [claude-context-bundles.md](claude-context-bundles.md) | draft | Capturing and restoring a Claude session's native files. Used by transfers; line 7 says it has no production caller |
| [codex-context-bundles.md](codex-context-bundles.md) | draft | Capturing a Codex rollout and its history. Used by transfers; line 5 says it has no production caller |

## Cloud

The architecture of cloud workspaces and the same table with more detail: [cloud/README.md](cloud/README.md). The `cloud-*.md` files stay where they are.

| Document | Status | What it is |
|---|---|---|
| [cloud/README.md](cloud/README.md) | draft | Architecture of cloud workspaces as built: actors, flows, the documents |
| [cloud-workspaces.md](cloud-workspaces.md) | decided | The approved scope (settled with Justin 2026-10-19). Outdated in parts: the controller on the relay VM and the credential vault, removed on 2026-10-31 ([ADR](adr/credential-vault-removed.md)), deleting workers and backups after a return |
| [cloud-workers.md](cloud-workers.md) | decided | GCP workers: creating, signing in from the Mac, reaching, opening in an editor, stopping, deleting |
| [cloud-workspace-transfer.md](cloud-workspace-transfer.md) | decided | Driving a transfer: machines, the wait for turns, batches, resume and cancel, the app, the phone |
| [cloud-workspace-return.md](cloud-workspace-return.md) | decided | A workspace coming back, the copy it left, edits checked |
| [cloud-workspace-backups.md](cloud-workspace-backups.md) | decided | Encrypted backups from a worker; manual recovery on the Mac |
| [cloud-workspace-handoff-journal.md](cloud-workspace-handoff-journal.md) | draft | The controller's journal of a transfer. Used; lines 4–5 say no RPC invokes it |
| [cloud-workspace-snapshots.md](cloud-workspace-snapshots.md) | draft | A worktree's Git snapshot and its verified restore. Used by the package |
| [cloud-workspace-package.md](cloud-workspace-package.md) | draft | The transfer package. Used; line 7 says no action calls it |
| [cloud-workspace-preparation.md](cloud-workspace-preparation.md) | draft | Capture, seal and staging on the source. Used |
| [cloud-workspace-archives.md](cloud-workspace-archives.md) | draft | The encrypted archive format. Used; its key delivery (lines 16–21) is not what ships |
| [cloud-workspace-artifacts.md](cloud-workspace-artifacts.md) | draft | The daemon's store of staged and received ciphertext. Used |
| [cloud-workspace-bridge.md](cloud-workspace-bridge.md) | draft | The handle RPCs the driver copies blocks through. Their Noise half is unused |
| [cloud-workspace-restoration.md](cloud-workspace-restoration.md) | draft | Quarantined restore on the destination. Used; its one-login-per-machine rule (line 54) was settled with Justin |
| [cloud-workspace-activation.md](cloud-workspace-activation.md) | draft | Activation after the commit, the source retired. Used; lines 28–30 predate signing a worker in from the Mac |
| [cloud-workspace-configuration.md](cloud-workspace-configuration.md) | superseded | The archive helper's protected configuration, published only by tests |
| [cloud-workspace-delivery.md](cloud-workspace-delivery.md) | superseded | Archive delivery over Noise; the driver copies over the daemons' sockets |
| [cloud-workspace-dispatch.md](cloud-workspace-dispatch.md) | superseded | `cloud.archive.start` and the admission service, which nothing calls |
| [cloud-workspace-helper.md](cloud-workspace-helper.md) | superseded | Supervising the archive helper, never launched |
| [cloud-workspace-jobs.md](cloud-workspace-jobs.md) | superseded | The archive helper's durable jobs |
| [cloud-workspace-runtime.md](cloud-workspace-runtime.md) | superseded | The archive helper executable and its GCS storage |
| [cloud-workspace-approval.md](cloud-workspace-approval.md) | superseded | Controller admission of one ciphertext, published only by tests |

## Mobile

| Document | Status | What it is |
|---|---|---|
| [mobile.md](mobile.md) | decided | The phone companion: how it connects, the decisions behind it (ADRs 0023, 0028), its delivery notes. The app itself lives in its own repository |

## Research

| Document | Status | What it is |
|---|---|---|
| [prior-art.md](prior-art.md) | draft | Comparable tools looked at before the V1 spec, and what they showed about driving agents across repositories |
| [session-portability.md](session-portability.md) | draft | The Mac → Linux → Mac proof that native Claude and Codex sessions resume elsewhere |
