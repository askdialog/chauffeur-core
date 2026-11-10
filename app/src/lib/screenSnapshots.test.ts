import { describe, expect, test } from "bun:test";
import type { CanvasScreen } from "@chauffeur/protocol";
import { HEADER, layoutCanvas, screenSrc, type View } from "./canvas";
import {
  CACHE_PIXELS,
  currentPicture,
  KEPT,
  keepSnapshots,
  MAX_PIXELS,
  picturesToTake,
  pruneSnapshots,
  type Snapshot,
  satisfies,
  shownSnapshot,
  snapshotKey,
  visiblePart,
  wantedPicture,
  wholeDensity,
} from "./screenSnapshots";

const screen = (file: string, width = 390, height = 844, more: Partial<CanvasScreen> = {}): CanvasScreen => ({
  file,
  width,
  height,
  url: `http://127.0.0.1:5000/s/w/c/${file}`,
  modified_ms: 1,
  ...more,
});

const shot = (more: Partial<Snapshot> = {}): Snapshot => ({
  src: "a",
  rect: { x: 0, y: 0, width: 390, height: 844 },
  density: 1,
  whole: true,
  url: "blob:1",
  ...more,
});

const AREA = { width: 1600, height: 1000 };
const visible = { x: 0, y: 0, width: 390, height: 844 };

describe("the density a whole screen is pictured at", () => {
  test("is the power of two at or above what the zoom shows on the display", () => {
    expect(wholeDensity(0.6, 1)).toBe(1);
    expect(wholeDensity(1, 1)).toBe(1);
    expect(wholeDensity(1, 2)).toBe(2);
    expect(wholeDensity(0.3, 2)).toBe(1);
    expect(wholeDensity(0.2, 1)).toBe(0.25);
    expect(wholeDensity(1.2, 2)).toBe(4);
  });
});

describe("the part of a screen on screen", () => {
  test("is the whole screen when it is all in the area", () => {
    expect(visiblePart({ width: 390, height: 844 }, 100, 50, 0.5, AREA)).toEqual(visible);
  });
  test("is cut by the area's edges, in the screen's pixels", () => {
    expect(visiblePart({ width: 1440, height: 900 }, -200, -100, 2, AREA)).toEqual({ x: 100, y: 50, width: 800, height: 500 });
  });
  test("is none for a screen beside the area", () => {
    expect(visiblePart({ width: 390, height: 844 }, 1700, 0, 1, AREA)).toBeNull();
    expect(visiblePart({ width: 390, height: 844 }, -400, 0, 1, AREA)).toBeNull();
  });
});

describe("the picture wanted for a screen", () => {
  test("is the whole screen at its power-of-two density while it fits", () => {
    expect(wantedPicture({ width: 1440, height: 900 }, { x: 0, y: 0, width: 1440, height: 900 }, 0.6, 1)).toEqual({
      rect: { x: 0, y: 0, width: 1440, height: 900 },
      density: 1,
      whole: true,
    });
  });
  test("is the part shown and some around it, at the zoom's own density, for a large screen at a large zoom", () => {
    const part = { x: 400, y: 300, width: 400, height: 250 };
    const wanted = wantedPicture({ width: 1440, height: 900 }, part, 2, 2);
    expect(wanted.whole).toBe(false);
    expect(wanted.density).toBe(4);
    expect(wanted.rect).toEqual({ x: 200, y: 175, width: 800, height: 500 });
  });
  test("is only the part shown when the margin would make it too large", () => {
    const part = { x: 100, y: 100, width: 800, height: 500 };
    const wanted = wantedPicture({ width: 1440, height: 900 }, part, 2, 2);
    expect(wanted.rect).toEqual(part);
    expect(wanted.rect.width * wanted.rect.height * wanted.density ** 2).toBeLessThanOrEqual(MAX_PIXELS);
  });
});

describe("whether a picture is the one wanted", () => {
  const whole = { density: 1, whole: true };
  test("needs the screen as it is now and the density wanted", () => {
    expect(satisfies(shot(), "a", whole, visible)).toBe(true);
    expect(satisfies(shot({ src: "old" }), "a", whole, visible)).toBe(false);
    expect(satisfies(shot({ density: 2 }), "a", whole, visible)).toBe(false);
  });
  test("of a part, needs it to cover what is shown", () => {
    const part = shot({ whole: false, density: 6, rect: { x: 200, y: 175, width: 800, height: 500 } });
    expect(satisfies(part, "a", { density: 6, whole: false }, { x: 400, y: 300, width: 400, height: 250 })).toBe(true);
    expect(satisfies(part, "a", { density: 6, whole: false }, { x: 900, y: 300, width: 400, height: 250 })).toBe(false);
  });
});

describe("the picture laid over a screen", () => {
  test("is the least dense one dense enough", () => {
    const pictures = [shot({ density: 4, url: "blob:4" }), shot({ density: 1, url: "blob:1" }), shot({ density: 0.5, url: "blob:half" })];
    expect(shownSnapshot(pictures, "a", visible, 0.6, 1)?.url).toBe("blob:1");
    expect(shownSnapshot(pictures, "a", visible, 2, 1)?.url).toBe("blob:4");
  });
  test("may be a little stretched while the better one comes, never much", () => {
    expect(shownSnapshot([shot({ density: 1 })], "a", visible, 1.2, 1)).not.toBeNull();
    expect(shownSnapshot([shot({ density: 1 })], "a", visible, 2, 1)).toBeNull();
  });
  test("is never one of an older version of the screen", () => {
    expect(shownSnapshot([shot({ src: "old" })], "a", visible, 1, 1)).toBeNull();
  });
  test("prefers a picture covering the part shown to one beside it", () => {
    const left = shot({ whole: false, density: 4, rect: { x: 0, y: 0, width: 600, height: 900 }, url: "blob:left" });
    const middle = shot({ whole: false, density: 4, rect: { x: 300, y: 0, width: 800, height: 900 }, url: "blob:middle" });
    expect(shownSnapshot([left, middle], "a", { x: 400, y: 100, width: 500, height: 400 }, 4, 1)?.url).toBe("blob:middle");
  });
});

describe("the pictures to take", () => {
  const layout = layoutCanvas([{ title: "Cart", unsorted: false, screens: [screen("a.html"), screen("b.html"), screen("c.html")] }]);
  const view: View = { scale: 0.5, x: 0, y: 0 };
  const none = () => [];

  test("are of the screens shown, the nearest the middle of the area first", () => {
    // At 50%, the screens start at 0, 215 and 430 on screen: the third is beside the area.
    const area = { width: 420, height: 600 };
    const requests = picturesToTake(layout, view, area, 2, none, new Set());
    expect(requests.map((r) => r.file)).toEqual(["b.html", "a.html"]);
    expect(requests[0]).toMatchObject({ width: 390, height: 844, density: 1, whole: true, pixels: 390, rect: { x: 0, y: 0, width: 390, height: 844 } });
    expect(requests[0].src).toBe(screenSrc(screen("b.html")) ?? "");
  });
  test("leave out a screen that has the picture wanted, and one that failed", () => {
    const src = screenSrc(screen("a.html")) ?? "";
    const have = (id: string) => (id === layout.sections[0].screens[0].id ? [shot({ src })] : []);
    const failed = new Set([snapshotKey({ id: layout.sections[0].screens[1].id, src: screenSrc(screen("b.html")) ?? "", rect: { x: 0, y: 0, width: 390, height: 844 }, density: 1 })]);
    expect(picturesToTake(layout, view, AREA, 2, have, failed).map((r) => r.file)).toEqual(["c.html"]);
  });
  test("leave out a screen without a file served", () => {
    const missing = layoutCanvas([{ title: "Cart", unsorted: false, screens: [screen("gone.html", 390, 844, { url: undefined })] }]);
    expect(picturesToTake(missing, view, AREA, 1, none, new Set())).toEqual([]);
  });
  test("count the section's header above its screens", () => {
    // The row starts HEADER screen pixels down: an area that shows only the header shows no screen.
    expect(picturesToTake(layout, { scale: 1, x: 0, y: 0 }, { width: 1600, height: HEADER - 1 }, 1, none, new Set())).toEqual([]);
    expect(picturesToTake(layout, { scale: 1, x: 0, y: 0 }, { width: 1600, height: HEADER + 10 }, 1, none, new Set())).toHaveLength(3);
  });
});

describe("the pictures a screen keeps", () => {
  test("are the newest of its current version, at most KEPT", () => {
    let pictures: Snapshot[] = [];
    const dropped: Snapshot[] = [];
    for (let i = 0; i < KEPT + 1; i++) {
      const result = keepSnapshots(pictures, shot({ url: `blob:${i}`, density: 2 ** i }));
      pictures = result.kept;
      dropped.push(...result.dropped);
    }
    expect(pictures.map((p) => p.url)).toEqual(["blob:3", "blob:2", "blob:1"]);
    expect(dropped.map((p) => p.url)).toEqual(["blob:0"]);
  });
  test("drop every picture of an older version", () => {
    const { kept, dropped } = keepSnapshots([shot({ src: "old", url: "blob:old" })], shot({ src: "new", url: "blob:new" }));
    expect(kept.map((p) => p.url)).toEqual(["blob:new"]);
    expect(dropped.map((p) => p.url)).toEqual(["blob:old"]);
  });
});

describe("the canvas picture cache", () => {
  const layout = layoutCanvas([{ title: "Screens", unsorted: false, screens: [screen("a.html"), screen("b.html")] }]);
  const view = { scale: 1, x: 0, y: 0 };
  const area = { width: 390, height: 900 };
  const [aId, bId] = layout.sections[0].screens.map((s) => s.id);
  const a = shot({ src: screenSrc(screen("a.html"))! });
  const b = shot({ src: screenSrc(screen("b.html"))!, url: "blob:b" });

  test("releases pictures from other pages and previous versions even without taking another picture", () => {
    const old = shot({ src: "old", url: "blob:old" });
    const gone = shot({ url: "blob:gone" });
    const cache = new Map([[aId, [a, old]], ["gone.html", [gone]]]);
    expect(pruneSnapshots(cache, layout, view, area)).toEqual([old, gone]);
    expect([...cache]).toEqual([[aId, [a]]]);
  });

  test("evicts offscreen pictures over the pixel budget and keeps visible ones", () => {
    const large = { ...b, rect: { x: 0, y: 0, width: CACHE_PIXELS, height: 1 } };
    const cache = new Map([[aId, [a]], [bId, [large]]]);
    expect(pruneSnapshots(cache, layout, view, area)).toEqual([large]);
    expect([...cache.keys()]).toEqual([aId]);
  });

  test("evicts the offscreen screen seen longest ago, not the one captured first", () => {
    const wide = layoutCanvas([{ title: "Screens", unsorted: false, screens: [screen("a.html"), screen("b.html"), screen("c.html")] }]);
    const [first, second, third] = wide.sections[0].screens.map((s) => s.id);
    // Three pictures of a third of the budget each: dropping one is enough to fit.
    const oneThird = { x: 0, y: 0, width: CACHE_PIXELS / 3 + 1, height: 1 };
    const big = (id: string) => ({ ...shot({ src: screenSrc(screen(id))!, url: `blob:${id}` }), rect: oneThird });
    const cache = new Map([[first, [big("a.html")]], [second, [big("b.html")]], [third, [big("c.html")]]]);
    // `a.html` was captured first but is the one in view; `b.html` was captured next and is the least recently seen.
    const dropped = pruneSnapshots(cache, wide, view, { width: 390, height: 900 });
    expect(dropped.map((p) => p.url)).toEqual(["blob:b.html"]);
    expect([...cache.keys()]).toEqual([third, first]);
  });

  test("keeps offscreen pictures under budget so returning to a screen reuses them", () => {
    const cache = new Map([[aId, [a]], [bId, [b]]]);
    expect(pruneSnapshots(cache, layout, view, area)).toEqual([]);
    expect(cache.size).toBe(2);
  });

  test("never evicts visible pictures into an endless recapture loop", () => {
    const large = { ...a, rect: { x: 0, y: 0, width: CACHE_PIXELS + 1, height: 1 } };
    const cache = new Map([[aId, [large]]]);
    expect(pruneSnapshots(cache, layout, view, area)).toEqual([]);
    expect(cache.get(aId)).toEqual([large]);
  });

  test("rejects late captures after a page, source or dimension change", () => {
    const [request] = picturesToTake(layout, view, area, 1, () => [], new Set());
    expect(currentPicture(request, layout)).toBe(true);
    expect(currentPicture(request, layoutCanvas([]))).toBe(false);
    expect(currentPicture({ ...request, src: "old" }, layout)).toBe(false);
    expect(currentPicture({ ...request, width: 1440 }, layout)).toBe(false);
  });
});

test("two occurrences of a file at different sizes receive separate pictures", () => {
  const layout = layoutCanvas([{ title: "Repeated", unsorted: false, screens: [screen("oden.html"), screen("oden.html", 390, 1120)] }]);
  const view = { scale: 0.44, x: 0, y: 0 };
  const first = layout.sections[0].screens[0];
  const have = (id: string) => id === first.id ? [shot({ src: screenSrc(first.screen) ?? "" })] : [];
  const requests = picturesToTake(layout, view, AREA, 2, have, new Set());
  expect(requests).toHaveLength(1);
  expect(requests[0].height).toBe(1120);
  expect(requests[0].rect.height).toBe(1120);
});
