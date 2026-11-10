import { describe, expect, spyOn, test } from "bun:test";
import hljs from "highlight.js/lib/core";
import { highlight } from "./highlight";

describe("highlight", () => {
  test("a code block drawn again is not highlighted again, until 200 others came after it", () => {
    const spy = spyOn(hljs, "highlight");
    const first = highlight("const answer = 42;", "ts");
    expect(first).toContain("hljs-keyword");
    expect(highlight("const answer = 42;", "ts")).toBe(first);
    expect(spy).toHaveBeenCalledTimes(1);
    for (let i = 0; i < 200; i++) highlight(`let n = ${i};`, "ts");
    expect(highlight("const answer = 42;", "ts")).toBe(first);
    expect(spy).toHaveBeenCalledTimes(202);
    spy.mockRestore();
  });

  test("the same code in another language is highlighted for that language", () => {
    expect(highlight("x = 1", "python")).not.toBe(highlight("x = 1", "unknown-language"));
    expect(highlight("<b>", null)).toBe("&lt;b&gt;");
  });
});
