import { describe, expect, test } from "bun:test";
import { clampSidebarWidth, SIDEBAR_DEFAULT_WIDTH, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH, storedSidebarWidth } from "./sidebarWidth";

describe("clampSidebarWidth", () => {
  test("a width dragged past either bound stops at it", () => {
    expect(clampSidebarWidth(50)).toBe(SIDEBAR_MIN_WIDTH);
    expect(clampSidebarWidth(2000)).toBe(SIDEBAR_MAX_WIDTH);
  });

  test("a width within the bounds is kept, rounded to a pixel", () => {
    expect(clampSidebarWidth(300.6)).toBe(301);
  });
});

describe("storedSidebarWidth", () => {
  test("nothing stored gives the default width", () => {
    expect(storedSidebarWidth(null)).toBe(SIDEBAR_DEFAULT_WIDTH);
    expect(storedSidebarWidth("")).toBe(SIDEBAR_DEFAULT_WIDTH);
    expect(storedSidebarWidth("wide")).toBe(SIDEBAR_DEFAULT_WIDTH);
  });

  test("a stored width comes back, within the bounds", () => {
    expect(storedSidebarWidth("320")).toBe(320);
    expect(storedSidebarWidth("9999")).toBe(SIDEBAR_MAX_WIDTH);
  });
});
