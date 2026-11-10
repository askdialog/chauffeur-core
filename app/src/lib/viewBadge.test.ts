import { describe, expect, test } from "bun:test";
import { viewBadge } from "./viewBadge";

describe("view badge", () => {
  test("nothing open shows nothing", () => {
    expect(viewBadge(0)).toEqual({ count: null, dot: null });
  });

  test("one thing open shows a faint dot, not a count", () => {
    expect(viewBadge(1)).toEqual({ count: null, dot: "faint" });
  });

  test("several open show their count and no dot", () => {
    expect(viewBadge(3)).toEqual({ count: 3, dot: null });
  });

  test("an activity mark takes the faint dot's place and sits beside the count", () => {
    expect(viewBadge(1, "running")).toEqual({ count: null, dot: "running" });
    expect(viewBadge(2, "unseen")).toEqual({ count: 2, dot: "unseen" });
    expect(viewBadge(1, "attention")).toEqual({ count: null, dot: "attention" });
    expect(viewBadge(3, "working")).toEqual({ count: 3, dot: "working" });
  });
});
