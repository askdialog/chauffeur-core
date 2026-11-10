// Dragging a tab along its strip, as a browser's: where it lands, how the others make room, when
// the strip scrolls. Pointer events only: WebKitGTK's HTML5 drag and drop is unreliable.

/** How far the pointer goes, in px, before a press on a tab becomes a drag (a click otherwise). */
export const DRAG_THRESHOLD = 4;

/** A tab's box along the strip, in the strip's scrolled content (x from its start). */
export type TabBox = { left: number; width: number };

/**
 * The index the dragged tab lands at, now at `left`, among the tabs once it is taken out: past
 * each tab before it whose middle its left edge has not passed back over, and each tab after it
 * whose middle its right edge passed. Its edges, not its middle: held at a strip's end, it still
 * passes a narrower tab there.
 */
export function dropIndex(boxes: TabBox[], from: number, left: number): number {
  const right = left + (boxes[from]?.width ?? 0);
  return boxes.filter((box, i) => {
    const middle = box.left + box.width / 2;
    return (i < from && middle < left) || (i > from && middle < right);
  }).length;
}

/** How far a tab that is not dragged moves aside to show where the dragged one would land. */
export function tabShift(index: number, from: number, to: number, width: number): number {
  if (from < to && index > from && index <= to) return -width;
  if (to < from && index >= to && index < from) return width;
  return 0;
}

/** The tab the moved one goes before once moved from `from` to `to`; undefined for the last place. */
export function beforeOf(ids: string[], from: number, to: number): string | undefined {
  const rest = ids.filter((_, i) => i !== from);
  return rest[to];
}

/** `ids` with `id` moved before `before`, or last, as the daemon orders them (`chat.move`, `terminal.move`). */
export function movedBefore(ids: string[], id: string, before: string | undefined): string[] {
  const rest = ids.filter((other) => other !== id);
  const at = before === undefined ? -1 : rest.indexOf(before);
  rest.splice(at < 0 ? rest.length : at, 0, id);
  return rest;
}

/** Where the strip scrolls while a tab is dragged near one of its edges: px per frame, signed. */
export function edgeScroll(x: number, left: number, right: number, zone = 28, fastest = 12): number {
  if (x < left + zone) return -Math.ceil((fastest * Math.min(zone, left + zone - x)) / zone);
  if (x > right - zone) return Math.ceil((fastest * Math.min(zone, x - (right - zone))) / zone);
  return 0;
}
