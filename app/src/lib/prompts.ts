// The user's saved prompts in a chat's composer (docs/adr/saved-prompts.md): the `prompts.open`
// shortcut (⇧Tab by default) opens their menu, typing filters it.

import type { Prompt } from "@chauffeur/protocol";

/** The prompts whose name or text holds the filter, ignoring case, in the daemon's order. */
export function matchPrompts(prompts: Prompt[], filter: string): Prompt[] {
  const needle = filter.toLowerCase();
  return prompts.filter((prompt) => prompt.name.toLowerCase().includes(needle) || prompt.text.toLowerCase().includes(needle));
}
