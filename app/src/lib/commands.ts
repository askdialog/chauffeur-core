// The `/` menu of the composer: when it opens, and which commands match what was typed.

import type { SlashCommand } from "@chauffeur/protocol";

/** What follows the slash while the message is a lone `/word` being typed; null otherwise. */
export function slashQuery(text: string): string | null {
  const match = /^\/(\S*)$/.exec(text);
  return match ? (match[1] ?? "") : null;
}

/**
 * The commands matching `query`, best first: the name starts with it, then a part of the name does
 * (`impl` → `acme-skills:implement-ticket`), then the name or the description contains it. Ties
 * keep the agent's order (the user's and the repos' skills before the agent's own commands).
 */
export function matchCommands(commands: SlashCommand[], query: string): SlashCommand[] {
  const q = query.toLowerCase();
  if (!q) return commands;
  const rank = (command: SlashCommand): number => {
    const name = command.name.toLowerCase();
    if (name.startsWith(q)) return 0;
    if (name.split(/[:\-_]/).some((part) => part.startsWith(q))) return 1;
    if (name.includes(q)) return 2;
    if (command.description.toLowerCase().includes(q)) return 3;
    return -1;
  };
  return commands
    .map((command, index) => ({ command, index, rank: rank(command) }))
    .filter((entry) => entry.rank >= 0)
    .sort((a, b) => a.rank - b.rank || a.index - b.index)
    .map((entry) => entry.command);
}
