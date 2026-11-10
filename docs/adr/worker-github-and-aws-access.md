# A worker gets the Mac's GitHub and AWS SSO access on each send

- **Date:** 2026-10-31
- **Status:** accepted

## Context

A workspace moved to a GCP worker could neither open pull requests nor reach AWS there: the worker's `gh` was signed out and it had no AWS CLI configuration, so the user had to run `gh auth login` and `aws sso login` on the VM. The first real on-demand trial (2026-10-31) also found `git fetch` failing there: the worktrees' `origin` keeps the Mac's SSH URL and the worker has no SSH key. [0033](0033-gcp-workers.md) had ruled out any copy of the Mac's credentials, a rule written for the agents' logins, whose refreshed tokens log the other machine out. Justin asked that a worker need no new GitHub or AWS sign-in.

## Decision

Decided with Justin 2026-10-31: on each transfer to a GCP worker, before anything stops, the Mac's daemon copies to it the token of each host the Mac's `gh` is signed in to, and the AWS CLI's `~/.aws/config` with its SSO token cache (`~/.aws/sso/cache/*.json`). This amends 0033 for GitHub and AWS only: the agents' logins stay one per worker, signed in from the Mac.

## Consequences

- The worker's daemon receives them through a private peer method (`worker.credentials_install`) and writes them itself: `gh auth login --with-token` then `gh auth setup-git`, `git` set globally on the worker to fetch and push GitHub's SSH URLs over HTTPS (`url.https://<host>/.insteadOf`), the AWS files owner-only (`0600`, folders `0700`) over the previous copies of the same name. The Mac's own `git` and AWS configuration are never touched.
- The secrets travel only over the worker's forwarded private socket; they are never logged, nor put in a result, an event or a transfer's log. They stay on the VM while it exists: deleting the worker deletes them.
- `~/.aws/credentials` (static keys) is not copied. The SSO session copied expires on the worker when it expires on the Mac; the next send copies the current one.
- What cannot be read or written is a warning in the transfer's log, never a refusal. A Mac whose `gh` is signed out, or with no AWS configuration, gives nothing; the worker's check still says when its `gh` is signed out.
- The worker installation adds the AWS CLI.
- Machines declared by hand in Settings → Transfer and the way back get nothing: only workers this daemon created.
