# Third-party notices

Chauffeur Core's MIT license applies to its own code and documentation. Third-party material
retains its original license and attribution.

## Vendored source

- **Jev compaction** — `sidecar/plugins/jev/`, copied from
  [tamaratran/fast-jev-compaction](https://github.com/tamaratran/fast-jev-compaction) at `e3f262a`,
  under its [MIT license](sidecar/plugins/jev/LICENSE). The
  [plugin README](sidecar/plugins/jev/README.md) records Chauffeur's modifications. Its embedded
  copy in `sidecar/src/agents/claude/jev-plugin.generated.ts` carries the same license.
- **OpenAI Codex protocol bindings** — `sidecar/src/agents/codex/generated/`, generated from
  codex-cli `0.167.0` with `codex app-server generate-ts`. Codex is Copyright 2025 OpenAI,
  licensed under [Apache-2.0](sidecar/licenses/codex-LICENSE); its
  [upstream notice](sidecar/licenses/codex-NOTICE) is retained alongside the license.
  Regenerating the bindings must preserve these files and update their provenance when the
  Codex version changes.

## Dependencies and external tools

Rust and JavaScript dependencies retain their own licenses. The lockfiles record the versions
used; their source packages supply their license texts. Core's MIT license does not relicense
Claude Code, the Claude Agent SDK, Codex, or other external tools. Building or redistributing a
bundle also requires retaining the notices of the dependencies and tools included in that bundle.
