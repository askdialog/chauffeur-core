import { describe, expect, test } from "bun:test";
import { digitOf, fromTerminal, fromVimEditor, hasMod, hasSecondMod, historyKey, inForeignEditor, kbd, stepTab } from "./keys";

const keys = (over: Partial<{ metaKey: boolean; ctrlKey: boolean; altKey: boolean }>) => ({
  metaKey: false,
  ctrlKey: false,
  altKey: false,
  ...over,
});

describe("shortcuts", () => {
  test("⌘ on macOS is Ctrl elsewhere", () => {
    expect(hasMod(keys({ metaKey: true }), true)).toBe(true);
    expect(hasMod(keys({ ctrlKey: true }), true)).toBe(false);
    expect(hasMod(keys({ ctrlKey: true }), false)).toBe(true);
    expect(hasMod(keys({ metaKey: true }), false)).toBe(false);
    expect(hasSecondMod(keys({ ctrlKey: true }), true)).toBe(true);
    expect(hasSecondMod(keys({ altKey: true }), false)).toBe(true);
  });

  test("labels keep macOS's symbols there and spell keys out elsewhere", () => {
    expect(kbd("⌘⇧E", true)).toBe("⌘⇧E");
    expect(kbd("⌘N", false)).toBe("Ctrl+N");
    expect(kbd("⌘⇧E", false)).toBe("Ctrl+Shift+E");
    expect(kbd("⌘⇧/", false)).toBe("Ctrl+Shift+/");
    expect(kbd("⌘↵", false)).toBe("Ctrl+Enter");
    expect(kbd("⌃⌘1", false)).toBe("Ctrl+Alt+1");
    expect(kbd("⌥⌘1", false)).toBe("Ctrl+Alt+1");
    expect(kbd("⌥⌘1", true)).toBe("⌥⌘1");
    expect(kbd("⌥1", false)).toBe("Alt+1");
  });

  test("outside macOS the terminal keeps its keys", () => {
    const inside = (terminal: boolean) =>
      ({ target: { closest: (selector: string) => (terminal && selector === ".xterm" ? {} : null) } }) as unknown as Event;
    expect(fromTerminal(inside(true), false)).toBe(true);
    expect(fromTerminal(inside(false), false)).toBe(false);
    expect(fromTerminal(inside(true), true)).toBe(false);
    expect(fromTerminal({ target: null } as unknown as Event, false)).toBe(false);
  });

  test("an editor in Vim mode keeps its Esc", () => {
    const inside = (vim: boolean) =>
      ({ target: { closest: (selector: string) => (vim && selector === "[data-vim]" ? {} : null) } }) as unknown as Event;
    expect(fromVimEditor(inside(true))).toBe(true);
    expect(fromVimEditor(inside(false))).toBe(false);
    expect(fromVimEditor({ target: null } as unknown as Event)).toBe(false);
  });
});

describe("digit shortcuts", () => {
  test("are read from the key's place, whatever it types", () => {
    expect(digitOf({ code: "Digit1" })).toBe(1);
    expect(digitOf({ code: "Numpad9" })).toBe(9);
    expect(digitOf({ code: "Digit0" })).toBeNull();
    expect(digitOf({ code: "KeyA" })).toBeNull();
  });
});

describe("inForeignEditor", () => {
  // An element inside the given selectors, as `closest` answers for it.
  const inside = (...selectors: string[]) =>
    ({ closest: (selector: string) => (selectors.some((s) => selector.includes(s)) ? {} : null) }) as unknown as EventTarget;

  test("the Files editor and the terminal keep their keys, so ⌘F and ⌘↑ leave them alone", () => {
    expect(inForeignEditor(inside(".cm-editor"))).toBe(true);
    expect(inForeignEditor(inside(".xterm"))).toBe(true);
  });

  test("the chat composer, an editor too, lets the chat's keys through", () => {
    expect(inForeignEditor(inside(".cm-editor", "[data-composer]"))).toBe(false);
  });

  test("a plain field, the page or no target are no editor", () => {
    expect(inForeignEditor(inside())).toBe(false);
    expect(inForeignEditor({} as EventTarget)).toBe(false);
    expect(inForeignEditor(null)).toBe(false);
  });
});

describe("history shortcuts", () => {
  const press = (key: string, over: Partial<{ code: string; metaKey: boolean; ctrlKey: boolean; altKey: boolean; shiftKey: boolean }> = {}) =>
    ({ key, code: "", metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...over }) as Parameters<typeof historyKey>[0];

  test("⌘[ and ⌘] go back and forward between workspaces, ⇧⌘[ and ⇧⌘] stay the tabs'", () => {
    expect(historyKey(press("[", { code: "BracketLeft", metaKey: true }), true)).toBe("back");
    expect(historyKey(press("]", { code: "BracketRight", metaKey: true }), true)).toBe("forward");
    expect(historyKey(press("^", { code: "BracketLeft", metaKey: true }), true)).toBe("back");
    expect(historyKey(press("[", { code: "BracketLeft", ctrlKey: true }), false)).toBe("back");
    expect(historyKey(press("{", { code: "BracketLeft", metaKey: true, shiftKey: true }), true)).toBeNull();
    expect(historyKey(press("[", { code: "BracketLeft" }), true)).toBeNull();
    expect(historyKey(press("[", { code: "BracketLeft", metaKey: true, ctrlKey: true }), true)).toBeNull();
  });
});

describe("tabs", () => {
  test("the next and previous tabs wrap around the ends", () => {
    const ids = ["a", "b", "c"];
    expect(stepTab(ids, "a", 1)).toBe("b");
    expect(stepTab(ids, "c", 1)).toBe("a");
    expect(stepTab(ids, "a", -1)).toBe("c");
    expect(stepTab(ids, "b", -1)).toBe("a");
    expect(stepTab(ids, "gone", 1)).toBe("a");
    expect(stepTab(ids, null, -1)).toBe("c");
    expect(stepTab([], "a", 1)).toBeNull();
  });
});
