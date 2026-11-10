// Codex's skills (`skills/list`) for the `/` menu, and the `skill` input item that runs one: Codex
// does not read a `/name` typed in a message, it runs the skill named by that item.

import { existsSync } from "node:fs";
import { join } from "node:path";
import type { SlashCommand } from "../../protocol.ts";
import type { Logger } from "../../log.ts";
import type { AppServer } from "./app-server.ts";
import { probeCodex } from "./models.ts";
import type { SkillMetadata, SkillsListResponse, UserInput } from "./protocol.ts";
import type { CodexSessionDeps } from "./session.ts";

/**
 * The skill folders of the workspace's repos. Codex finds a repo's skills from a cwd inside the
 * repo; a session starts above the repos, so they are handed to it as extra roots.
 */
export function repoSkillRoots(directories: string[]): string[] {
  return directories.flatMap((dir) => [join(dir, ".agents", "skills"), join(dir, ".codex", "skills")]).filter((root) => existsSync(root));
}

/** The enabled skills Codex finds from `cwd` and each directory (a repo's own), first one wins. */
export async function codexSkills(server: AppServer, cwd: string, directories: string[]): Promise<SkillMetadata[]> {
  const listed: SkillsListResponse = await server.request("skills/list", { cwds: [cwd, ...directories] });
  const byName = new Map<string, SkillMetadata>();
  for (const entry of listed.data) {
    for (const skill of entry.skills) {
      if (skill.enabled && !byName.has(skill.name)) byName.set(skill.name, skill);
    }
  }
  return [...byName.values()];
}

export function skillCommands(skills: SkillMetadata[]): SlashCommand[] {
  return skills.map((skill) => ({
    name: skill.name,
    description: skill.shortDescription || skill.description,
    argument_hint: "",
    builtin: skill.scope === "system",
  }));
}

/** Chauffeur's own command: the session compacts the thread (see CodexSession). */
export const COMPACT_COMMAND: SlashCommand = {
  name: "compact",
  description: "Compact the conversation: through Jev when it is on, else Codex's own way",
  argument_hint: "",
  builtin: true,
};

/** The commands a Codex session started in `cwd`, with `directories` added, runs on `/name`. */
export async function listCodexCommands(deps: CodexSessionDeps, log: Logger, cwd: string, directories: string[]): Promise<SlashCommand[]> {
  return probeCodex(deps, log, cwd, "list its skills", async (server) => [
    COMPACT_COMMAND,
    ...skillCommands(await codexSkills(server, cwd, directories)).filter((command) => command.name !== COMPACT_COMMAND.name),
  ]);
}

/** The skill a message starting with `/name` runs, as the input item that runs it. */
export function skillInput(text: string, skills: ReadonlyMap<string, string>): UserInput | null {
  const name = /^\/([^\s/]+)(?:\s|$)/.exec(text.trimStart())?.[1];
  const path = name ? skills.get(name) : undefined;
  return name && path ? { type: "skill", name, path } : null;
}
