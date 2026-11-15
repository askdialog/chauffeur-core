# The phone creates a workspace on the Mac

- **Date:** 2026-11-15
- **Status:** accepted

## Context

The phone could follow and drive the workspaces of its Mac, never create one: ADR 0023 kept
workspace mutations out of the mobile host's allowlist. Away from the Mac (shopping, say), the
user wants to start work there: pick repos, give a first prompt, and let an agent begin. Justin
validated the principle on 2026-11-14 and asked for the screens designed in the workspace
`create-workspace-from-mobile-app` on 2026-11-15.

## Decision

An approved phone creates a workspace on its Mac, which the Mac's daemon creates as the app
does (`workspace.create`). The phone chooses repos among those already cloned on the Mac (the
Mac's defaults ticked first), optionally Linear tickets found through the Mac (`linear.search`),
a first prompt, which is required, the first chat's agent, model, effort and fast mode, and in
advanced options a name and a branch. Creation runs in the background; the phone follows its
steps (fetching the repos, the worktrees, the setup scripts, the first chat) and opens the first
chat once it exists. Closing the screen leaves the creation running on the Mac, shown as
creating in the workspace list. A failed creation keeps the phone's choices for a retry.

The mobile host forwards only that: `workspace.create` with `background` and the fields above,
at least one repo, and every repo cloned on the Mac (checked against the Mac's list before the
call), never a share, a parent, an exploration, a design, images or a permission mode;
`repos.cached` without refreshing from GitHub, its clones' paths removed; `linear.search` for
issues only. Of a workspace's events, a phone's one-workspace subscription receives its
transfer, state, progress, warnings and added worktrees, and the output of its setup scripts,
never its terminals or its run and archive scripts. ADR 0023's other exclusions (terminals,
files, settings, archive and other workspace mutations) stand.

## Consequences

- A Mac that is asleep or offline cannot create anything: the phone says so before the form is
  sent and keeps what was typed in memory. A Mac whose mobile host predates this refuses the
  methods; the phone asks to update Chauffeur on the Mac.
- `workspace.create` is not idempotent: the phone never retries it on its own after a lost
  acknowledgement; the workspace, if it was created, shows in the list.
- Cloning a repo, a milestone, a parent workspace or a design chat from the phone stay for later.
