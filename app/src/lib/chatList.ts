// Where a chat's list stands when the start of its conversation arrives under it.

import type { ChatRow } from "./chat";

export type ListLocation = { index: number | "LAST"; align: "start" | "end"; offset?: number };

/** The first item a row holds: an activity group stands for several. */
export function firstItemKey(row: ChatRow): string {
  return row.kind === "activity" ? row.items[0].key : row.key;
}

/**
 * Where the list mounts again once earlier messages joined it: at the bottom if the reader was
 * there, else on the row that holds the item they saw at the top (it may have joined a group).
 */
export function remountLocation(rows: ChatRow[], topKey: string | null, atBottom: boolean, offset = 0): ListLocation {
  const end: ListLocation = { index: "LAST", align: "end" };
  if (atBottom || topKey === null) return end;
  const index = rows.findIndex((row) => (row.kind === "activity" ? row.items.some((item) => item.key === topKey) : row.key === topKey));
  return index < 0 ? end : { index, align: "start", ...(offset ? { offset } : {}) };
}
