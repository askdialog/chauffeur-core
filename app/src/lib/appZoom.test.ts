import { describe, expect, test } from "bun:test";
import { stepZoom, storedZoom } from "./appZoom";

describe("app zoom", () => {
  test("steps through the levels and stops at both ends", () => {
    expect(stepZoom(1, 1)).toBe(1.1);
    expect(stepZoom(1, -1)).toBe(0.9);
    expect(stepZoom(2, 1)).toBe(2);
    expect(stepZoom(0.67, -1)).toBe(0.67);
    expect(stepZoom(1.5, 0)).toBe(1);
  });

  test("a stored level is kept only when it is one of the levels", () => {
    expect(storedZoom("1.25")).toBe(1.25);
    expect(storedZoom(null)).toBe(1);
    expect(storedZoom("7")).toBe(1);
  });
});
