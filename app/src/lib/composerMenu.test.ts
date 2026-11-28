import { expect, test } from "bun:test";
import { menuKey } from "./composerMenu";
import { isMac } from "./keys";

const press = (key: string, fields: Partial<Parameters<typeof menuKey>[0]> = {}) =>
  menuKey({ key, shiftKey: false, ctrlKey: false, metaKey: false, altKey: false, ...fields });

test("a menu above the composer closes on Esc, moves on ↑↓, and picks on ↵ or Tab", () => {
  expect(press("Escape")).toBe("close");
  expect(press("ArrowUp")).toBe("up");
  expect(press("ArrowDown")).toBe("down");
  expect(press("Enter")).toBe("pick");
  expect(press("Tab")).toBe("pick");
  expect(press("a")).toBeNull();
});

test("⇧Tab, Ctrl+Tab, ⇧↵ and ⌘↵ pick nothing in a menu", () => {
  expect(press("Tab", { shiftKey: true })).toBeNull();
  expect(press("Tab", { ctrlKey: true })).toBeNull();
  expect(press("Enter", { shiftKey: true })).toBeNull();
  expect(press("Enter", isMac ? { metaKey: true } : { ctrlKey: true })).toBeNull();
});
