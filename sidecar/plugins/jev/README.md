# Jev compaction, as Chauffeur ships it

A copy of [tamaratran/fast-jev-compaction](https://github.com/tamaratran/fast-jev-compaction)
(MIT, see `LICENSE`) at commit `e3f262a`: a Claude Code function-hook plugin whose
`session.compact` hook replaces Claude Code's compaction summary with the original messages minus
the tool calls and results TypeSafe's Jev model judged no longer needed.

What differs from upstream, in `.claude-plugin/plugin.json` only:

- named `jev`, so its notices read `jev: kept 40/120 messages, no summary (…)`;
- no `apiKey` option: the key comes from `TYPESAFE_API_KEY`, which the sidecar sets from the
  key saved in Chauffeur's settings;
- `compactAtPercent` defaults to 101: its `turn.complete` trigger cannot compact an SDK session
  (`$.session.compact` is unavailable there), so the sidecar sends `/compact` itself past the
  threshold chosen in the settings (`src/agents/claude/session.ts`).

The sidecar is one compiled binary: `bun scripts/gen-jev-plugin.ts` writes these files into
`src/agents/claude/jev-plugin.generated.ts`, and the sidecar writes them back to a folder of the
temp dir for Claude Code to load (`src/agents/claude/jev.ts`). Run it after changing a file here;
a test fails while the two differ.

Codex chats use the library in `src/` directly, without the plugin: the sidecar maps a Codex
thread's history onto its messages and back (`src/agents/codex/jev.ts`).
