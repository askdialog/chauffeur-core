import { describe, expect, test } from "bun:test";
import {
  closeSide,
  DEFAULT_LAYOUT,
  defaultMain,
  DIVIDER,
  dropZone,
  LAYOUTS_KEY,
  MIN_PANE,
  moveToMain,
  openSide,
  panes,
  parseLayouts,
  serializeLayouts,
  shareAt,
  showView,
  SIDE_HEADER,
  sidePixels,
  splitBoxes,
  type SplitLayout,
  toggleSide,
  tooSmall,
} from "./splitLayout";

const withSide = (side: SplitLayout["side"], position: SplitLayout["position"] = "right"): SplitLayout => ({
  ...DEFAULT_LAYOUT,
  side,
  last: side ?? DEFAULT_LAYOUT.last,
  position,
});

describe("which view goes where", () => {
  test("⌘\\ opens the terminal on the right first, and closes it again", () => {
    const opened = toggleSide(DEFAULT_LAYOUT, "chat");
    expect(opened.layout.side).toBe("terminal");
    expect(opened.layout.position).toBe("right");
    expect(opened.main).toBe("chat");
    const closed = toggleSide(opened.layout, opened.main);
    expect(closed.layout.side).toBeNull();
    expect(closed.layout.last).toBe("terminal");
    expect(closed.main).toBe("chat");
  });

  test("⌘\\ opens the last side view where it was", () => {
    const diffBelow = closeSide(withSide("diff", "bottom"));
    const opened = toggleSide(diffBelow, "chat");
    expect(opened.layout.side).toBe("diff");
    expect(opened.layout.position).toBe("bottom");
  });

  test("the view shown in the main pane leaves it for the side: the main pane shows another", () => {
    const moved = openSide(DEFAULT_LAYOUT, "terminal", "terminal", "right");
    expect(moved.layout.side).toBe("terminal");
    expect(moved.main).toBe("chat");
    // The chat going to the side swaps with the side's view.
    const swapped = openSide(withSide("terminal"), "chat", "chat", "right");
    expect(swapped).toEqual({ layout: { ...withSide("chat") }, main: "terminal" });
    // With nothing on the side, the chat leaves the terminal in the main pane.
    expect(openSide(DEFAULT_LAYOUT, "chat", "chat", "bottom").main).toBe("terminal");
  });

  test("opening another view on the side replaces the one there", () => {
    const next = openSide(withSide("terminal"), "chat", "diff", "bottom");
    expect(next.layout.side).toBe("diff");
    expect(next.layout.position).toBe("bottom");
    expect(next.main).toBe("chat");
  });

  test("moving the side's view back to the main area closes the split", () => {
    const back = moveToMain(withSide("terminal"), "terminal");
    expect(back.layout.side).toBeNull();
    expect(back.main).toBe("terminal");
    // Another view dropped on the tab bar is only shown in the main pane.
    const other = moveToMain(withSide("terminal"), "diff");
    expect(other.layout.side).toBe("terminal");
    expect(other.main).toBe("diff");
  });

  test("the side's tab focuses the side pane rather than showing the view twice", () => {
    expect(showView(withSide("terminal"), "chat", "terminal", false)).toEqual({ main: "chat", focusSide: true });
    expect(showView(withSide("terminal"), "chat", "diff", false)).toEqual({ main: "diff", focusSide: false });
    expect(showView(DEFAULT_LAYOUT, "chat", "terminal", false)).toEqual({ main: "terminal", focusSide: false });
    // A window too small for the split shows it in the main pane.
    expect(showView(withSide("terminal"), "chat", "terminal", true)).toEqual({ main: "terminal", focusSide: false });
  });

  test("the split waits while the window is too small or both panes would show one view", () => {
    expect(panes(withSide("terminal"), "chat", false).side).toBe("terminal");
    expect(panes(withSide("terminal"), "chat", true).side).toBeNull();
    expect(panes(withSide("terminal"), "terminal", false).side).toBeNull();
    expect(panes(DEFAULT_LAYOUT, "chat", false).side).toBeNull();
  });

  test("the main pane starts on the chat, or on the terminal when the chat is on the side", () => {
    expect(defaultMain(DEFAULT_LAYOUT)).toBe("chat");
    expect(defaultMain(withSide("terminal"))).toBe("chat");
    expect(defaultMain(withSide("chat"))).toBe("terminal");
  });
});

describe("sizes", () => {
  test("a window too narrow for two panes collapses a split on the right, not one below", () => {
    const narrow = MIN_PANE.right * 2 + DIVIDER - 1;
    expect(tooSmall("right", narrow, 900)).toBe(true);
    expect(tooSmall("right", narrow + 1, 900)).toBe(false);
    expect(tooSmall("bottom", narrow, 900)).toBe(false);
    expect(tooSmall("bottom", 1200, MIN_PANE.bottom * 2 + DIVIDER - 1)).toBe(true);
  });

  test("each pane keeps its minimum size", () => {
    expect(sidePixels("right", 0.42, 1000)).toBe(420);
    expect(sidePixels("right", 0.05, 1000)).toBe(MIN_PANE.right);
    expect(sidePixels("right", 0.95, 1000)).toBe(1000 - MIN_PANE.right - DIVIDER);
    expect(sidePixels("bottom", 0.01, 600)).toBe(MIN_PANE.bottom);
  });

  test("the boxes tile the view area", () => {
    const right = splitBoxes("right", 0.4, 1000, 700);
    expect(right.main).toEqual({ left: 0, top: 0, width: 600 - DIVIDER, height: 700 });
    expect(right.divider.left + right.divider.width).toBe(right.header.left);
    expect(right.header).toEqual({ left: 600, top: 0, width: 400, height: SIDE_HEADER });
    expect(right.side).toEqual({ left: 600, top: SIDE_HEADER, width: 400, height: 700 - SIDE_HEADER });
    const bottom = splitBoxes("bottom", 0.4, 1000, 700);
    expect(bottom.main.height + DIVIDER + bottom.header.height + bottom.side.height).toBe(700);
    expect(bottom.side.top + bottom.side.height).toBe(700);
  });

  test("the divider dragged gives the side its share, within the minimums", () => {
    const area = { left: 100, top: 50, width: 1000, height: 600 };
    expect(shareAt("right", area, { x: 700, y: 0 })).toBeCloseTo(0.4);
    expect(shareAt("right", area, { x: 1095, y: 0 })).toBeCloseTo(MIN_PANE.right / 1000);
    expect(shareAt("bottom", area, { x: 0, y: 410 })).toBeCloseTo(0.4);
  });
});

describe("drop zones", () => {
  const area = { left: 0, top: 100, width: 900, height: 600 };
  test("near the right edge lands on the right, near the bottom below, the middle nowhere", () => {
    expect(dropZone(area, { x: 850, y: 300 })).toBe("right");
    expect(dropZone(area, { x: 300, y: 650 })).toBe("bottom");
    expect(dropZone(area, { x: 450, y: 400 })).toBeNull();
    expect(dropZone(area, { x: 1000, y: 400 })).toBeNull();
    expect(dropZone(area, { x: 450, y: 50 })).toBeNull();
  });

  test("in the corner, the nearer edge wins", () => {
    expect(dropZone(area, { x: 895, y: 600 })).toBe("right");
    expect(dropZone(area, { x: 700, y: 695 })).toBe("bottom");
  });
});

describe("kept per workspace", () => {
  test("a layout survives a round trip", () => {
    const layouts = { "ws-1": { ...withSide("diff", "bottom"), size: { right: 0.5, bottom: 0.3 } } };
    expect(parseLayouts(serializeLayouts(layouts))).toEqual(layouts);
    expect(LAYOUTS_KEY).toBe("chauffeur.splitLayouts");
  });

  test("anything unreadable falls back to the defaults", () => {
    expect(parseLayouts("not json")).toEqual({});
    expect(parseLayouts("[1]")).toEqual({});
    expect(parseLayouts(null)).toEqual({});
    expect(parseLayouts('{"ws":{"side":"browser","position":"left","size":{"right":7}}}')).toEqual({
      ws: DEFAULT_LAYOUT,
    });
  });

  test("the layout changed last is kept when old ones are dropped", () => {
    const layouts: Record<string, SplitLayout> = {};
    for (let i = 0; i < 205; i++) layouts[`ws-${i}`] = DEFAULT_LAYOUT;
    const kept = parseLayouts(serializeLayouts(layouts, "ws-0"));
    expect(Object.keys(kept)).toHaveLength(200);
    expect(kept["ws-0"]).toBeDefined();
    expect(kept["ws-1"]).toBeUndefined();
  });
});
