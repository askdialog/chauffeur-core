// Sharp pictures of a design chat's screens on the app's canvas (story 47), kept pure so that it
// is tested in screenSnapshots.test.ts: what to picture for a view, which picture shows, which to
// keep.
//
// Under the canvas's zoom, WebKit draws a framed screen whose page holds a composited layer at
// 100% and lets the GPU shrink that bitmap, which blurs it. On macOS the app pictures the screens
// natively instead and lays the pictures over the live frames. A whole screen is pictured at a
// density (device pixels per screen pixel) among the powers of two, so that one picture serves
// every zoom it is at most twice as dense as (WebKit shrinks it sharply); a screen too large for
// that at the zoom shown is pictured at the zoom's own density, only around the part on screen.

import { type CanvasLayout, rowTop, screenSrc, type Size, type View } from "./canvas";

/** A part of a screen, in its pixels. */
export type Rect = { x: number; y: number; width: number; height: number };
/** A picture of `rect` of the screen at `src`, `density` device pixels per screen pixel; `url` shows it. */
export type Snapshot = { src: string; rect: Rect; density: number; whole: boolean; url: string };
export type SnapshotRequest = {
  id: string;
  file: string;
  src: string;
  /** The screen's size, which the page is laid out at. */
  width: number;
  height: number;
  rect: Rect;
  density: number;
  whole: boolean;
  /** The picture's width in device pixels. */
  pixels: number;
};

/** The largest picture of a whole screen; past it, the part on screen is pictured. */
export const MAX_PIXELS = 12_000_000;
const MIN_DENSITY = 1 / 32;
/** A picture shows while the view needs at most this much more density (it is stretched). */
const STRETCH = 4 / 3;
/** How many pictures a screen keeps (Fit and 100% both stay sharp when the user goes back). */
export const KEPT = 3;
/** Decoded pixels retained across screens; pictures currently visible take priority. */
export const CACHE_PIXELS = 48_000_000;

/** Whether an in-flight picture still belongs to the page and screen version being shown. */
export function currentPicture(request: SnapshotRequest, layout: CanvasLayout): boolean {
  return layout.sections.some((section) => section.screens.some((placed) =>
    placed.id === request.id && screenSrc(placed.screen) === request.src
      && placed.width === request.width && placed.height === request.height,
  ));
}

/** Removes obsolete pictures and, over budget, the least recently seen offscreen screens; returns URLs to release. */
export function pruneSnapshots(pictures: Map<string, Snapshot[]>, layout: CanvasLayout, view: View, area: Size | null): Snapshot[] {
  const sources = new Map<string, string | null>();
  const visible = new Set<string>();
  layout.sections.forEach((section, index) => {
    const top = rowTop(layout, view, index);
    for (const placed of section.screens) {
      sources.set(placed.id, screenSrc(placed.screen));
      if (area && visiblePart(placed, view.x + view.scale * placed.x, top, view.scale, area)) visible.add(placed.id);
    }
  });
  const dropped: Snapshot[] = [];
  const pixels = (list: Snapshot[]) => list.reduce((sum, s) => sum + s.rect.width * s.rect.height * s.density ** 2, 0);
  let total = 0;
  for (const [id, list] of pictures) {
    const kept = list.filter((s) => s.src === sources.get(id));
    dropped.push(...list.filter((s) => !kept.includes(s)));
    if (kept.length) pictures.set(id, kept);
    else pictures.delete(id);
    total += pixels(kept);
  }
  // The map's order is the eviction order: screens in view move to the end, so the oldest are the least recently seen.
  for (const id of visible) {
    const list = pictures.get(id);
    if (!list) continue;
    pictures.delete(id);
    pictures.set(id, list);
  }
  for (const [id, list] of pictures) {
    if (total <= CACHE_PIXELS) break;
    // Keeping visible pictures avoids a capture/eviction loop on a densely fitted page.
    if (visible.has(id)) continue;
    total -= pixels(list);
    dropped.push(...list);
    pictures.delete(id);
  }
  return dropped;
}

/** The density a whole screen is pictured at for `scale` on a display of `ratio`: the power of two at or above. */
export function wholeDensity(scale: number, ratio: number): number {
  return Math.max(MIN_DENSITY, 2 ** Math.ceil(Math.log2(scale * ratio) - 1e-9));
}

/** The part of a screen on screen, in its pixels, its corner at `left`, `top`; null when none is. */
export function visiblePart(size: Size, left: number, top: number, scale: number, area: Size): Rect | null {
  const x0 = Math.max(0, -left / scale);
  const y0 = Math.max(0, -top / scale);
  const x1 = Math.min(size.width, (area.width - left) / scale);
  const y1 = Math.min(size.height, (area.height - top) / scale);
  if (x1 <= x0 || y1 <= y0) return null;
  return { x: x0, y: y0, width: x1 - x0, height: y1 - y0 };
}

/**
 * The picture a screen should have for the view: whole at its power-of-two density when that fits
 * in `MAX_PIXELS`, else the part shown and half as much around it (in whole screen pixels, less
 * when that is too large) at the zoom's density.
 */
export function wantedPicture(size: Size, visible: Rect, scale: number, ratio: number): { rect: Rect; density: number; whole: boolean } {
  const whole = wholeDensity(scale, ratio);
  if (size.width * size.height * whole * whole <= MAX_PIXELS) {
    return { rect: { x: 0, y: 0, width: size.width, height: size.height }, density: whole, whole: true };
  }
  const density = scale * ratio;
  const around = (margin: number): Rect => {
    const x0 = Math.floor(Math.max(0, visible.x - visible.width * margin));
    const y0 = Math.floor(Math.max(0, visible.y - visible.height * margin));
    const x1 = Math.ceil(Math.min(size.width, visible.x + visible.width * (1 + margin)));
    const y1 = Math.ceil(Math.min(size.height, visible.y + visible.height * (1 + margin)));
    return { x: x0, y: y0, width: x1 - x0, height: y1 - y0 };
  };
  const wide = around(0.5);
  const rect = wide.width * wide.height * density * density <= MAX_PIXELS ? wide : around(0);
  return { rect, density, whole: false };
}

function contains(outer: Rect, inner: Rect): boolean {
  const e = 0.5;
  return outer.x <= inner.x + e && outer.y <= inner.y + e && outer.x + outer.width >= inner.x + inner.width - e && outer.y + outer.height >= inner.y + inner.height - e;
}

function overlaps(a: Rect, b: Rect): boolean {
  return a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
}

/** Whether `snapshot` is the picture wanted: of the screen as it is, at the density wanted, around the part shown. */
export function satisfies(snapshot: Snapshot, src: string, wanted: { density: number; whole: boolean }, visible: Rect): boolean {
  if (snapshot.src !== src) return false;
  if (wanted.whole) return snapshot.whole && snapshot.density === wanted.density;
  return !snapshot.whole && Math.abs(snapshot.density / wanted.density - 1) < 0.01 && contains(snapshot.rect, visible);
}

/**
 * The picture to lay over a screen now, while the view moves too: of the screen as it is, over
 * the part shown, and dense enough not to look softer than the live frame under it (stretched by
 * at most `STRETCH`). The one covering the part shown first, then the least dense of those dense
 * enough, else the densest. Null to show the live frame.
 */
export function shownSnapshot(snapshots: readonly Snapshot[], src: string, visible: Rect, scale: number, ratio: number): Snapshot | null {
  const need = scale * ratio;
  const usable = snapshots.filter((s) => s.src === src && overlaps(s.rect, visible) && s.density * STRETCH >= need);
  if (usable.length === 0) return null;
  const rank = (s: Snapshot) => [contains(s.rect, visible) ? 0 : 1, s.density >= need ? s.density : 1e6 - s.density];
  return usable.reduce((best, s) => {
    const [a, b] = rank(s);
    const [c, d] = rank(best);
    return a < c || (a === c && b < d) ? s : best;
  });
}

/** The pictures to take for the view, the screen nearest the middle of the area first; none failed already. */
export function picturesToTake(
  layout: CanvasLayout,
  view: View,
  area: Size,
  ratio: number,
  have: (id: string) => readonly Snapshot[],
  failed: ReadonlySet<string>,
): SnapshotRequest[] {
  const requests: Array<SnapshotRequest & { distance: number }> = [];
  layout.sections.forEach((section, index) => {
    const top = rowTop(layout, view, index);
    for (const placed of section.screens) {
      const src = screenSrc(placed.screen);
      if (!src) continue;
      const size = { width: placed.width, height: placed.height };
      const left = view.x + view.scale * placed.x;
      const visible = visiblePart(size, left, top, view.scale, area);
      if (!visible) continue;
      const wanted = wantedPicture(size, visible, view.scale, ratio);
      if (have(placed.id).some((s) => satisfies(s, src, wanted, visible))) continue;
      const request = {
        id: placed.id,
        file: placed.screen.file,
        src,
        width: placed.width,
        height: placed.height,
        ...wanted,
        pixels: Math.max(1, Math.round(wanted.rect.width * wanted.density)),
      };
      if (failed.has(snapshotKey(request))) continue;
      const middle = { x: left + view.scale * (visible.x + visible.width / 2), y: top + view.scale * (visible.y + visible.height / 2) };
      requests.push({ ...request, distance: Math.hypot(middle.x - area.width / 2, middle.y - area.height / 2) });
    }
  });
  return requests.sort((a, b) => a.distance - b.distance).map(({ distance: _, ...request }) => request);
}

/** Names a picture, to remember one that failed. */
export function snapshotKey(request: Pick<SnapshotRequest, "id" | "src" | "rect" | "density">): string {
  const { x, y, width, height } = request.rect;
  return `${request.id} ${request.src} ${request.density} ${x},${y},${width},${height}`;
}

/** A screen's pictures with `added`: those of another version of it go, then the oldest past `KEPT`. */
export function keepSnapshots(snapshots: readonly Snapshot[], added: Snapshot): { kept: Snapshot[]; dropped: Snapshot[] } {
  const current = snapshots.filter((s) => s.src === added.src);
  const kept = [added, ...current].slice(0, KEPT);
  return { kept, dropped: snapshots.filter((s) => !kept.includes(s)) };
}
