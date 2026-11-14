/** The end is followed until the reader moves away, regardless of their input device. */
export type ChatScrollIntent = { following: boolean; awaitingUserScroll: boolean };
export type ChatScrollPosition = { top: number; height: number; viewport: number };

export function atChatEnd(position: ChatScrollPosition): boolean {
  return position.height - position.top - position.viewport <= 2;
}

/** Distinguish moving into history from the scroll compensation caused by new row sizes. */
export function afterChatScroll(
  intent: ChatScrollIntent,
  before: ChatScrollPosition,
  after: ChatScrollPosition,
  jumping: boolean,
  measuring = false,
): ChatScrollIntent {
  if (jumping) return { ...intent, following: false };
  // An already queued bottom event must not undo an upward wheel before it takes effect.
  if (intent.awaitingUserScroll && atChatEnd(after)) return intent;
  if (atChatEnd(after)) return { following: true, awaitingUserScroll: false };
  if (measuring && !intent.awaitingUserScroll) return intent;
  if (after.top - before.top < Math.min(0, after.height - before.height) - 1) {
    return { following: false, awaitingUserScroll: false };
  }
  return intent;
}

/** The size used by Virtuoso until a row has been drawn. */
export const CHAT_ROW_ESTIMATE = 60;
export const CHAT_LIST_HEADER = 12;

export function chatRowPosition(
  ranges: readonly { startIndex: number; endIndex: number; size: number }[],
  index: number,
): { top: number; height: number } {
  let top = 0;
  let next = 0;
  for (const range of ranges) {
    if (index < range.startIndex) return { top: top + (index - next) * CHAT_ROW_ESTIMATE, height: CHAT_ROW_ESTIMATE };
    top += (range.startIndex - next) * CHAT_ROW_ESTIMATE;
    if (index <= range.endIndex) return { top: top + (index - range.startIndex) * range.size, height: range.size };
    top += (range.endIndex - range.startIndex + 1) * range.size;
    next = range.endIndex + 1;
  }
  return { top: top + (index - next) * CHAT_ROW_ESTIMATE, height: CHAT_ROW_ESTIMATE };
}
