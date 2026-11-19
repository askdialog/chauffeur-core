# Pilot GitHub's merge queue on Core

- **Date:** 2026-11-19
- **Status:** accepted

## Context

Core requires an up-to-date branch before merging. Concurrent merges can force authors to
update their branch, rerun checks and collect approval again.

## Decision

Pilot GitHub's merge queue on public Core only. A human still approves and queues each
PR; GitHub validates the candidate on top of `main` and squash-merges it if green. The
required `check` covers the candidate's complete diff from its queue base using Core's
existing Linux job selection. The queue does not run the Mac, Debian or headless
packaging workflows. Wait for the applicable packaging checks before enqueuing a packaging PR.

## Consequences

- Merge the CI support before enabling the queue. In the Core `main` ruleset, add the queue
  and disable the separate up-to-date requirement in the same edit. Keep the required
  `check` and existing review rules. Start with squash, all queue entries green, two
  concurrent candidates and no minimum batch wait.
- Watch a documentation PR and a code PR: the queued candidate must report `check`,
  select the expected jobs and not start packaging. Compare branch updates, time from
  green approval to merge and runner minutes per merged PR after one week.
- To roll back, remove the queue and restore the up-to-date requirement in the same
  ruleset edit. Dialog's rules, package triggers and release approvals never change.
