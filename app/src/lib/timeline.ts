// The timeline rail of a chat: one tick per user message, placed where that message sits in the
// whole conversation, to jump back to it. Pure, tested in timeline.test.ts; the rail itself is
// components/chat/TimelineRail.tsx.

import type { ChatRow, UserItem } from "./chat";

/** A user message the rail marks. `index` is its row in the list. */
export type TimelineMark = { index: number; key: string; preview: string; ts: string };

/** Rows sharing one height, as the virtualised list reports them (`endIndex` may be Infinity). */
export type SizeRange = { startIndex: number; endIndex: number; size: number };

/** Where each row starts and how tall it is, in pixels from the top of the first row. */
export type RowLayout = { offsets: number[]; heights: number[]; total: number };

/** What the scroll area shows: `header` is the space above the first row; all in pixels. */
export type Viewport = { scrollTop: number; clientHeight: number; scrollHeight: number; header: number };

/** A mark placed on the rail: `top` is its position over the whole conversation, from 0 to 1. */
export type TimelineTick = TimelineMark & { top: number; inView: boolean };

// A message's preview, worked out once: the rows are listed again at each streamed token.
const previews = new WeakMap<UserItem, string>();

function previewOf(row: UserItem): string {
  const known = previews.get(row);
  if (known !== undefined) return known;
  const files = row.attachments.length;
  const preview = messagePreview(row.text) || (files > 0 ? `${files} ${files === 1 ? "attachment" : "attachments"}` : "(empty message)");
  previews.set(row, preview);
  return preview;
}

/**
 * The user messages of the chat, in order. Returns `previous` when the marks are the same, so that
 * what depends on them does not run again while only the agent's rows change.
 */
export function timelineMarks(rows: readonly ChatRow[], previous: readonly TimelineMark[] = []): readonly TimelineMark[] {
  const marks: TimelineMark[] = [];
  rows.forEach((row, index) => {
    if (row.kind === "user") marks.push({ index, key: row.key, preview: previewOf(row), ts: row.ts });
  });
  const same =
    marks.length === previous.length &&
    marks.every((mark, i) => mark.index === previous[i].index && mark.key === previous[i].key && mark.preview === previous[i].preview && mark.ts === previous[i].ts);
  return same ? previous : marks;
}

/** The start of a message on one line: whitespace collapsed, cut at a word with an ellipsis. */
export function messagePreview(text: string, max = 140): string {
  const flat = text.replace(/\s+/g, " ").trim();
  if (flat.length <= max) return flat;
  const cut = flat.slice(0, max);
  const space = cut.lastIndexOf(" ");
  return `${(space > max * 0.6 ? cut.slice(0, space) : cut).replace(/[\s.,;:!?-]+$/, "")}…`;
}

/** The layout of `count` rows from the list's measured (or estimated) size ranges. */
export function rowLayout(ranges: readonly SizeRange[], count: number): RowLayout {
  const offsets = new Array<number>(count);
  const heights = new Array<number>(count);
  let top = 0;
  let r = 0;
  for (let i = 0; i < count; i++) {
    while (r < ranges.length - 1 && i > ranges[r].endIndex) r++;
    const range = ranges[r];
    const size = range && i >= range.startIndex && i <= range.endIndex ? range.size : 0;
    offsets[i] = top;
    heights[i] = size;
    top += size;
  }
  return { offsets, heights, total: top };
}

/** Whether the rail is worth showing: two user messages or more, in a chat that scrolls. */
export function railShown(marks: readonly TimelineMark[], viewport: Viewport): boolean {
  return marks.length >= 2 && viewport.scrollHeight > viewport.clientHeight + 1;
}

/**
 * Places each mark at its row's offset over the scroll height, and tells whether any of the row
 * is in the scroll area's window.
 */
export function placeTicks(marks: readonly TimelineMark[], layout: RowLayout, viewport: Viewport): TimelineTick[] {
  const { scrollTop, clientHeight, scrollHeight, header } = viewport;
  const height = Math.max(1, scrollHeight);
  return marks.flatMap((mark) => {
    if (mark.index >= layout.offsets.length) return [];
    const start = header + layout.offsets[mark.index];
    const end = start + layout.heights[mark.index];
    const top = Math.min(1, Math.max(0, start / height));
    const inView = end > scrollTop && start < scrollTop + clientHeight;
    return [{ ...mark, top, inView }];
  });
}

/** Ticks equal for the rail: lets a frame that changed nothing skip a render. */
export function sameTicks(a: readonly TimelineTick[], b: readonly TimelineTick[]): boolean {
  if (a.length !== b.length) return false;
  return a.every(
    (tick, i) =>
      tick.key === b[i].key &&
      tick.inView === b[i].inView &&
      tick.preview === b[i].preview &&
      Math.abs(tick.top - b[i].top) < 0.001,
  );
}

/** The time a message was sent, for its preview: the clock today, the day before that. */
export function markTime(iso: string, now = new Date()): string {
  const t = new Date(iso);
  if (Number.isNaN(t.getTime())) return "";
  const time = t.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  if (t.toDateString() === now.toDateString()) return time;
  const day = t.toLocaleDateString([], { month: "short", day: "numeric" });
  return `${day}, ${time}`;
}

/** Marks sharing a 2px stroke keep one hit target, with the current message kept visible. */
export function pixelTicks(ticks: readonly TimelineTick[], height: number, active: string | null = null): TimelineTick[] {
  const pixels = new Map<number, TimelineTick>();
  const extent = Math.max(2, Math.floor(height / 2) * 2);
  for (const tick of ticks) {
    const pixel = Math.round(tick.top * extent / 2) * 2;
    const previous = pixels.get(pixel);
    if (previous?.key === active) continue;
    if (tick.key === active || !previous?.inView || tick.inView) pixels.set(pixel, tick);
  }
  return [...pixels.entries()].map(([pixel, tick]) => ({ ...tick, top: pixel / extent }));
}
