// The agent's suggested next message (Claude's prompt suggestions), offered as grey ghost text in
// an empty composer while the chat is idle: Tab takes it (without sending), Esc sets it aside,
// typing hides it until the composer is empty again, and the next turn or suggestion replaces it.

export type Suggestion = { text: string; key: string };

/** What the composer holds, and whether the suggestion was already set aside. */
export type ComposerState = {
  text: string;
  /** Images attached: the message is not empty. */
  images: number;
  working: boolean;
  /** The key of the suggestion the user set aside (Esc, or taken with Tab). */
  dismissed: string | null;
};

/** The suggestion to show as ghost text, or null. */
export function shownSuggestion(suggestion: Suggestion | null, composer: ComposerState): Suggestion | null {
  if (!suggestion || !suggestion.text.trim()) return null;
  if (composer.working || composer.text !== "" || composer.images > 0) return null;
  if (composer.dismissed === suggestion.key) return null;
  return suggestion;
}

/** The composer's keys while a suggestion shows: Tab takes it, Esc sets it aside. */
export function suggestionKey(
  shown: Suggestion | null,
  key: { key: string; shiftKey: boolean; metaKey: boolean; ctrlKey: boolean; altKey: boolean; isComposing: boolean },
): "accept" | "dismiss" | null {
  if (!shown || key.isComposing || key.metaKey || key.ctrlKey || key.altKey) return null;
  if (key.key === "Tab" && !key.shiftKey) return "accept";
  if (key.key === "Escape") return "dismiss";
  return null;
}

/** Suggestions set aside, by chat: they stay aside when the user comes back to the chat. */
const dismissed = new Map<string, string>();

export const dismissedSuggestions = {
  get(chatId: string): string | null {
    return dismissed.get(chatId) ?? null;
  },
  set(chatId: string, key: string) {
    dismissed.set(chatId, key);
  },
};
