# A transfer carries the workspace folder's files outside its repos

- **Date:** 2026-11-03
- **Status:** accepted

## Context

A transfer refused any file of the workspace folder in no repo and outside `.chauffeur/`, so that nothing was left behind silently. Agents start in the workspace folder, and the tools they run write there: Playwright MCP puts its snapshots and console logs in `.playwright-mcp/` of its working folder. A send to Cloud was refused for one of those files ("`.playwright-mcp/page-….yml` in the workspace folder belongs to no repo"), after a GCP worker had been prepared for it. Justin asked that transfers be reliable and fast.

## Decision

Decided with Justin 2026-11-03: a transfer carries the workspace folder's files outside its repos and notes, and restores them at the same path. It refuses only what a package cannot carry: a link, a special file, a Git repository the workspace does not have (a stale worktree under `.worktrees/`, a clone made by hand), a name Git snapshots cannot carry, or more than 20,000 such files or 1 GiB. The folder is checked before a GCP worker is prepared, so that a refused send fails at once.

## Consequences

- The package's manifest lists them in `loose`, packaged under `loose/` and checked like the notes; a backup carries them the same way and leaves out, named, what a transfer would refuse.
- `node_modules/` and `.venv/` folders outside the repos do not travel, as a repo's ignored files do not: the destination installs them again.
- `CLAUDE.md`, `AGENTS.md`, `.claude/` and `.mcp.json` at the root are still not carried: the destination writes them again.
- Bringing a workspace back compares these files too: one changed or added in the retired copy since it left refuses the return unless the local copy is kept.
- Archiving still keeps only the notes folder.
