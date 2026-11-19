# A fork's clone reads its base and its PRs from the parent

- **Date:** 2026-11-19
- **Status:** proposed, awaiting Justin

## Context

A user added their fork of a repo (`owner/name` outside the main organization). `gh repo clone` of a fork adds the parent as `upstream` and marks it `gh-resolved = base`, so `gh` reads and opens the PRs there while git fetches and pushes `origin`, the fork. Chauffeur took `origin` for both: workspaces were cut from the fork's `main`, 45 commits behind the parent's, the header counted ↑47 for 2 commits and the Diff tab showed the parent's commits as the branch's, and the PR the agent opened from the fork to the parent showed "no PR", since a fork's PR was never a branch's (story 28), so neither Merge nor archive saw it.

## Decision

Asked by a user 2026-11-19, refined in the review of PR #87.

- A repo's base branch is read from the remote `gh` takes the repo's PRs from, the one `remote.<name>.gh-resolved` marks `base` (a fork's parent after `gh repo clone`, or the remote `gh repo set-default` picked), else `origin`: workspaces and explorations are cut from it after fetching it, and the header's ahead count, the Diff tab and a routine's refresh count from it. For such a remote other than `origin`, the default branch is that remote's (as last fetched, else as it says), unless the settings name one. Branches are still pushed where git pushes them; a PR's branch being resumed is fetched from `origin`.
- A worktree's PR is one whose head is where the branch is pushed, as git pushes it (`branch.<name>.pushRemote`, else `remote.pushDefault`, else the remote it tracks, else `origin`; at its push URL, else its URL, as configured): a PR from the repo itself only when the branch is pushed to that repo, a fork's PR only when the branch is pushed to that fork. So a fork's clone shows the PR it opened to the parent, never the parent's own PR from a branch of the same name, and nobody takes another fork's. The rest of story 28 stands (the pushed-under name, its checks, when in doubt none).
- The sidebar reads a workspace again when any open PR on one of its branches changes, a fork's included; the full read decides whether it is the branch's.

## Consequences

- A few `git config` reads per PR lookup (the push remote, its URL, the base remote); one `ls-remote` when a fork's clone has never fetched its parent's HEAD.
- A clone without a remote marked `gh-resolved = base` reads its base as before; its PRs are its own as before unless its branches are pushed elsewhere than `origin`.
- Another fork's PR on a branch of the same name may make the sidebar read a workspace again for nothing.
- The app still labels an exploration's checkout `origin/<branch>`.
