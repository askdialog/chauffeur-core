# 0021. The daemon's commands refused from inside a workspace

- **Date:** 2026-10-18
- **Status:** accepted

## Context

An agent cleaning up its throwaway daemon with `chauffeur daemon stop`, `CHAUFFEUR_HOME` forgotten, stopped the user's real daemon and every session on it.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, The daemon's commands from inside:

> **The daemon's commands from inside (decided with Justin 2026-10-18).** An agent cleaning up its throwaway daemon with `chauffeur daemon stop`, `CHAUFFEUR_HOME` forgotten, stopped the user's real daemon and every session on it. So `daemon stop`, `install` and `uninstall` (the last two stop the running daemon first) refuse when they run inside a workspace or an agent's session (`CHAUFFEUR_WORKSPACE_ID`, `CHAUFFEUR_WORKSPACE_PATH`, Claude Code's `CLAUDECODE`, Codex's `CODEX_THREAD_ID` or `AI_AGENT` set) and would act on the default state folder (`$HOME/.chauffeur` or the account's home's, through symlinks). The refusal says it is the daemon the caller's own chats run on, and to set `CHAUFFEUR_HOME` to a throwaway daemon's temp dir; any other `CHAUFFEUR_HOME` is stopped as before (the e2e scripts, verify-in-app). The user overrides it with `--real-daemon`, hidden from `--help` and never named by the refusal, so that an agent does not reach for it by reflex as it would for `--force`; `scripts/install.sh`, whose purpose is replacing the user's daemon, passes it. `daemon start` inside a workspace (`CHAUFFEUR_WORKSPACE_ID` set) still starts nothing, whatever the folder, and says it is because of the workspace, not that the daemon is not running.

## Consequences

Tests and scripts set `CHAUFFEUR_HOME` to a temp dir (`CLAUDE.md`); `scripts/install.sh` passes the hidden `--real-daemon`, which the refusal never names.
