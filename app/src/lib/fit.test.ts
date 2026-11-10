import { describe, expect, test } from "bun:test";
import { fitCount } from "./fit";

describe("fitting chips on one line", () => {
  test("all of them when they fit", () => {
    expect(fitCount([100, 100, 100], 308, 4, 40)).toBe(3);
    expect(fitCount([], 0, 4, 40)).toBe(0);
  });

  test("as many as fit next to the +N chip otherwise", () => {
    // 100 + 4 + 100 + 4 + 40 = 248
    expect(fitCount([100, 100, 100], 250, 4, 40)).toBe(2);
    expect(fitCount([100, 100, 100], 247, 4, 40)).toBe(1);
    expect(fitCount([300, 100], 200, 4, 40)).toBe(0);
  });
});
