# Codex is updated from Chauffeur, in Settings › Agents and beside the error it causes

- **Date:** 2026-11-26
- **Status:** accepted

## Context

GPT-6.2 Sol failed on every first message with "Codex could not confirm gpt-6.2-sol in its model catalog", for Justin and for a teammate who could not find it at all. OpenAI offers a new model only to a Codex recent enough for it: Chauffeur ran the user's `codex` 0.167.0, whose catalog has no GPT-6.2, where 0.172.0 lists it. The picker had shown it from a catalog the Codex desktop app (0.169.1) had written in `~/.codex`. Fixing it took `npm i -g @openai/codex@latest` in a terminal; Justin asked for a button in Chauffeur instead.

## Decision

Asked by Justin, decided with Justin 2026-11-26:

- Settings › Agents shows the installed Codex's version and the latest published, and "Update Codex" when a newer one is out; the error of a first turn whose model Codex's catalog lacks offers the same button, after which the user resends the message.
- Codex only: Claude Code and Mistral Vibe can follow later.
- Once updated, the idle Codex chats restart on the new Codex at once (their sessions close and their next message resumes them, the conversation kept); a working chat is never interrupted and takes it at its next message once idle.

## Consequences

- The daemon runs `codex update`, Codex's own command, which picks the way Codex was installed (npm, bun, pnpm, Homebrew, its installer): Chauffeur does not guess the package manager. It runs with the login shell's environment, as the sidecar runs `codex`, so it updates the `codex` chats use; in the background (an npm install can outlast the sidecar's 120 s calls), followed through `codex.status`.
- The latest version comes from npm's registry, read at most hourly. A Homebrew cask can lag behind it: `codex update` then ends with Codex still older, which is said as a failure.
- A chat whose first turn failed has a Codex thread with no turn, which Codex cannot resume ("no rollout found"): the sidecar starts a new thread in its place, nothing being lost.
- Updating Codex past the version `protocol-ts`'s Codex types come from (`CODEX_APP_SERVER_VERSION`) only logs a warning, as a user's own update already did.
