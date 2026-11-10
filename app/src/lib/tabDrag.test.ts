import { describe, expect, test } from "bun:test";
import { beforeOf, dropIndex, edgeScroll, movedBefore, tabShift } from "./tabDrag";

// Three tabs of 100 px with a 2 px gap: middles at 50, 152, 254.
const boxes = [
  { left: 0, width: 100 },
  { left: 102, width: 100 },
  { left: 204, width: 100 },
];

describe("dragging a tab", () => {
  test("a tab lands past the tabs whose middle its leading edge passed", () => {
    // Not moved: its own place.
    expect(dropIndex(boxes, 0, 0)).toBe(0);
    expect(dropIndex(boxes, 1, 102)).toBe(1);
    // Dragged right: its right edge past the next tab's middle (152), then the last's (254).
    expect(dropIndex(boxes, 0, 51)).toBe(0);
    expect(dropIndex(boxes, 0, 53)).toBe(1);
    expect(dropIndex(boxes, 0, 155)).toBe(2);
    // Dragged left: its left edge back over a tab's middle.
    expect(dropIndex(boxes, 2, 153)).toBe(2);
    expect(dropIndex(boxes, 2, 151)).toBe(1);
    expect(dropIndex(boxes, 2, 49)).toBe(0);
  });

  test("a wide tab held at the strip's start passes a narrow first tab", () => {
    const narrowFirst = [
      { left: 0, width: 40 },
      { left: 42, width: 120 },
    ];
    expect(dropIndex(narrowFirst, 1, 0)).toBe(0);
  });

  test("the tabs between its place and where it lands move aside by its width", () => {
    // Dragged right from 0 to 2: tabs 1 and 2 move left.
    expect([0, 1, 2].map((i) => tabShift(i, 0, 2, 102))).toEqual([0, -102, -102]);
    // Dragged left from 2 to 0: tabs 0 and 1 move right.
    expect([0, 1, 2].map((i) => tabShift(i, 2, 0, 102))).toEqual([102, 102, 0]);
    // Not moved yet: nothing moves.
    expect([0, 1, 2].map((i) => tabShift(i, 1, 1, 102))).toEqual([0, 0, 0]);
  });

  test("a drop says which tab the moved one goes before, none for the last place", () => {
    const ids = ["a", "b", "c", "d"];
    expect(beforeOf(ids, 3, 0)).toBe("a");
    expect(beforeOf(ids, 0, 2)).toBe("d");
    expect(beforeOf(ids, 0, 3)).toBeUndefined();
    expect(movedBefore(ids, "d", beforeOf(ids, 3, 1))).toEqual(["a", "d", "b", "c"]);
    expect(movedBefore(ids, "a", beforeOf(ids, 0, 3))).toEqual(["b", "c", "d", "a"]);
  });

  test("a move before an unknown tab puts it last, as the daemon does", () => {
    expect(movedBefore(["a", "b", "c"], "a", "z")).toEqual(["b", "c", "a"]);
    expect(movedBefore(["a", "b", "c"], "b", undefined)).toEqual(["a", "c", "b"]);
  });

  test("the strip scrolls faster the closer the pointer gets to its edge, and not in between", () => {
    expect(edgeScroll(200, 0, 400)).toBe(0);
    expect(edgeScroll(0, 0, 400)).toBe(-12);
    expect(edgeScroll(14, 0, 400)).toBe(-6);
    expect(edgeScroll(-50, 0, 400)).toBe(-12);
    expect(edgeScroll(400, 0, 400)).toBe(12);
    expect(edgeScroll(386, 0, 400)).toBe(6);
  });
});
