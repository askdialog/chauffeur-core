import { describe, expect, test } from "bun:test";
import { shellSequence } from "./terminalKeys";

const press = (key: string, over: Partial<{ type: string; altKey: boolean; metaKey: boolean; ctrlKey: boolean; shiftKey: boolean }> = {}) => ({
  type: "keydown",
  key,
  altKey: false,
  metaKey: false,
  ctrlKey: false,
  shiftKey: false,
  ...over,
});

describe("terminal keys", () => {
  test("⌥← and ⌥→ move one word on macOS, as in Terminal.app", () => {
    expect(shellSequence(press("ArrowLeft", { altKey: true }), true)).toBe("\x1bb");
    expect(shellSequence(press("ArrowRight", { altKey: true }), true)).toBe("\x1bf");
  });

  test("other keys and other platforms keep xterm's sequences", () => {
    expect(shellSequence(press("ArrowLeft", { altKey: true }), false)).toBeNull();
    expect(shellSequence(press("ArrowLeft"), true)).toBeNull();
    expect(shellSequence(press("ArrowUp", { altKey: true }), true)).toBeNull();
    expect(shellSequence(press("Backspace", { altKey: true }), true)).toBeNull();
    expect(shellSequence(press("ArrowLeft", { metaKey: true }), true)).toBeNull();
    expect(shellSequence(press("ArrowLeft", { altKey: true, metaKey: true }), true)).toBeNull();
    expect(shellSequence(press("ArrowLeft", { altKey: true, shiftKey: true }), true)).toBeNull();
    expect(shellSequence(press("ArrowRight", { altKey: true, ctrlKey: true }), true)).toBeNull();
  });

  test("only the key press sends, not its release", () => {
    expect(shellSequence(press("ArrowLeft", { type: "keyup", altKey: true }), true)).toBeNull();
  });
});
