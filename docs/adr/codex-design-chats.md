# Design chats run on Codex too

- **Date:** 2026-11-02
- **Status:** accepted

## Context

A user asked to design with OpenAI's models. In Chauffeur they run only through Codex, which design chats refused ([0018](0018-design-chats.md), [0053](0053-create-design-from-command-n.md)): Codex had no file-path guard, so "never code" would have rested on its sandbox alone, untested. A test on 2026-11-02 (codex-cli 0.162.1, macOS) ran Codex in its workspace-write sandbox with a design folder as its only writable root and the approval policy `never`. Patches into a repo were rejected, by path or through a symlink. Shell writes, a hard link, a `git commit` and a Python write all failed. Writes in the folder passed. The test also showed two traps. Workspace-write lets Codex write `/tmp` and `$TMPDIR` by default. And `never` refuses every MCP tool that needs an approval, the design browser's included.

## Decision

Asked by a user, decided with Justin 2026-11-02: a design chat runs on Claude Code or Codex. It starts on Claude Code (the first Claude favorite) unless Codex is picked: in ⌘N's model picker, in the composer's (a provider change, as any chat's), with `chat.create`'s `agent`, or with `chauffeur chat --design --agent codex`. A Codex design chat writes only its design folder. Codex's sandbox enforces it: the folder is its only writable root, `/tmp`, `$TMPDIR` and the network are left out, and the approval policy is `never`, so it never asks to leave the sandbox. The design browser's tools are approved by the thread's config.

## Consequences

- The sidecar sets the design sandbox on the thread's config and on each turn. A user's or a repo's config cannot widen it (writable roots, network). The repos are not writable roots, and the daemon's socket is not reachable.
- Chauffeur's `PreToolUse` hook refuses git and GitHub writes with the design chat's reason. A Codex design chat gets no Chauffeur tool.
- Without auto mode's classifier, the user's MCP tools that need an approval are refused in a Codex design chat, unless the user's Codex config approves them.
- Codex has no Artifact tool: no "Open on claude.ai", and its system prompt does not mention it.
- The design brief is still written by the user's default Claude model.
- Not tested on Linux, where Codex's sandbox is different.
