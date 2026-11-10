// The workspace view's split, as in VS Code: a main pane with the tab bar, and at most one side
// pane, on the right or at the bottom, showing another view. Pure: which view goes where, the
// panes' boxes, the collapse rule for a small window, the drop zones of a dragged tab, and what is
// kept per workspace across restarts of the app.

export type ViewId = "chat" | "diff" | "files" | "terminal" | "processes" | "database" | "routine";
export type SplitPosition = "right" | "bottom";

export const VIEWS: ViewId[] = ["chat", "diff", "files", "terminal", "processes", "database", "routine"];

export type SplitLayout = {
  /** The view in the side pane; null when the split is closed. */
  side: ViewId | null;
  /** The view ⌘\ opens again. */
  last: ViewId;
  position: SplitPosition;
  /** The side pane's share of the view area, for each position. */
  size: Record<SplitPosition, number>;
};

export const DEFAULT_LAYOUT: SplitLayout = {
  side: null,
  last: "terminal",
  position: "right",
  size: { right: 0.42, bottom: 0.38 },
};

/** The smallest a pane gets: its width beside the other, its height below it. */
export const MIN_PANE: Record<SplitPosition, number> = { right: 320, bottom: 160 };
/** The divider's grab area between the panes. */
export const DIVIDER = 5;
/** The side pane's own header (its view's name, swap, close). */
export const SIDE_HEADER = 28;

/** What each pane shows. The split waits while the window is too small or both would show one view. */
export function panes(layout: SplitLayout, main: ViewId, narrow: boolean): { main: ViewId; side: ViewId | null } {
  const side = layout.side && !narrow && layout.side !== main ? layout.side : null;
  return { main, side };
}

/** The main pane's view when none was picked yet: the chat, unless the chat is on the side. */
export function defaultMain(layout: SplitLayout): ViewId {
  return layout.side === "chat" ? "terminal" : "chat";
}

/** What the main pane shows when its view leaves for the side. */
export function fallbackMain(leaving: ViewId, previousSide: ViewId | null): ViewId {
  if (previousSide && previousSide !== leaving) return previousSide;
  return leaving === "chat" ? "terminal" : "chat";
}

type Change = { layout: SplitLayout; main: ViewId };

/** Puts `view` in the side pane; the main pane gives it up for another view if it showed it. */
export function openSide(layout: SplitLayout, main: ViewId, view: ViewId, position: SplitPosition): Change {
  return {
    layout: { ...layout, side: view, last: view, position },
    main: main === view ? fallbackMain(view, layout.side) : main,
  };
}

/** Closes the side pane; ⌘\ brings its view back. */
export function closeSide(layout: SplitLayout): SplitLayout {
  return { ...layout, side: null, last: layout.side ?? layout.last };
}

/** Shows `view` in the main pane, out of the side pane if it was there. */
export function moveToMain(layout: SplitLayout, view: ViewId): Change {
  return { layout: layout.side === view ? closeSide(layout) : layout, main: view };
}

/** ⌘\: closes the side pane, or opens its last view where it was (Terminal on the right at first). */
export function toggleSide(layout: SplitLayout, main: ViewId): Change {
  if (layout.side) return { layout: closeSide(layout), main };
  return openSide(layout, main, layout.last, layout.position);
}

export function otherPosition(position: SplitPosition): SplitPosition {
  return position === "right" ? "bottom" : "right";
}

/**
 * A view tab clicked (or a view asked for, as a file opened from a chat): the view the side pane
 * shows is focused there rather than shown twice; any other goes to the main pane.
 */
export function showView(
  layout: SplitLayout,
  main: ViewId,
  view: ViewId,
  narrow: boolean,
): { main: ViewId; focusSide: boolean } {
  if (panes(layout, main, narrow).side === view) return { main, focusSide: true };
  return { main: view, focusSide: false };
}

/** Whether the view area is too small for two panes side by side (or one above the other). */
export function tooSmall(position: SplitPosition, width: number, height: number): boolean {
  if (position === "right") return width < MIN_PANE.right * 2 + DIVIDER;
  return height < MIN_PANE.bottom * 2 + DIVIDER;
}

/** The side pane's size in pixels: its share of `total`, each pane kept at its minimum. */
export function sidePixels(position: SplitPosition, share: number, total: number): number {
  const min = MIN_PANE[position];
  const max = total - min - DIVIDER;
  const wanted = Math.round(share * total);
  if (max < min) return Math.max(0, Math.round((total - DIVIDER) / 2));
  return Math.min(max, Math.max(min, wanted));
}

export type Box = { left: number; top: number; width: number; height: number };

/** Where the panes, the divider and the side pane's header sit in a view area of this size. */
export function splitBoxes(
  position: SplitPosition,
  share: number,
  width: number,
  height: number,
): { main: Box; divider: Box; header: Box; side: Box } {
  if (position === "right") {
    const side = sidePixels("right", share, width);
    const left = width - side;
    return {
      main: { left: 0, top: 0, width: left - DIVIDER, height },
      divider: { left: left - DIVIDER, top: 0, width: DIVIDER, height },
      header: { left, top: 0, width: side, height: SIDE_HEADER },
      side: { left, top: SIDE_HEADER, width: side, height: Math.max(0, height - SIDE_HEADER) },
    };
  }
  const side = sidePixels("bottom", share, height);
  const top = height - side;
  return {
    main: { left: 0, top: 0, width, height: top - DIVIDER },
    divider: { left: 0, top: top - DIVIDER, width, height: DIVIDER },
    header: { left: 0, top, width, height: SIDE_HEADER },
    side: { left: 0, top: top + SIDE_HEADER, width, height: Math.max(0, side - SIDE_HEADER) },
  };
}

/** The side pane's share for the divider dragged to `point` in `area` (page coordinates). */
export function shareAt(position: SplitPosition, area: Box, point: { x: number; y: number }): number {
  const total = position === "right" ? area.width : area.height;
  if (total <= 0) return DEFAULT_LAYOUT.size[position];
  const offset = position === "right" ? area.left + area.width - point.x : area.top + area.height - point.y;
  return sidePixels(position, offset / total, total) / total;
}

/** Past this share of the view area, from its right or bottom edge, a dragged tab lands on the side. */
export const DROP_EDGE = 0.33;

/**
 * Where a view tab dragged over the view area would land: on the right or at the bottom when near
 * that edge (the nearer one, relative to the area's size, when near both), nowhere in the middle.
 */
export function dropZone(area: Box, point: { x: number; y: number }): SplitPosition | null {
  if (area.width <= 0 || area.height <= 0) return null;
  const fx = (point.x - area.left) / area.width;
  const fy = (point.y - area.top) / area.height;
  if (fx < 0 || fx > 1 || fy < 0 || fy > 1) return null;
  const right = fx >= 1 - DROP_EDGE;
  const bottom = fy >= 1 - DROP_EDGE;
  if (right && bottom) return 1 - fx <= 1 - fy ? "right" : "bottom";
  return right ? "right" : bottom ? "bottom" : null;
}

export function inBox(box: Box, point: { x: number; y: number }): boolean {
  return point.x >= box.left && point.x <= box.left + box.width && point.y >= box.top && point.y <= box.top + box.height;
}

// --- kept per workspace ---

export const LAYOUTS_KEY = "chauffeur.splitLayouts";
/** Workspaces whose layout is kept at most: the oldest go first. */
const MAX_LAYOUTS = 200;

function isView(value: unknown): value is ViewId {
  return typeof value === "string" && (VIEWS as string[]).includes(value);
}

function share(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) && value > 0 && value < 1 ? value : fallback;
}

/** One workspace's layout read back, anything unknown or out of range left at its default. */
export function parseLayout(value: unknown): SplitLayout {
  const raw = (value && typeof value === "object" ? value : {}) as Record<string, unknown>;
  const size = (raw.size && typeof raw.size === "object" ? raw.size : {}) as Record<string, unknown>;
  return {
    side: isView(raw.side) ? raw.side : null,
    last: isView(raw.last) ? raw.last : DEFAULT_LAYOUT.last,
    position: raw.position === "bottom" ? "bottom" : "right",
    size: {
      right: share(size.right, DEFAULT_LAYOUT.size.right),
      bottom: share(size.bottom, DEFAULT_LAYOUT.size.bottom),
    },
  };
}

export function parseLayouts(text: string | null): Record<string, SplitLayout> {
  let raw: unknown;
  try {
    raw = JSON.parse(text ?? "{}");
  } catch {
    return {};
  }
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return {};
  const layouts: Record<string, SplitLayout> = {};
  for (const [id, value] of Object.entries(raw)) layouts[id] = parseLayout(value);
  return layouts;
}

/** The layouts to keep, `changed` last so that it is the newest. */
export function serializeLayouts(layouts: Record<string, SplitLayout>, changed?: string): string {
  const entries = Object.entries(layouts).filter(([id]) => id !== changed);
  if (changed && layouts[changed]) entries.push([changed, layouts[changed]]);
  return JSON.stringify(Object.fromEntries(entries.slice(-MAX_LAYOUTS)));
}
