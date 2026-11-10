import { describe, expect, test } from "bun:test";
import { canStep, EMPTY_NAV, step, visit } from "./navHistory";

const all = () => true;

describe("navHistory", () => {
  test("going back returns to the workspace the user came from, and forward goes there again", () => {
    let history = visit(EMPTY_NAV, "a", "b");
    history = visit(history, "b", "c");
    const back = step(history, "c", -1, all);
    expect(back?.id).toBe("b");
    const forward = step(back!.history, "b", 1, all);
    expect(forward?.id).toBe("c");
    expect(forward?.history).toEqual(history);
  });

  test("going to a new workspace after stepping back drops what was ahead", () => {
    const history = visit(EMPTY_NAV, "a", "b");
    const back = step(history, "b", -1, all)!;
    expect(visit(back.history, "a", "d").forward).toEqual([]);
  });

  test("selecting the workspace already shown changes nothing", () => {
    const history = visit(EMPTY_NAV, "a", "b");
    expect(visit(history, "b", "b")).toBe(history);
  });

  test("the first workspace shown leaves nothing behind", () => {
    expect(visit(EMPTY_NAV, null, "a")).toEqual(EMPTY_NAV);
  });

  test("workspaces that can no longer be reached are passed over", () => {
    let history = visit(EMPTY_NAV, "a", "b");
    history = visit(history, "b", "c");
    history = visit(history, "c", "d");
    const reachable = (id: string) => id !== "c";
    expect(step(history, "d", -1, reachable)?.id).toBe("b");
    expect(canStep(history, -1, (id) => id === "c")).toBe(true);
    expect(canStep(history, -1, () => false)).toBe(false);
    expect(step(history, "d", -1, () => false)).toBeNull();
  });

  test("with nothing behind or ahead there is nowhere to go", () => {
    expect(step(EMPTY_NAV, "a", -1, all)).toBeNull();
    expect(step(EMPTY_NAV, "a", 1, all)).toBeNull();
  });

  test("only the last fifty steps are kept", () => {
    let history = EMPTY_NAV;
    for (let i = 0; i < 80; i++) history = visit(history, `w${i}`, `w${i + 1}`);
    expect(history.back).toHaveLength(50);
    expect(history.back[49]).toBe("w79");
  });
});
