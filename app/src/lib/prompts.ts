// The user's saved prompts in a chat's composer (docs/adr/saved-prompts.md): ⇧Tab opens their menu,
// typing filters it.

import type { Prompt } from "@chauffeur/protocol";

/** Whether the key opens the prompts menu: ⇧Tab alone, fixed as the composer's other keys are. */
export function opensPrompts(key: Pick<KeyboardEvent, "key" | "shiftKey" | "metaKey" | "ctrlKey" | "altKey">): boolean {
  return key.key === "Tab" && key.shiftKey && !key.metaKey && !key.ctrlKey && !key.altKey;
}

/** The prompts whose name or text holds the filter, ignoring case, in the daemon's order. */
export function matchPrompts(prompts: Prompt[], filter: string): Prompt[] {
  const needle = filter.toLowerCase();
  return prompts.filter((prompt) => prompt.name.toLowerCase().includes(needle) || prompt.text.toLowerCase().includes(needle));
}
