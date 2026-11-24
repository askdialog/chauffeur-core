// Recalling the messages already sent from the composer with ↑ and ↓, as Claude Code does: ↑ walks
// back through them, ↓ forward again, down to the text that was being written before the first ↑.

export type Recall = {
  /** How far back the shown text is: 0 is the draft, 1 the last message sent. */
  depth: number;
  /** The draft the user was writing before the first ↑, put back by the ↓ that leaves the history. */
  draft: string;
};

export const NO_RECALL: Recall = { depth: 0, draft: "" };

/** The messages worth recalling, oldest first, without a message repeated right after itself. */
export function recallable(sent: readonly string[]): string[] {
  const out: string[] = [];
  for (const raw of sent) {
    const text = raw.trim();
    if (text && out[out.length - 1] !== text) out.push(text);
  }
  return out;
}

/** The text and state after ↑ (`up`) or ↓; null when there is nothing to move to. */
export function recall(
  history: readonly string[],
  state: Recall,
  current: string,
  direction: "up" | "down",
): { text: string; state: Recall } | null {
  if (direction === "up") {
    if (state.depth >= history.length) return null;
    const draft = state.depth === 0 ? current : state.draft;
    const depth = state.depth + 1;
    return { text: history[history.length - depth], state: { depth, draft } };
  }
  if (state.depth === 0) return null;
  const depth = state.depth - 1;
  return { text: depth === 0 ? state.draft : history[history.length - depth], state: { depth, draft: state.draft } };
}
