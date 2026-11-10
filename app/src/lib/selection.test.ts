import { describe, expect, test } from "bun:test";
import { pickRange, togglePicked } from "./selection";

describe("selection", () => {
  test("the first ⌘-click picks the open workspace with the one clicked; another removes it", () => {
    expect(togglePicked([], "b", "a")).toEqual(["a", "b"]);
    expect(togglePicked([], "a", "a")).toEqual(["a"]);
    expect(togglePicked([], "b", null)).toEqual(["b"]);
    expect(togglePicked(["a", "b"], "c", "a")).toEqual(["a", "b", "c"]);
    expect(togglePicked(["a", "b"], "a", "a")).toEqual(["b"]);
  });

  test("⇧-click adds the rows between the anchor and the one clicked, either way", () => {
    const order = ["a", "b", "c", "d", "e"];
    expect(pickRange([], order, "b", "d")).toEqual(["b", "c", "d"]);
    expect(pickRange(["e"], order, "e", "c")).toEqual(["e", "c", "d"]);
    // An anchor no longer shown (folded, archived) picks the row alone.
    expect(pickRange([], order, "z", "c")).toEqual(["c"]);
    expect(pickRange(["a"], order, null, "c")).toEqual(["a", "c"]);
    expect(pickRange(["a"], order, "a", "z")).toEqual(["a"]);
  });
});
