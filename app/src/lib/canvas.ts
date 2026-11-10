// A design chat's canvas (story 47), kept pure so that it is tested in canvas.test.ts: which
// sections a page shows, where its screens sit, the zoom (fit, steps, around the pointer), the
// order Present walks, which of the agent's tool calls change the canvas, the selection, and
// whether the tab is too narrow for the chat and the canvas side by side.
//
// Geometry. Screens live in canvas pixels, scaled by the zoom on one layer; the section titles and
// the screen labels stay at their size on screen, whatever the zoom, so that they can be read.
// Each section is a header (its title, then its screens' labels, `HEADER` screen pixels) above a
// row of screens; sections are `SECTION_GAP` canvas pixels apart. A point of the canvas is thus
// on screen at `view.y + scale * rowsY + headers * HEADER`, `rowsY` counting rows and gaps only.

import type { CanvasPage, CanvasResult, CanvasScreen, JsonValue } from "@chauffeur/protocol";
import type { ChatItem } from "./chat";

/** A section of the page shown; `unsorted` for the last one, of the files no page lists. */
export type ShownSection = { title: string; screens: CanvasScreen[]; unsorted: boolean };

/** The section a page shows: its own, then "Unsorted" when files are not in the manifest. */
export function pageSections(result: Pick<CanvasResult, "pages" | "unsorted">, page: number): ShownSection[] {
  const own: CanvasPage | undefined = result.pages[Math.min(Math.max(0, page), result.pages.length - 1)];
  const sections: ShownSection[] = (own?.sections ?? []).map((section) => ({ ...section, unsorted: false }));
  if (result.unsorted.length > 0) sections.push({ title: "Unsorted", screens: result.unsorted, unsorted: true });
  return sections;
}

/** Whether the canvas has no screen at all, on any page. */
export function noScreens(result: Pick<CanvasResult, "pages" | "unsorted">): boolean {
  return result.unsorted.length === 0 && result.pages.every((page) => page.sections.every((s) => s.screens.length === 0));
}

/** The source of a screen's iframe: a new `modified_ms` makes it a new URL, which reloads it. */
export function screenSrc(screen: Pick<CanvasScreen, "url" | "modified_ms">): string | null {
  if (!screen.url) return null;
  if (screen.modified_ms == null) return screen.url;
  return `${screen.url}${screen.url.includes("?") ? "&" : "?"}v=${screen.modified_ms}`;
}

// --- layout ---

/** A section's title and its screens' labels, above its row, in screen pixels. */
export const TITLE_HEIGHT = 30;
export const LABEL_HEIGHT = 22;
export const HEADER = TITLE_HEIGHT + LABEL_HEIGHT;
/** Between two screens of a row, and between two sections, in canvas pixels. */
export const SCREEN_GAP = 40;
export const SECTION_GAP = 120;

export type PlacedScreen = { id: string; screen: CanvasScreen; x: number; width: number; height: number };
export type PlacedSection = {
  title: string;
  unsorted: boolean;
  /** Its row's top in rows space (rows and gaps above it, headers left out). */
  rowsY: number;
  /** Its row's height: its tallest screen. */
  height: number;
  /** Its row's width. */
  width: number;
  screens: PlacedScreen[];
};
export type CanvasLayout = {
  sections: PlacedSection[];
  /** The widest row. */
  width: number;
  /** The rows and the gaps between them, headers left out. */
  rowsHeight: number;
};

export function layoutCanvas(sections: ShownSection[]): CanvasLayout {
  let rowsY = 0;
  let width = 0;
  const placed = sections.map((section, index) => {
    if (index > 0) rowsY += SECTION_GAP;
    let x = 0;
    const occurrences = new Map<string, number>();
    const screens = section.screens.map((screen, i) => {
      if (i > 0) x += SCREEN_GAP;
      const occurrence = occurrences.get(screen.file) ?? 0;
      occurrences.set(screen.file, occurrence + 1);
      // One file may appear several times, even at different sizes, within a section.
      const id = JSON.stringify([index, screen.file, occurrence, screen.width, screen.height]);
      const box = { id, screen, x, width: Math.max(1, screen.width), height: Math.max(1, screen.height) };
      x += box.width;
      return box;
    });
    const height = screens.reduce((tallest, s) => Math.max(tallest, s.height), 0);
    const result: PlacedSection = { title: section.title, unsorted: section.unsorted, rowsY, height, width: x, screens };
    rowsY += height;
    width = Math.max(width, x);
    return result;
  });
  return { sections: placed, width, rowsHeight: rowsY };
}

/** The zoom: canvas point `p` shows at `x + scale * p.x` (and likewise down, headers added). */
export type View = { scale: number; x: number; y: number };
export type Size = { width: number; height: number };
export type Point = { x: number; y: number };

export const MIN_SCALE = 0.05;
export const MAX_SCALE = 4;
/** Around the page fitted, in screen pixels. */
export const FIT_MARGIN = 40;

export function clampScale(scale: number): number {
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, scale));
}

/** Where section `index`'s header starts, on screen. */
export function headerTop(layout: CanvasLayout, view: View, index: number): number {
  return view.y + view.scale * layout.sections[index].rowsY + index * HEADER;
}

/** Where section `index`'s row starts, on screen. */
export function rowTop(layout: CanvasLayout, view: View, index: number): number {
  return headerTop(layout, view, index) + HEADER;
}

/** Where section `index`'s row sits on the scaled layer, in canvas pixels. */
export function rowLayerY(layout: CanvasLayout, scale: number, index: number): number {
  return layout.sections[index].rowsY + ((index + 1) * HEADER) / scale;
}

/** Screens near the viewport, grouped by their original section so headers keep their positions. */
export function visibleScreens(layout: CanvasLayout, view: View, area: Size | null, margin = 300): PlacedScreen[][] {
  return layout.sections.map((section, index) => {
    if (!area || area.width <= 0 || area.height <= 0) return [];
    const top = rowTop(layout, view, index);
    return section.screens.filter((screen) => {
      const left = view.x + view.scale * screen.x;
      return left < area.width + margin && left + view.scale * screen.width > -margin
        && top - LABEL_HEIGHT < area.height + margin && top + view.scale * screen.height > -margin;
    });
  });
}

/** The page's size on screen at `scale`. */
export function pageSize(layout: CanvasLayout, scale: number): Size {
  return { width: layout.width * scale, height: layout.rowsHeight * scale + layout.sections.length * HEADER };
}

/** The page fitted to the area, centred; never above 100%. */
export function fitView(layout: CanvasLayout, area: Size, margin = FIT_MARGIN): View {
  if (layout.sections.length === 0 || layout.width <= 0) return { scale: 1, x: margin, y: margin };
  const headers = layout.sections.length * HEADER;
  const byWidth = (area.width - 2 * margin) / layout.width;
  const byHeight = layout.rowsHeight > 0 ? (area.height - 2 * margin - headers) / layout.rowsHeight : Infinity;
  const scale = clampScale(Math.min(1, byWidth, byHeight));
  const size = pageSize(layout, scale);
  return { scale, x: (area.width - size.width) / 2, y: Math.max(margin / 2, (area.height - size.height) / 2) };
}

/**
 * The view at `scale` that keeps the canvas point under `at` (a point of the area) where it is.
 * Across the headers, which keep their size, the point is the one of the row (or the header) the
 * pointer is over.
 */
export function zoomAround(layout: CanvasLayout, view: View, scale: number, at: Point): View {
  const next = clampScale(scale);
  const ratio = next / view.scale;
  const x = at.x - (at.x - view.x) * ratio;
  if (layout.sections.length === 0) return { scale: next, x, y: at.y - (at.y - view.y) * ratio };
  // The section whose header starts at or above the pointer (the first one above the page).
  let index = 0;
  for (let i = layout.sections.length - 1; i >= 0; i--) {
    if (headerTop(layout, view, i) <= at.y) {
      index = i;
      break;
    }
  }
  const section = layout.sections[index];
  const top = headerTop(layout, view, index);
  if (at.y < top + HEADER) {
    // Over the header (or above the page): the header stays where it is.
    return { scale: next, x, y: at.y - next * section.rowsY - index * HEADER - (at.y - top) };
  }
  const rowsY = section.rowsY + (at.y - top - HEADER) / view.scale;
  return { scale: next, x, y: at.y - next * rowsY - (index + 1) * HEADER };
}

/** A zoom step (the toolbar's 50%, 100%, 200%, ⇧0) around the middle of the area. */
export function zoomCentred(layout: CanvasLayout, view: View, scale: number, area: Size): View {
  return zoomAround(layout, view, scale, { x: area.width / 2, y: area.height / 2 });
}

/** A pinch or a ⌘-wheel's zoom factor: a trackpad sends small deltas, a mouse wheel large ones. */
export function wheelZoom(deltaY: number, deltaMode = 0): number {
  const pixels = deltaMode === 1 ? deltaY * 16 : deltaY;
  return Math.exp(-Math.max(-50, Math.min(50, pixels)) * 0.01);
}

export function panBy(view: View, dx: number, dy: number): View {
  return { ...view, x: view.x + dx, y: view.y + dy };
}

/** The zoom menu: fit, or a step. */
export const ZOOM_STEPS = [0.5, 1, 2] as const;

export function percent(scale: number): string {
  return `${Math.round(scale * 100)}%`;
}

/** The dot grid's spacing behind the canvas: 24 canvas pixels, kept between 12 and 48 on screen. */
export function gridSpacing(scale: number): number {
  let size = 24 * scale;
  while (size < 12) size *= 2;
  while (size > 48) size /= 2;
  return size;
}

/** The scale that shows a screen whole in the area: its own size, or smaller. */
export function fitScale(screen: Size, area: Size): number {
  if (screen.width <= 0 || screen.height <= 0 || area.width <= 0 || area.height <= 0) return 1;
  return Math.min(1, area.width / screen.width, area.height / screen.height);
}

// --- Present ---

export type PresentStep = { section: string; screen: CanvasScreen };

/** Every screen of the page, sections then screens, in order. */
export function presentOrder(sections: ShownSection[]): PresentStep[] {
  return sections.flatMap((section) => section.screens.map((screen) => ({ section: section.title, screen })));
}

/** ← and → through the steps, stopping at either end. */
export function stepPresent(index: number, count: number, direction: -1 | 1): number {
  if (count === 0) return 0;
  return Math.min(count - 1, Math.max(0, index + direction));
}

// --- selection ---

/** A click on a screen selects it alone; a ⇧-click adds it, or takes it out. */
export function toggleSelection(selection: readonly string[], file: string, additive: boolean): string[] {
  if (!additive) return [file];
  return selection.includes(file) ? selection.filter((f) => f !== file) : [...selection, file];
}

// --- reloads ---

const WRITE_TOOLS = new Set(["Write", "Edit", "MultiEdit"]);

/** The files a tool call writes: Claude's Write and Edit name one, Codex's apply_patch lists its changes. */
function writtenPaths(name: string, input: JsonValue): string[] {
  if (!input || typeof input !== "object" || Array.isArray(input)) return [];
  if (WRITE_TOOLS.has(name)) return typeof input.file_path === "string" ? [input.file_path] : [];
  if (name !== "apply_patch" || !Array.isArray(input.changes)) return [];
  return input.changes.flatMap((change) =>
    change && typeof change === "object" && !Array.isArray(change) && typeof change.path === "string" ? [change.path] : [],
  );
}

/** Whether a tool call writes a file of the canvas: a screen under its folder, or the manifest. */
export function touchesCanvas(name: string, input: JsonValue, dir: string | null): boolean {
  return writtenPaths(name, input).some((path) => {
    if (path === "canvas.json" || path.endsWith("/canvas.json")) return true;
    // The folder is not known before the first answer: any write may be one.
    if (!dir) return true;
    const folder = dir.endsWith("/") ? dir.slice(0, -1) : dir;
    return path.startsWith(`${folder}/`) || !path.startsWith("/");
  });
}

/**
 * Changes whenever the canvas may have: a write of one of its files is called or done (the file
 * changes at its result), or a turn ends (a shell command may have written too).
 */
export function canvasRevision(items: readonly ChatItem[], turns: number, dir: string | null): string {
  let writes = 0;
  for (const item of items) {
    if (item.kind !== "tool" || !touchesCanvas(item.name, item.input, dir)) continue;
    writes += item.result ? 2 : 1;
  }
  return `${turns}:${writes}`;
}

/** Each screen's last change, by file. */
export function modifiedTimes(result: Pick<CanvasResult, "pages" | "unsorted">): Map<string, number> {
  const times = new Map<string, number>();
  const add = (screen: CanvasScreen) => {
    if (screen.modified_ms != null) times.set(screen.file, screen.modified_ms);
  };
  for (const page of result.pages) for (const section of page.sections) section.screens.forEach(add);
  result.unsorted.forEach(add);
  return times;
}

/** The screens that changed since `before`: an outline shows it. A screen new to the canvas is not one. */
export function changedScreens(before: Map<string, number> | null, after: Map<string, number>): string[] {
  if (!before) return [];
  const changed: string[] = [];
  for (const [file, time] of after) {
    const previous = before.get(file);
    if (previous != null && previous !== time) changed.push(file);
  }
  return changed;
}

// --- the tab's layout ---

/** The chat's width beside the canvas: about 380 px, never under 300. */
export const CHAT_WIDTH = 380;
export const MIN_CHAT = 300;
/** Below this canvas width, the tab shows one of the chat and the canvas at a time. */
export const MIN_CANVAS = 400;
export const DIVIDER = 5;
export const CHAT_WIDTH_KEY = "chauffeur.designChatWidth";

/** The width kept in localStorage, or the default. */
export function storedChatWidth(raw: string | null): number {
  const value = raw == null ? NaN : Number(raw);
  return Number.isFinite(value) && value >= MIN_CHAT ? Math.round(value) : CHAT_WIDTH;
}

/** The chat's width in a tab `total` wide; `narrow` when the canvas would have under `MIN_CANVAS`. */
export function designPanes(total: number, wanted: number): { narrow: boolean; chat: number } {
  const max = total - DIVIDER - MIN_CANVAS;
  if (max < MIN_CHAT) return { narrow: true, chat: total };
  return { narrow: false, chat: Math.min(max, Math.max(MIN_CHAT, Math.round(wanted))) };
}
