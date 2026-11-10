# Cloud workspace handoff — approved scope

Settled with Justin on 2026-10-19. Delivered through reviewable PRs, without a Linear project. This describes the follow-up scope, not functionality already available. The first slice is the [native session portability proof](session-portability.md).

On 2026-10-31 the credential vault and the controller on the relay VM were removed, never used by the shipped transfers ([ADR](adr/credential-vault-removed.md)): workers keep their own logins and transfers run a local controller. The bullets about them below are kept as approved, struck by that decision.

## Transfer and return

- Manual only, from desktop or mobile, for one workspace or a selected batch. Never fail over because a connection disappears.
- Prepare a worker while agents run; perform final transfer at the end of their turns. Offer explicit Interrupt and transfer. Coordinate service writes and require source-stop confirmation before destination execution.
- Each workspace has independent progress/outcome. One failure does not roll back successful handoffs. Preserve recoverable data and offer retry.
- Closing the phone app or losing its network does not cancel host-to-host transfer. A Mac must be awake and reachable to send or receive. Source connectivity loss before final validation cannot authorize destination execution.
- Cancel before ownership commits, preserving a usable source and cleaning temporary data. After destination ownership commits, offer a reverse handoff. Reconcile uncertain ownership before running either side.
- Show execution location on both clients and route Chauffeur actions to the active host. Repatriation detects external Mac edits, preserves both versions and requires explicit conflict resolution before overwrite.
- Preserve native Claude/Codex session state. If a conversation cannot resume, block it and explain; a new conversation with a summary requires a separate explicit user action.

## Workers and controller

- Compute Engine workers in GCP Paris (`europe-west9`), separate from the relay. The controller is a separate service on the existing relay VM. No Cloud Run or Kubernetes for V1.
- Several workspaces can share a worker. All agents, repositories and services of one workspace stay on one worker. No automatic live migration or cross-worker agent distribution.
- Up to two workers can be created automatically; request confirmation above that. This is not a monetary cap. Show readiness, capacity and estimated hourly cost before transfer confirmation.
- Agents wait for workspace capacity instead of automatically upgrading its worker. Display Waiting for resources / En attente de ressources, and explain when persistent-service saturation prevents progress. No arbitrary fixed agent-count limit or promise of unlimited physical capacity.

## Development environment and accounts

- Store each development profile only in Chauffeur; add no configuration file to repositories. Declare Linux startup/setup, Docker services, persistent development data, reconstructible caches and web preview ports. The daemon owns all state and filesystem writes.
- Transfer required files, local Git changes, native context and declared data. Rebuild dependencies/caches and restart services. Use coordinated consistent database exports/restores, not arbitrary copies of active volumes. Classify Redis as cache versus meaningful state/tasks. Verify access to remote databases without duplicating them.
- Required unsupported services or tools block readiness with an explanation; optional tool failures require acceptance of an explicit warning. Do not depend on an offline Mac for local-only MCPs. macOS process memory, native apps and simulators are not portable Linux environments.
- Cloud connections retain native Claude/Codex and other authorized sessions beyond worker deletion. Select secrets explicitly and preflight required access; reauthenticate only when expiration/revocation/provider policy requires it. Never silently switch to separately billed API authentication.
- Chauffeur manages a persistent encrypted credential vault, backed by GCP object storage and separate from transfer archives. Only authorized endpoints receive necessary credentials/keys. No export of whole personal CLI homes, Keychain, SSH directories or shell environments. Account disconnection withdraws cloud access; provider-side revocation depends on provider capabilities.
- Preserve E2E confidentiality from relay transport and storage. A worker is a trusted endpoint and accesses plaintext code, context and required credentials while executing. Worker enrollment, key distribution/rotation and credential renewal need implementation proofs; GCP encryption at rest alone does not satisfy this property.

## Recovery, notifications and cleanup

- Encrypted backups in a Paris GCS Standard bucket, targeting every five minutes. Retain the three latest successful backups; an incomplete backup never replaces a successful one. Show last successful time and stale/failed backup status. The schedule is not a guaranteed recovery point when a backup fails or is slow.
- Never delete the last recoverable copy during cloud execution or unresolved transfers. On worker failure, notify with backup time and potential lost work, then offer manual restoration. Confirm the old worker cannot execute before any restored agent runs. A missing heartbeat alone is insufficient fencing.
- After one hour without an agent turn/one-off command or user interaction, send a grouped reminder with Remind me in one hour. An idle dev server does not suppress it. Ignoring it never automatically stops, deletes or repatriates work.
- Remove transfer archives after durable verified restoration. After verified return, delete the cloud workspace and its backups; delete a worker and its disks when no workspaces depend on it. Keep the relay/controller VM. Avoid retained empty worker disks/caches by default.
- Track cleanup separately: return success permits immediate local work even if deletion fails. Show On Mac · cloud cleanup pending / Sur le Mac · nettoyage cloud en attente, retry automatically and notify persistent failures. Identify remaining billable resources. Configure object soft-delete/version retention deliberately rather than promising immediate billing cessation.

## Desktop/mobile interface

Use Chauffeur's restrained visual language and existing components, French and English copy, compact mobile layouts and keyboard-accessible desktop actions. Lists show execution location, activity and attention states, with explicit multi-selection. A confirmation view prioritizes selected workspaces, readiness, capacity and hourly estimate, then the primary transfer action.

Show preparation, waiting for turns, transfer, restoration, success, recoverable errors, cancellation and partial batch outcomes per workspace. Distinguish unavailable Mac and pending capacity from generic loading. Configuration errors offer a repair action; never turn failed loading into an empty result.

Private previews list only declared web services and their starting/ready/stopped/error states. Open them in a mobile integrated web view through an authenticated E2E tunnel to the worker, only for authorized devices. No public share links or exposed database ports. Close the cloud preview when its workspace leaves the worker. Validate cookies, redirects, login flows and live-update connections per supported project.

## Validation before real work

First prove an isolated disposable Mac→Linux→Mac round trip: both native agent sessions, retained account connections, Git/local files, development data, previews, network interruptions, source/destination ownership fencing, backup recovery, local conflicts and verified cleanup. Then exercise batch partial failures and multiple concurrent workspaces. Never use real user conversations as test fixtures. Keep bounded diagnostics with operation identifiers and no leaked secrets or conversation contents.

This scope does not add iOS push/store publication, public previews, automatic failover, Kubernetes, live process migration or a general-purpose clone of the Mac. Existing Android pairing and push remain foundations. Model consumption and existing relay costs are separate from worker estimates.
