# An exploration's agent reads the network, the same for every agent

- **Date:** 2026-11-19
- **Status:** accepted

## Context

A pull request review asked in an exploration could not read the pull request: Codex's read-only sandbox has no network at all, while Claude's exploration sandbox allowed GitHub and a Vibe exploration read it over approved commands. One mode gave three different answers to "can I call the API?", and a review (PR 2297 of dialog-python-api, 2026-11-19) lost hours to the difference.

## Decision

An exploration is read-only: it never writes. Reading includes the network: GitHub's domains (`github.com`, `*.github.com`, `*.githubusercontent.com`), and the GitLab's host on an installation set to GitLab, are reachable from an exploration's commands, whatever the agent — Claude Code, Codex, Mistral Vibe (asked by a user, decided with Justin 2026-11-19).

## Consequences

- One shared domain list (`sidecar/src/guard/read-only-guard.ts`), `readOnlyAllowedDomains`: Claude's read-only sandbox allows exactly it; Codex's thread proxy carries it (`features.network_proxy`'s domains riding the thread as the daemon's socket does: the thread begins with the network on, its first turn sets the read-only sandbox, the proxy stays for the thread's life); a Vibe exploration reads it over the approvals its commands ask.
- The git and GitHub write guard stays the enforcement for writes on GitHub, for all three agents: neither a sandbox nor the proxy can tell an HTTP read from a write.
- Vibe's exploration has no OS sandbox of Chauffeur's: its guards refuse git and GitHub writes, and a network read is a command the user approves. When ACP offers Chauffeur a sandbox for Vibe, the same list applies to it.
- Nothing else changes: no `chauffeur` CLI, no daemon socket, no writable roots, plans as before.
