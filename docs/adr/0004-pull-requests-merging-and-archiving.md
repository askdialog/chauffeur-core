# 0004. Pull requests, merging and archiving

- **Date:** 2026-10-14 (also 2026-10-15, 2026-10-16, 2026-10-18)
- **Status:** accepted

## Context

A multi-repo slice ships as linked PRs, one per repo. Archiving must leave nothing behind (processes, worktrees, compose projects) and yet never lose work.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, story 28:

> As a developer, I want to create one PR per repo that has commits, drafted by the agent, each linking the others, so that a multi-repo slice ships as linked PRs. The agent opens them with the user's own PR skill (ready for review, the team's title, description and labels) and drives them with a PR-shepherding skill (CI, AI review; it never merges), when installed. Then I merge them one PR at a time, with the green Merge button beside each PR in the header, shown only while that PR can be merged on its own: open, not a draft, not stacked on another open PR of its repo, checks passed or none, no changes requested nor review required, no conflict, and write access to its repo; otherwise its tooltip says why (2026-10-16; it replaces one header Merge for every PR, decided with Justin 2026-10-15). I pick squash and merge or a merge commit (what its repo allows; my last choice kept). The CLI still merges several, one after the other, APIs first, the first refusal from GitHub stopping the rest. The PRs its agents opened from their own worktrees (`chauffeur worktree add`, one per sub-ticket, say) are the workspace's too: the header shows them, by worktree group, and Merge merges them. A worktree's PR is the newest GitHub has on its branch's name (not a fork's); without one, the one on the name the branch was pushed under (`git push -u origin HEAD:<name>`), only if that name is no shared branch (main, master, staging, develop…, the base, origin's default) nor the branch or upstream of another branch of the clone or of any worktree Chauffeur has known, the PR comes from the repo itself, and its head commit is in the local branch (of several, only a lone open one): when in doubt none is shown, never a PR opened from another workspace (decided with Justin 2026-10-18). A PR stacked on another is merged after it, and first pointed at where that one landed (GitHub retargets it only once the merged branch is deleted, and it would otherwise merge into that branch).

From `SPEC.md`, story 33:

> Removed on 2026-10-14 (Justin): no workspace is archived automatically, not even once its PRs are merged. A merged PR does not end the conversation (a follow-up ticket, a question); only the user archives.

From `SPEC.md`, Archive, in order:

> **Archive, in order** (in the background from the app, without a confirmation since a workspace can be restored (decided with Justin 2026-10-16); a dialog opens only when the daemon refuses, to say why and offer to archive anyway: the workspace leaves the sidebar at once, the one shown giving way to its neighbour; a toast tells how it ended, and a workspace that could not be archived comes back with its error; decided with Justin 2026-10-16 after a user's feedback).
> 1. Kill process groups and their descendants.
> 2. Run each repo's archive script; a failure sets `cleanup_failed`.
> 3. `docker compose down` every compose project whose config files live in the workspace folder. Skipped, Docker not called, when no repo of the workspace tracks a compose file (`compose.yml`, `docker-compose.yml`, their `.yaml` and `compose.*.yml` variants, at any depth; an untracked one is missed). `docker compose ls` gets 10 s, each `down` the archive scripts' timeout; past it the call's process group is killed and archiving goes on, with a warning (decided with Justin 2026-10-18).
> 4. Snapshot each worktree (HEAD, index, working tree with untracked files) in `refs/chauffeur/archive/<workspace>/<repo>`; purge after 30 days.
> 5. Remove the worktrees, children's included.
> 6. Delete the local branches whose PR is merged, found as the header finds it; through the name a branch was pushed under, only when its tip is the merged PR's head (decided with Justin 2026-10-18).
> 7. Strip worktrees, `node_modules` and `.venv` from the notes folder.
>
> Restore recreates the worktrees from the snapshot refs. Only the user archives: nothing is archived automatically (story 33).

## Consequences

Only the user archives; nothing is archived automatically, not even once its PRs are merged. Archiving runs in the background without confirmation because a workspace can be restored for 30 days. A worktree's PR is shown only when Chauffeur is sure it belongs to that branch; when in doubt, none is.
