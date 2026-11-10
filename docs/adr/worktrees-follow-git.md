# The daemon follows git for a workspace's worktrees

- **Date:** 2026-11-03
- **Status:** accepted

## Context

A worktree is recorded with the branch it was created on, and its PR is looked up under that name. A branch cut afterwards in the worktree (`git switch -c`) is therefore not followed: the app kept showing the PR of the first branch, and the PR of the second, open on GitHub, nowhere. A worktree made with a plain `git worktree add` is refused to agents (story 19) but not to anyone else, and then nothing tracks it, though archiving removes it. Justin asked that a worktree made outside Chauffeur be taken in anyway.

## Decision

When the daemon reads a workspace's PRs, it first brings the store in step with git: each worktree's branch is the one its HEAD is on (a detached HEAD leaves what the store has), and a worktree that git has inside the workspace's folder and that nobody registered is registered as an agent's, with a warning in the chat, and no ports nor setup script. A worktree outside the workspace's folder is left alone, and so is an exploration. Asked and decided by Justin, 2026-11-03.

## Consequences

An adopted worktree is archived with the workspace, as those made through Chauffeur are: the folder is the criterion, since that is what archiving already removes. The guard against `git worktree add` stays for agents, which are told the command to use; this is for what it cannot see, a terminal or another tool. The pass runs with the PR read, under the lock that a creation holds, and only reads git. Not done: following a branch change or a creation that no PR read comes after (the next read finds it).
