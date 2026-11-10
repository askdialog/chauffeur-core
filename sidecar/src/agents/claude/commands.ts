// The commands Claude Code runs when a message starts with `/name` (the SDK's
// `supportedCommands`) → SlashCommand: skills and commands of the user, the plugins and the
// workspace's repos, and those of Claude Code's own that work in a chat.

import type { SlashCommand as SdkCommand } from "@anthropic-ai/claude-agent-sdk";
import type { SlashCommand } from "../../protocol.ts";
import { probeClaude } from "./models.ts";
import type { ClaudeSessionDeps } from "./session.ts";

/**
 * Claude Code's own commands that are prompts, and so work in a chat. The others drive its
 * terminal (colors, focus view…), a setting Chauffeur owns (model, effort, fast mode,
 * permissions), or the session itself (`/clear` starts another one).
 */
const BUILTINS_IN_A_CHAT = new Set([
  "batch",
  "claude-api",
  "code-review",
  "dataviz",
  "deep-research",
  "fewer-permission-prompts",
  "init",
  "run",
  "security-review",
  "simplify",
  "update-config",
  "verify",
]);

export function claudeCommands(rows: SdkCommand[]): SlashCommand[] {
  // A name can appear twice; Claude Code's own command is the one `/name` runs.
  const builtins = new Set(rows.filter((row) => row.builtin).map((row) => row.name));
  const seen = new Set<string>();
  const commands: SlashCommand[] = [];
  for (const row of rows) {
    const builtin = row.builtin === true;
    if (seen.has(row.name) || (builtins.has(row.name) && !builtin)) continue;
    if (builtin && !BUILTINS_IN_A_CHAT.has(row.name)) continue;
    seen.add(row.name);
    commands.push({ name: row.name, description: row.description ?? "", argument_hint: row.argumentHint ?? "", builtin });
  }
  return commands;
}

/** The commands a session started in `cwd`, with `directories` added, would offer. */
export async function listClaudeCommands(deps: ClaudeSessionDeps, cwd: string, directories: string[]): Promise<SlashCommand[]> {
  const rows = await probeClaude(
    deps,
    { cwd, additionalDirectories: directories, settingSources: ["user", "project", "local"] },
    (query) => query.supportedCommands(),
    "list its commands",
  );
  return claudeCommands(rows);
}
