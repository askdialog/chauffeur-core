import { describe, expect, test } from "bun:test";
import type { CanvasResult, CanvasScreen } from "@chauffeur/protocol";
import type { ChatItem, ToolItem } from "./chat";
import {
  canvasRevision,
  CHAT_WIDTH,
  changedScreens,
  designPanes,
  fitScale,
  fitView,
  gridSpacing,
  HEADER,
  headerTop,
  layoutCanvas,
  MAX_SCALE,
  MIN_SCALE,
  modifiedTimes,
  noScreens,
  pageSections,
  pageSize,
  presentOrder,
  rowLayerY,
  rowTop,
  screenSrc,
  SCREEN_GAP,
  SECTION_GAP,
  stepPresent,
  storedChatWidth,
  toggleSelection,
  touchesCanvas,
  type View,
  visibleScreens,
  zoomAround,
  zoomCentred,
} from "./canvas";

const screen = (file: string, width = 390, height = 844, more: Partial<CanvasScreen> = {}): CanvasScreen => ({
  file,
  width,
  height,
  url: `http://127.0.0.1:5000/s/w/c/${file}`,
  modified_ms: 1,
  ...more,
});

const result = (more: Partial<CanvasResult> = {}): CanvasResult => ({
  chat: "c",
  dir: "/ws/.chauffeur/designs/c",
  pages: [
    {
      name: "Checkout",
      sections: [
        { title: "Cart", screens: [screen("cart.html"), screen("cart-empty.html")] },
        { title: "Payment", screens: [screen("pay.html", 1440, 900)] },
      ],
    },
    { name: "Account", sections: [{ title: "Profile", screens: [screen("profile.html")] }] },
  ],
  unsorted: [],
  ...more,
});

describe("the sections of a page", () => {
  test("a page shows its sections, then the unsorted files", () => {
    const sections = pageSections(result({ unsorted: [screen("draft.html")] }), 0);
    expect(sections.map((s) => s.title)).toEqual(["Cart", "Payment", "Unsorted"]);
    expect(sections.map((s) => s.unsorted)).toEqual([false, false, true]);
  });

  test("without a manifest, every file is in Unsorted; a page out of range is the last one", () => {
    expect(pageSections(result({ pages: [], unsorted: [screen("a.html")] }), 0).map((s) => s.title)).toEqual(["Unsorted"]);
    expect(pageSections(result(), 7).map((s) => s.title)).toEqual(["Profile"]);
  });

  test("a canvas has no screen when no page nor Unsorted has one", () => {
    expect(noScreens(result())).toBe(false);
    expect(noScreens({ pages: [{ name: "P", sections: [{ title: "S", screens: [] }] }], unsorted: [] })).toBe(true);
    expect(noScreens({ pages: [], unsorted: [] })).toBe(true);
  });

  test("a screen's source changes with its file, and a missing file has none", () => {
    expect(screenSrc({ url: "http://h/x/a.html", modified_ms: 42 })).toBe("http://h/x/a.html?v=42");
    expect(screenSrc({ url: "http://h/x/a.html?q=1", modified_ms: 42 })).toBe("http://h/x/a.html?q=1&v=42");
    expect(screenSrc({ url: "http://h/x/a.html" })).toBe("http://h/x/a.html");
    expect(screenSrc({})).toBeNull();
  });
});

describe("the layout", () => {
  const layout = layoutCanvas(pageSections(result(), 0));

  test("screens sit in a row at their own size, sections one under the other", () => {
    const [cart, payment] = layout.sections;
    expect(cart.screens.map((s) => s.x)).toEqual([0, 390 + SCREEN_GAP]);
    expect(cart.width).toBe(390 * 2 + SCREEN_GAP);
    expect(cart.height).toBe(844);
    expect(cart.rowsY).toBe(0);
    expect(payment.rowsY).toBe(844 + SECTION_GAP);
    expect(layout.width).toBe(1440);
    expect(layout.rowsHeight).toBe(844 + SECTION_GAP + 900);
  });

  test("headers keep their size on screen whatever the zoom", () => {
    const view: View = { scale: 0.5, x: 10, y: 20 };
    expect(headerTop(layout, view, 0)).toBe(20);
    expect(rowTop(layout, view, 0)).toBe(20 + HEADER);
    expect(headerTop(layout, view, 1)).toBe(20 + 0.5 * (844 + SECTION_GAP) + HEADER);
    // On the scaled layer, a row sits where it shows on screen.
    for (const index of [0, 1]) {
      expect(view.y + view.scale * rowLayerY(layout, view.scale, index)).toBeCloseTo(rowTop(layout, view, index));
    }
    expect(pageSize(layout, 0.5)).toEqual({ width: 720, height: 0.5 * layout.rowsHeight + 2 * HEADER });
  });
});

describe("the zoom", () => {
  const layout = layoutCanvas(pageSections(result(), 0));

  test("fit shows the whole page in the area, centred, never above 100%", () => {
    const area = { width: 1000, height: 800 };
    const view = fitView(layout, area);
    const size = pageSize(layout, view.scale);
    expect(size.width).toBeLessThanOrEqual(area.width - 80 + 1e-9);
    expect(size.height).toBeLessThanOrEqual(area.height - 80 + 1e-9);
    expect(view.x).toBeCloseTo((area.width - size.width) / 2);
    expect(view.y).toBeCloseTo((area.height - size.height) / 2);
    const small = layoutCanvas([{ title: "S", screens: [screen("a.html", 200, 100)], unsorted: false }]);
    expect(fitView(small, { width: 2000, height: 2000 }).scale).toBe(1);
  });

  test("zooming keeps the point under the pointer in place, in a row", () => {
    const view: View = { scale: 0.4, x: 30, y: 15 };
    // A point of the payment row.
    const at = { x: 300, y: rowTop(layout, view, 1) + 120 };
    const canvasPoint = { x: (at.x - view.x) / view.scale, y: layout.sections[1].rowsY + 120 / view.scale };
    const next = zoomAround(layout, view, 1.3, at);
    expect(next.scale).toBe(1.3);
    expect(next.x + next.scale * canvasPoint.x).toBeCloseTo(at.x);
    expect(rowTop(layout, next, 1) + next.scale * (canvasPoint.y - layout.sections[1].rowsY)).toBeCloseTo(at.y);
  });

  test("zooming over a header keeps that header where it is", () => {
    const view: View = { scale: 0.4, x: 0, y: 15 };
    const at = { x: 100, y: headerTop(layout, view, 1) + 10 };
    const next = zoomAround(layout, view, 0.8, at);
    expect(headerTop(layout, next, 1)).toBeCloseTo(headerTop(layout, view, 1));
  });

  test("the zoom stays within its bounds, and a step zooms around the middle", () => {
    const view: View = { scale: 1, x: 0, y: 0 };
    expect(zoomAround(layout, view, 100, { x: 0, y: 0 }).scale).toBe(MAX_SCALE);
    expect(zoomAround(layout, view, 0, { x: 0, y: 0 }).scale).toBe(MIN_SCALE);
    const area = { width: 800, height: 600 };
    const centred = zoomCentred(layout, view, 2, area);
    // The canvas point at the middle stays there.
    expect(centred.x + 2 * ((400 - view.x) / 1)).toBeCloseTo(400);
  });

  test("the dot grid keeps a readable spacing", () => {
    expect(gridSpacing(1)).toBe(24);
    expect(gridSpacing(0.1)).toBeGreaterThanOrEqual(12);
    expect(gridSpacing(4)).toBeLessThanOrEqual(48);
  });

  test("a screen presented is shrunk to fit, never enlarged", () => {
    expect(fitScale({ width: 390, height: 844 }, { width: 1200, height: 800 })).toBeCloseTo(800 / 844);
    expect(fitScale({ width: 390, height: 600 }, { width: 1200, height: 800 })).toBe(1);
  });
});

describe("Present", () => {
  test("walks every screen of the page, sections then screens, stopping at the ends", () => {
    const steps = presentOrder(pageSections(result({ unsorted: [screen("draft.html")] }), 0));
    expect(steps.map((s) => `${s.section}/${s.screen.file}`)).toEqual([
      "Cart/cart.html",
      "Cart/cart-empty.html",
      "Payment/pay.html",
      "Unsorted/draft.html",
    ]);
    expect(stepPresent(0, 4, -1)).toBe(0);
    expect(stepPresent(2, 4, 1)).toBe(3);
    expect(stepPresent(3, 4, 1)).toBe(3);
  });
});

describe("the selection", () => {
  test("a click selects one screen, a ⇧-click adds or takes one out", () => {
    expect(toggleSelection(["a", "b"], "c", false)).toEqual(["c"]);
    expect(toggleSelection(["a"], "b", true)).toEqual(["a", "b"]);
    expect(toggleSelection(["a", "b"], "a", true)).toEqual(["b"]);
  });
});

describe("reloads", () => {
  const dir = "/ws/.chauffeur/designs/c";
  const tool = (name: string, path: unknown, done = false): ToolItem => ({
    kind: "tool",
    key: `${name}-${String(path)}-${done}`,
    ts: "",
    depth: 0,
    toolUseId: "t",
    name,
    input: { file_path: path as string },
    parentToolUseId: null,
    summary: "",
    result: done ? { output: "", isError: false, attachments: [] } : null,
  });

  test("a write of a screen or of the manifest changes the canvas; other tools and files do not", () => {
    expect(touchesCanvas("Write", { file_path: `${dir}/cart.html` }, dir)).toBe(true);
    expect(touchesCanvas("Edit", { file_path: `${dir}/css/app.css` }, `${dir}/`)).toBe(true);
    expect(touchesCanvas("MultiEdit", { file_path: "/elsewhere/canvas.json" }, dir)).toBe(true);
    expect(touchesCanvas("Write", { file_path: "cart.html" }, dir)).toBe(true);
    expect(touchesCanvas("Write", { file_path: "/ws/shop-web/src/App.tsx" }, dir)).toBe(false);
    expect(touchesCanvas("Write", { file_path: `${dir}-other/a.html` }, dir)).toBe(false);
    expect(touchesCanvas("Read", { file_path: `${dir}/cart.html` }, dir)).toBe(false);
    expect(touchesCanvas("Bash", { command: "ls" }, dir)).toBe(false);
    expect(touchesCanvas("Write", null, dir)).toBe(false);
    // Before the folder is known, any write may be one.
    expect(touchesCanvas("Write", { file_path: "/anywhere/a.html" }, null)).toBe(true);
  });

  test("a Codex patch changes the canvas when one of its files is a screen or the manifest", () => {
    const patch = (...paths: string[]) => ({ changes: paths.map((path) => ({ path, kind: { type: "add" }, diff: "" })) });
    expect(touchesCanvas("apply_patch", patch("/ws/shop-web/src/App.tsx", `${dir}/cart.html`), dir)).toBe(true);
    expect(touchesCanvas("apply_patch", patch(`${dir}/canvas.json`), dir)).toBe(true);
    expect(touchesCanvas("apply_patch", patch("/ws/shop-web/src/App.tsx"), dir)).toBe(false);
    expect(touchesCanvas("apply_patch", { changes: "x" }, dir)).toBe(false);
  });

  test("the revision moves at a write called, at its result, and at the end of a turn", () => {
    const items: ChatItem[] = [tool("Read", `${dir}/a.html`, true), tool("Write", "/ws/repo/x.ts", true)];
    const start = canvasRevision(items, 0, dir);
    const called = canvasRevision([...items, tool("Write", `${dir}/a.html`)], 0, dir);
    const done = canvasRevision([...items, tool("Write", `${dir}/a.html`, true)], 0, dir);
    const ended = canvasRevision([...items, tool("Write", `${dir}/a.html`, true)], 1, dir);
    expect(new Set([start, called, done, ended]).size).toBe(4);
    expect(canvasRevision([...items, tool("Write", "/ws/repo/y.ts", true)], 0, dir)).toBe(start);
  });

  test("a screen whose file changed gets an outline; a new one does not", () => {
    const before = modifiedTimes(result());
    const after = modifiedTimes(
      result({
        pages: [{ name: "P", sections: [{ title: "S", screens: [screen("cart.html", 390, 844, { modified_ms: 5 }), screen("pay.html")] }] }],
        unsorted: [screen("new.html", 390, 844, { modified_ms: 9 })],
      }),
    );
    expect(changedScreens(before, after)).toEqual(["cart.html"]);
    expect(changedScreens(null, after)).toEqual([]);
  });
});

describe("the tab's layout", () => {
  test("the chat keeps its width, at least 300 px, and leaves the canvas at least 400", () => {
    expect(designPanes(1400, 380)).toEqual({ narrow: false, chat: 380 });
    expect(designPanes(1400, 100)).toEqual({ narrow: false, chat: 300 });
    expect(designPanes(1000, 900)).toEqual({ narrow: false, chat: 1000 - 5 - 400 });
  });

  test("too narrow for both, the tab shows one at a time", () => {
    expect(designPanes(704, 380).narrow).toBe(true);
    expect(designPanes(705, 380)).toEqual({ narrow: false, chat: 300 });
  });

  test("the width kept is read back, or the default", () => {
    expect(storedChatWidth("512")).toBe(512);
    expect(storedChatWidth(null)).toBe(CHAT_WIDTH);
    expect(storedChatWidth("abc")).toBe(CHAT_WIDTH);
    expect(storedChatWidth("120")).toBe(CHAT_WIDTH);
  });
});

describe("the screens mounted near the viewport", () => {
  const layout = layoutCanvas([
    { title: "Long row", unsorted: false, screens: Array.from({ length: 100 }, (_, i) => screen(`${i}.html`)) },
    { title: "Below", unsorted: false, screens: [screen("below.html")] },
  ]);
  const area = { width: 800, height: 600 };
  test("a hundred-screen page mounts only the nearby screens", () => {
    const shown = visibleScreens(layout, { scale: 1, x: 0, y: 0 }, area);
    expect(shown.map((row) => row.map((s) => s.screen.file))).toEqual([["0.html", "1.html", "2.html"], []]);
  });
  test("panning brings new screens into view and releases distant ones", () => {
    const shown = visibleScreens(layout, { scale: 1, x: -4300, y: 0 }, area);
    expect(shown[0].map((s) => s.screen.file)).toEqual(["9.html", "10.html", "11.html", "12.html"]);
  });
  test("zoom and fixed-height headers preserve each row's position", () => {
    const view = { scale: 0.1, x: 0, y: 0 };
    const shown = visibleScreens(layout, view, { width: 100, height: HEADER }, 0);
    expect(shown.map((row) => row.length)).toEqual([3, 0]); // labels above the first row are visible
    const lower = visibleScreens(layout, { ...view, y: -rowTop(layout, view, 1) }, area, 0);
    expect(lower[0]).toEqual([]);
    expect(lower[1].map((s) => s.screen.file)).toEqual(["below.html"]);
  });
  test("waits for the viewport to be measured before loading any frames", () => {
    expect(visibleScreens(layout, { scale: 1, x: 0, y: 0 }, null)).toEqual([[], []]);
    expect(visibleScreens(layout, { scale: 1, x: 0, y: 0 }, { width: 0, height: 0 })).toEqual([[], []]);
  });
});

test("screen occurrences keep distinct identities even when their files and dimensions match", () => {
  const section = { title: "Repeated", unsorted: false, screens: [screen("same.html"), screen("same.html"), screen("same.html", 390, 1120)] };
  const ids = layoutCanvas([section]).sections[0].screens.map((s) => s.id);
  expect(new Set(ids).size).toBe(3);
  const reordered = layoutCanvas([{ ...section, screens: [screen("unrelated.html"), ...section.screens] }]);
  expect(reordered.sections[0].screens.slice(1).map((s) => s.id)).toEqual(ids);
});
