import { expect, test } from "bun:test";
import { popoverLeft } from "./ScreenComments";

test("a pin's box opens on its right, or on its left near the screen's right edge", () => {
  expect(popoverLeft(100, 1000)).toBe(112);
  expect(popoverLeft(900, 1000)).toBe(568);
  expect(popoverLeft(200, 300)).toBe(0);
});
