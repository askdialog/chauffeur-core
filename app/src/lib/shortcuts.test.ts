import { describe, expect, test } from "bun:test";
import {
  chatJump,
  conflictsOf,
  forApp,
  keysOf,
  labelOf,
  onceIn,
  parseBinding,
  pressed,
  rebind,
  recordBinding,
  reservedAs,
  SHORTCUT_IDS,
  tabKey,
  terminalTabKey,
  viewKey,
  viewShortcut,
} from "./shortcuts";

type Press = Parameters<typeof pressed>[0];
const xterm = { closest: (selector: string) => (selector === ".xterm" ? {} : null) };
const press = (
  key: string,
  over: Partial<{ code: string; metaKey: boolean; ctrlKey: boolean; altKey: boolean; shiftKey: boolean; target: unknown; getModifierState: (key: string) => boolean }> = {},
) => ({ key, code: "", metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, target: null, ...over }) as Press;

describe("the registry", () => {
  test("every default parses and no two shortcuts share a key, on either platform", () => {
    for (const mac of [true, false]) {
      for (const id of SHORTCUT_IDS) {
        for (const keys of keysOf(id, mac, {})) expect(parseBinding(keys)).not.toBeNull();
        expect(conflictsOf(id, mac, {})).toEqual([]);
      }
    }
  });

  test("labels follow the platform", () => {
    expect(labelOf("tab.next", true, {})).toBe("⌃Tab / ⌘PgDn / ⇧⌘]");
    expect(labelOf("tab.next", false, {})).toBe("Ctrl+Tab / Ctrl+PgDn");
    expect(labelOf("model.pick", false, {})).toBe("Ctrl+Alt+1–9");
    expect(labelOf("chat.new", false, { "chat.new": "⇧⌘J" })).toBe("Ctrl+Shift+J");
    expect(labelOf("terminal.new", true, {})).toBe("⌘T");
    expect(labelOf("terminal.new", false, {})).toBe("Ctrl+Shift+T");
    expect(labelOf("view.toggle-terminal", true, {})).toBe("⌃`");
    expect(labelOf("view.toggle-terminal", false, {})).toBe("Ctrl+`");
    expect(labelOf("view.diff", true, {})).toBe("⌥⌘2");
    expect(labelOf("view.diff", false, {})).toBe("Alt+2");
  });

  // The same cases as `bindings_name_modifiers_once_then_one_ascii_or_named_key` and
  // `reserved_keys_are_refused_however_they_are_written` in the daemon's settings.rs.
  test("a binding is modifiers once, ⌘, ⌃ or ⌥ among them, then one ASCII or named key", () => {
    for (const keys of ["⌘T", "⇧⌘t", "⌃⌘1", "⌥⌘1", "⌘\\", "⇧⌘/", "⌘↑", "⌃Tab", "⌘PgDn", "⌘F5", "⌘,", "⌘+", "⌘↵", "⌃`", "⌥1", "⌥T"]) {
      expect(parseBinding(keys)).not.toBeNull();
    }
    for (const keys of ["T", "⇧T", "⌥⌥⌘T", "⌘⌘T", "⌘", "⌘TT", "⌘ ", "⌘F13", "⌘F01", "Ctrl+T", "⌘é", "⌘⌥", "⌘€"]) {
      expect(parseBinding(keys)).toBeNull();
    }
  });

  test("reserved keys are refused however they are written", () => {
    for (const keys of ["⌘C", "⌘c", "⌘V", "⌘X", "⌘A", "⌘Z", "⇧⌘Z", "⌘⇧Z", "⌘Q", "⌘D", "⌘H", "⌘M", "⌃C", "⌃D", "⌃Z", "⌃⌘F", "⌘⌃F"]) {
      expect(reservedAs(keys)).not.toBeNull();
    }
    for (const keys of ["⇧⌘C", "⌃⌘C", "⇧⌘V", "⌃⇧C", "⌘F", "⌃⌘Z", "⌥⌘C"]) {
      expect(reservedAs(keys)).toBeNull();
    }
    expect(reservedAs("⌘C")).toBe("Copy, and the terminal's interrupt outside macOS");
  });

  test("keys other handlers read are listed, so a shortcut given them shows a conflict", () => {
    expect(conflictsOf("chat.new", true, { "chat.new": "⌘F" })).toEqual(["chat.find"]);
    expect(conflictsOf("chat.new", false, { "chat.new": "⌘S" })).toEqual(["files.save"]);
    expect(conflictsOf("chat.new", true, { "chat.new": "⌘↵" })).toEqual(["composer.queue"]);
    expect(conflictsOf("chat.new", true, { "chat.new": "⌘[" })).toEqual(["workspace.back"]);
    expect(conflictsOf("chat.new", false, { "chat.new": "⌘]" })).toEqual(["workspace.forward"]);
    // ⇧⌘W is the macOS menu's; Linux has none.
    expect(conflictsOf("chat.new", true, { "chat.new": "⇧⌘W" })).toEqual(["window.close"]);
    expect(conflictsOf("chat.new", false, { "chat.new": "⇧⌘W" })).toEqual([]);
    expect(keysOf("window.close", false, {})).toEqual([]);
  });
});

describe("default keys", () => {
  test("zoom by the character typed, or the keypad (AZERTY types ) where QWERTY has -)", () => {
    const zoom = (event: Press) => (["zoom.in", "zoom.out", "zoom.reset"] as const).find((id) => pressed(event, id, true, {})) ?? null;
    expect(zoom(press("=", { code: "Equal", metaKey: true }))).toBe("zoom.in");
    expect(zoom(press("+", { code: "Equal", metaKey: true, shiftKey: true }))).toBe("zoom.in");
    expect(zoom(press("+", { code: "NumpadAdd", metaKey: true }))).toBe("zoom.in");
    expect(zoom(press("-", { code: "Digit6", metaKey: true }))).toBe("zoom.out");
    expect(zoom(press(")", { code: "Minus", metaKey: true }))).toBeNull();
    expect(zoom(press("à", { code: "Digit0", metaKey: true }))).toBe("zoom.reset");
    expect(zoom(press("=", { code: "Equal", metaKey: true, ctrlKey: true }))).toBeNull();
  });

  test("digits by their place, Shift or not", () => {
    expect(pressed(press("&", { code: "Digit1", ctrlKey: true }), "workspace.goto", false, {})).toBe(true);
    expect(pressed(press("1", { code: "Digit1", ctrlKey: true, shiftKey: true }), "workspace.goto", false, {})).toBe(true);
    expect(pressed(press("1", { code: "Digit1", ctrlKey: true, altKey: true }), "workspace.goto", false, {})).toBe(false);
    expect(pressed(press("1", { code: "Digit1", ctrlKey: true, altKey: true }), "model.pick", false, {})).toBe(true);
    expect(pressed(press("1", { code: "Digit1", metaKey: true, ctrlKey: true }), "model.pick", true, {})).toBe(true);
  });

  test("a character Shift changes is found by its place, one ⌥ types by the character", () => {
    expect(pressed(press("?", { code: "Slash", metaKey: true, shiftKey: true }), "model.effort", true, {})).toBe(true);
    expect(pressed(press("/", { code: "Slash", metaKey: true }), "model.effort", true, {})).toBe(false);
    // \ is ⌥⇧/ on a French Mac.
    expect(pressed(press("\\", { code: "Slash", metaKey: true, altKey: true, shiftKey: true }), "view.split", true, {})).toBe(true);
    // A German keyboard types # where US has \.
    expect(pressed(press("#", { code: "Backslash", ctrlKey: true }), "view.split", false, {})).toBe(true);
    expect(pressed(press("'", { code: "Backslash", ctrlKey: true, shiftKey: true }), "view.split", false, {})).toBe(false);
    // A Russian layout types Cyrillic letters.
    expect(pressed(press("е", { code: "KeyT", ctrlKey: true }), "chat.new", false, {})).toBe(true);
  });
});

describe("the user's keys", () => {
  test("replace the defaults", () => {
    const overrides = { "chat.new": "⇧⌘J" };
    expect(pressed(press("t", { metaKey: true }), "chat.new", true, overrides)).toBe(false);
    expect(pressed(press("J", { metaKey: true, shiftKey: true }), "chat.new", true, overrides)).toBe(true);
    expect(pressed(press("j", { metaKey: true }), "chat.new", true, overrides)).toBe(false);
  });

  test("in the terminal outside macOS, the shell keeps the default keys but not the ones the user gave", () => {
    const overrides = { "sidebar.toggle": "⇧⌘B" };
    expect(pressed(press("k", { ctrlKey: true, target: xterm }), "switcher.open", false, {})).toBe(false);
    expect(forApp(press("k", { ctrlKey: true, target: xterm }), false, {})).toBe(false);
    expect(pressed(press("B", { ctrlKey: true, shiftKey: true, target: xterm }), "sidebar.toggle", false, overrides)).toBe(true);
    expect(forApp(press("B", { ctrlKey: true, shiftKey: true, target: xterm }), false, overrides)).toBe(true);
    expect(forApp(press("Tab", { ctrlKey: true, target: xterm }), false, {})).toBe(true);
  });

  test("are recorded the macOS way, from a key press of either platform", () => {
    const keys = (event: Press, mac: boolean) => recordBinding(event, mac).keys;
    expect(keys(press("?", { code: "Slash", metaKey: true, shiftKey: true }), true)).toBe("⇧⌘/");
    expect(keys(press("k", { code: "KeyK", ctrlKey: true, altKey: true }), false)).toBe("⌃⌘K");
    expect(keys(press("&", { code: "Digit1", ctrlKey: true }), false)).toBe("⌘1");
    expect(keys(press("PageDown", { code: "PageDown", metaKey: true }), true)).toBe("⌘PgDn");
    expect(keys(press("ArrowUp", { code: "ArrowUp", ctrlKey: true }), false)).toBe("⌘↑");
    expect(keys(press("Tab", { code: "Tab", ctrlKey: true }), true)).toBe("⌃Tab");
    // macOS's ⌥ types another character: the key's place names it.
    expect(keys(press("˚", { code: "KeyK", metaKey: true, altKey: true }), true)).toBe("⌥⌘K");
    expect(keys(press("¡", { code: "Digit1", metaKey: true, altKey: true }), true)).toBe("⌥⌘1");
    // AZERTY's ² sits where US has `.
    expect(keys(press("²", { code: "Backquote", ctrlKey: true }), false)).toBe("⌘`");
    // Outside macOS, Alt alone too.
    expect(keys(press("&", { code: "Digit1", altKey: true }), false)).toBe("⌥1");
    expect(keys(press("1", { code: "Digit1", altKey: true }), true)).toBeUndefined();
  });

  test("with ⌥, on macOS, act by the key's place", () => {
    const overrides = { "chat.new": "⌥⌘K" };
    expect(pressed(press("˚", { code: "KeyK", metaKey: true, altKey: true }), "chat.new", true, overrides)).toBe(true);
    expect(pressed(press("k", { code: "KeyK", metaKey: true }), "chat.new", true, overrides)).toBe(false);
    expect(pressed(press("k", { code: "KeyK", ctrlKey: true, altKey: true }), "chat.new", false, overrides)).toBe(true);
  });

  test("the keys pressed show as typed, and a press that makes no binding says why", () => {
    expect(recordBinding(press("Shift", { code: "ShiftLeft", metaKey: true, shiftKey: true }), true)).toEqual({ typed: "⇧⌘…" });
    expect(recordBinding(press("Control", { code: "ControlLeft", ctrlKey: true }), false)).toEqual({ typed: "Ctrl+…" });
    expect(recordBinding(press("k", { code: "KeyK" }), true)).toEqual({ typed: "K", refused: "A shortcut needs ⌘ or ⌃" });
    expect(recordBinding(press("k", { code: "KeyK", shiftKey: true }), false).refused).toBe("A shortcut needs Ctrl or Alt");
    expect(recordBinding(press("¡", { code: "Digit1", altKey: true }), true).refused).toBe("A shortcut needs ⌘ or ⌃");
    // AltGr types characters (AZERTY's ~ # { [ on the digits): never recorded as Alt or Ctrl+Alt.
    const altGr = (key: string) => key === "AltGraph";
    expect(recordBinding(press("~", { code: "Digit2", altKey: true, getModifierState: altGr }), false).refused).toContain("AltGr");
    expect(recordBinding(press("#", { code: "Digit3", ctrlKey: true, altKey: true, getModifierState: altGr }), false).keys).toBeUndefined();
    expect(recordBinding(press("k", { code: "KeyK", ctrlKey: true, metaKey: true }), false).refused).toContain("Super");
    expect(recordBinding(press("ù", { code: "Quote", metaKey: true }), true).refused).toContain("not a key a shortcut takes");
    expect(recordBinding(press("c", { code: "KeyC", ctrlKey: true }), false)).toEqual({
      typed: "Ctrl+C",
      refused: "Ctrl+C stays Copy, and the terminal's interrupt outside macOS: press other keys",
    });
    expect(recordBinding(press("c", { code: "KeyC", ctrlKey: true }), true).refused).toContain("the terminal's interrupt on macOS");
  });

  test("recording a shortcut's only default key drops the user's", () => {
    expect(rebind({ "chat.new": "⇧⌘J", "chat.close": "⌘J" }, "chat.new", "⌘T", true)).toEqual({ "chat.close": "⌘J" });
    expect(rebind({}, "tab.next", "⌃Tab", true)).toEqual({ "tab.next": "⌃Tab" });
  });

  test("a key two shortcuts share is a conflict, outside macOS ⌃ and ⌃⌘ being the same keys", () => {
    expect(conflictsOf("chat.new", true, { "chat.new": "⌘K" })).toEqual(["switcher.open"]);
    expect(conflictsOf("switcher.open", true, { "chat.new": "⌘K" })).toEqual(["chat.new"]);
    expect(conflictsOf("chat.new", false, { "chat.new": "⌃2" })).toEqual(["model.pick"]);
    expect(conflictsOf("chat.new", false, { "chat.new": "⌥2" })).toEqual(["view.diff"]);
    expect(conflictsOf("chat.new", true, { "chat.new": "⌃2" })).toEqual([]);
    // ⌘1–9 take Shift too (AZERTY's digits need it), so ⇧⌘1 also goes to a workspace.
    expect(conflictsOf("chat.new", true, { "chat.new": "⇧⌘1" })).toEqual(["workspace.goto"]);
    expect(conflictsOf("chat.new", true, { "chat.new": "⇧⌘J" })).toEqual([]);
    expect(conflictsOf("chat.new", false, { "chat.new": "⌥⌘3" })).toEqual(["model.pick"]);
  });

  test("the tabs of the Chat and Terminal views share keys without conflict, not with other shortcuts", () => {
    expect(conflictsOf("terminal.new", true, {})).toEqual([]);
    expect(conflictsOf("chat.new", true, {})).toEqual([]);
    expect(conflictsOf("chat.reopen", false, {})).toEqual([]);
    expect(conflictsOf("terminal.new", true, { "terminal.new": "⌘W" })).toEqual(["terminal.close"]);
    expect(conflictsOf("terminal.new", true, { "terminal.new": "⌘K" })).toEqual(["switcher.open"]);
    expect(conflictsOf("terminal.close", false, { "terminal.close": "⌃`" })).toEqual([]);
    expect(conflictsOf("terminal.close", false, { "terminal.close": "⌘`" })).toEqual(["view.toggle-terminal"]);
  });
});

describe("chatJump", () => {
  const key = (k: string, mods: Partial<KeyboardEvent> = {}, target: unknown = { tagName: "DIV" }) =>
    ({ key: k, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, target, ...mods }) as KeyboardEvent;

  test("⌘↑ / ⌘↓ on macOS, Ctrl elsewhere", () => {
    expect(chatJump(key("ArrowUp", { metaKey: true }), true)).toBe("top");
    expect(chatJump(key("ArrowDown", { metaKey: true }), true)).toBe("bottom");
    expect(chatJump(key("ArrowUp", { ctrlKey: true }), false)).toBe("top");
    expect(chatJump(key("ArrowUp", { ctrlKey: true }), true)).toBeNull();
  });

  test("the chat composer, an editor, jumps when empty and keeps ⌘↑ for its caret with text", () => {
    const composer = (state: string) => ({
      isContentEditable: true,
      closest: (selector: string) => (selector === "[data-composer]" ? { getAttribute: () => state } : {}),
    });
    expect(chatJump(key("ArrowUp", { metaKey: true }, composer("empty")), true)).toBe("top");
    expect(chatJump(key("ArrowUp", { metaKey: true }, composer("text")), true)).toBeNull();
    expect(chatJump(key("Home", {}, composer("empty")), true)).toBeNull();
  });

  test("another editor and the terminal keep their keys", () => {
    const editor = { isContentEditable: true, closest: (selector: string) => (selector === "[data-composer]" ? null : {}) };
    expect(chatJump(key("ArrowUp", { metaKey: true }, editor), true)).toBeNull();
  });

  test("a composer with text keeps ⌘↑ for its caret; an empty one jumps", () => {
    expect(chatJump(key("ArrowUp", { metaKey: true }, { tagName: "TEXTAREA", value: "draft" }), true)).toBeNull();
    expect(chatJump(key("ArrowUp", { metaKey: true }, { tagName: "TEXTAREA", value: "" }), true)).toBe("top");
  });

  test("Home and End jump only outside a text field", () => {
    expect(chatJump(key("End"), true)).toBe("bottom");
    expect(chatJump(key("Home", {}, { tagName: "TEXTAREA", value: "" }), true)).toBeNull();
    expect(chatJump(key("Home", { shiftKey: true }), true)).toBeNull();
  });
});

describe("tab shortcuts", () => {
  test("Ctrl+Tab and Ctrl+Shift+Tab switch tabs on every platform, plain Tab never", () => {
    for (const mac of [true, false]) {
      expect(tabKey(press("Tab", { ctrlKey: true }), mac)).toBe("next");
      expect(tabKey(press("Tab", { ctrlKey: true, shiftKey: true }), mac)).toBe("previous");
      expect(tabKey(press("Tab"), mac)).toBeNull();
      expect(tabKey(press("Tab", { shiftKey: true }), mac)).toBeNull();
    }
    expect(tabKey(press("Tab", { metaKey: true }), true)).toBeNull();
  });

  test("⌘PageDown and ⌘PageUp switch tabs, ⇧⌘] and ⇧⌘[ too on macOS", () => {
    expect(tabKey(press("PageDown", { metaKey: true }), true)).toBe("next");
    expect(tabKey(press("PageUp", { ctrlKey: true }), false)).toBe("previous");
    expect(tabKey(press("PageDown", { metaKey: true, shiftKey: true }), true)).toBeNull();
    expect(tabKey(press("}", { code: "BracketRight", metaKey: true, shiftKey: true }), true)).toBe("next");
    expect(tabKey(press("{", { code: "BracketLeft", metaKey: true, shiftKey: true }), true)).toBe("previous");
    expect(tabKey(press("}", { code: "BracketRight", ctrlKey: true, shiftKey: true }), false)).toBeNull();
  });

  test("⌘T opens a chat, ⌘W closes it, ⇧⌘T reopens one", () => {
    expect(tabKey(press("t", { metaKey: true }), true)).toBe("new");
    expect(tabKey(press("t", { ctrlKey: true }), false)).toBe("new");
    expect(tabKey(press("W", { ctrlKey: true }), false)).toBe("close");
    expect(tabKey(press("T", { metaKey: true, shiftKey: true }), true)).toBe("reopen");
    expect(tabKey(press("w", { metaKey: true, ctrlKey: true }), true)).toBeNull();
    expect(tabKey(press("t", { ctrlKey: true, altKey: true }), false)).toBeNull();
  });

  test("outside macOS the terminal keeps Ctrl+T and Ctrl+W but not the tab-switching keys", () => {
    expect(tabKey(press("t", { ctrlKey: true, target: xterm }), false)).toBeNull();
    expect(tabKey(press("w", { ctrlKey: true, target: xterm }), false)).toBeNull();
    expect(tabKey(press("T", { ctrlKey: true, shiftKey: true, target: xterm }), false)).toBeNull();
    expect(tabKey(press("Tab", { ctrlKey: true, target: xterm }), false)).toBe("next");
    expect(tabKey(press("PageUp", { ctrlKey: true, target: xterm }), false)).toBe("previous");
    expect(tabKey(press("w", { metaKey: true, target: xterm }), true)).toBe("close");
  });
});

describe("terminal tab shortcuts", () => {
  test("⌘T opens a terminal and ⌘W closes it on macOS; Ctrl+Shift+T and Ctrl+Shift+W elsewhere, from the terminal too", () => {
    expect(terminalTabKey(press("t", { metaKey: true }), true)).toBe("new");
    expect(terminalTabKey(press("w", { metaKey: true, target: xterm }), true)).toBe("close");
    expect(terminalTabKey(press("T", { ctrlKey: true, shiftKey: true, target: xterm }), false)).toBe("new");
    expect(terminalTabKey(press("W", { ctrlKey: true, shiftKey: true, target: xterm }), false)).toBe("close");
    expect(terminalTabKey(press("Tab", { ctrlKey: true, target: xterm }), false)).toBe("next");
    expect(terminalTabKey(press("T", { metaKey: true, shiftKey: true }), true)).toBeNull();
  });

  test("outside macOS Ctrl+T and Ctrl+W stay the shell's", () => {
    expect(terminalTabKey(press("t", { ctrlKey: true, target: xterm }), false)).toBeNull();
    expect(terminalTabKey(press("w", { ctrlKey: true }), false)).toBeNull();
    expect(forApp(press("t", { ctrlKey: true, target: xterm }), false, {})).toBe(false);
    expect(forApp(press("T", { ctrlKey: true, shiftKey: true, target: xterm }), false, {})).toBe(true);
    expect(forApp(press("W", { ctrlKey: true, shiftKey: true, target: xterm }), false, {})).toBe(true);
  });
});

describe("view shortcuts", () => {
  test("⌃` shows the Terminal, and from it the Chat; Ctrl+` outside macOS, ² on AZERTY", () => {
    const toggle = press("`", { code: "Backquote", ctrlKey: true });
    expect(viewKey(toggle, "chat", true)).toBe("terminal");
    expect(viewKey(toggle, "terminal", true)).toBe("chat");
    expect(viewKey(toggle, "diff", true)).toBe("terminal");
    expect(viewKey(press("²", { code: "Backquote", ctrlKey: true, target: xterm }), "terminal", false)).toBe("chat");
    expect(viewKey(press("`", { code: "Backquote", metaKey: true }), "chat", true)).toBeNull();
    expect(viewKey(press("~", { code: "Backquote", ctrlKey: true, shiftKey: true }), "chat", false)).toBeNull();
  });

  test("⌥⌘1–6 select workspace views including Database; Alt+1–6 outside macOS, from the terminal too", () => {
    expect(viewKey(press("¡", { code: "Digit1", metaKey: true, altKey: true }), "terminal", true)).toBe("chat");
    expect(viewKey(press("£", { code: "Digit3", metaKey: true, altKey: true }), "chat", true)).toBe("files");
    expect(viewKey(press("é", { code: "Digit2", altKey: true, target: xterm }), "terminal", false)).toBe("diff");
    expect(viewKey(press("5", { code: "Digit5", altKey: true }), "chat", false)).toBe("processes");
    expect(viewKey(press("6", { code: "Digit6", altKey: true }), "chat", false)).toBe("database");
    expect(viewKey(press("6", { code: "Digit6", metaKey: true, altKey: true }), "chat", true)).toBe("database");
    expect(forApp(press("4", { code: "Digit4", altKey: true, target: xterm }), false, {})).toBe(true);
    // Ctrl+Alt+1–9 stay the model favorites' outside macOS, ⌥1 types ¡ on macOS.
    expect(viewKey(press("2", { code: "Digit2", ctrlKey: true, altKey: true }), "chat", false)).toBeNull();
    expect(pressed(press("2", { code: "Digit2", ctrlKey: true, altKey: true }), "model.pick", false, {})).toBe(true);
    expect(viewKey(press("¡", { code: "Digit1", altKey: true }), "chat", true)).toBeNull();
    // AltGr types AZERTY's ~ # { [ on the digits.
    const altGr = (key: string) => key === "AltGraph";
    expect(viewKey(press("~", { code: "Digit2", altKey: true, getModifierState: altGr }), "chat", false)).toBeNull();
    // ⌘1–9 stay the workspaces', ⌃⌘1–9 macOS's model favorites.
    expect(viewKey(press("1", { code: "Digit1", metaKey: true }), "chat", true)).toBeNull();
    expect(viewKey(press("1", { code: "Digit1", metaKey: true, ctrlKey: true }), "chat", true)).toBeNull();
    expect(viewKey(press("1", { code: "Digit1", ctrlKey: true }), "chat", false)).toBeNull();
  });

  test("each view tab names its shortcut", () => {
    expect(viewShortcut("files")).toBe("view.files");
    expect(labelOf(viewShortcut("processes"), false, {})).toBe("Alt+5");
  });
});

test("a shortcut and its macOS menu item fired by the same press act once; elsewhere every press acts", () => {
  let runs = 0;
  onceIn("test-action", () => runs++, 1_000, true);
  onceIn("test-action", () => runs++, 1_200, true);
  onceIn("test-action", () => runs++, 1_500, true);
  expect(runs).toBe(2);
  onceIn("test-action", () => runs++, 1_600, false);
  expect(runs).toBe(3);
});
