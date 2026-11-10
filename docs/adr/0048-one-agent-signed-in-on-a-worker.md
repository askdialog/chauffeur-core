# 0048. One agent signed in on a worker is enough for a transfer

- **Date:** 2026-10-30
- **Status:** accepted

## Context

A transfer of a workspace to a new on-demand worker failed with the daemon's raw error: "cloud-<id> cannot take <workspace> yet; nothing was stopped: Codex is not signed in on cloud-<id>: …". A new worker has no agent signed in. The refusal required every agent that the workspace's chats use, and the app showed it as an error instead of leading to where the agents sign in. Justin found this abnormal: the transfer should send the user to the settings, where at least Claude Code or Codex gets signed in.

## Decision

**One agent signed in on a worker (decided with Justin 2026-10-30; see docs/adr/0048-one-agent-signed-in-on-a-worker.md).** A transfer to a GCP worker needs at least one agent signed in there, Claude Code or Codex, whichever the workspace's chats use. With none signed in, the transfer is refused before anything stops (`needs_login` names the worker and both agents). The app does not show the daemon's error: the notification, the workspace's header and the batch view say to sign Claude Code or Codex in on the worker, and offer "Open settings" (Settings → Transfers, where each worker's agents sign in) beside a sign-in per agent. The transfer resumes by itself once one agent is signed in there. With one signed in and the other not, the transfer goes on, and its log says that the other agent's chats wait for a sign-in there.

## Consequences

This replaces [0033](0033-gcp-workers.md)'s "resumes the transfer once every agent it needs is signed in". A chat whose agent is signed out on the worker cannot run there until that agent is signed in, from Settings → Transfers. Declared machines, whose agents sign in from a terminal there, keep the check for each agent of the workspace's chats.
