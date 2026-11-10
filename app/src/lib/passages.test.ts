import { expect, test } from "bun:test";
import { Window } from "happy-dom";
import { findPassage, passageRange, rangeText } from "./passages";

test("a passage is found whatever the whitespace between its words", () => {
  const text = "Plan\nStep one: ship it.\n\nStep two: tell Louis.";
  expect(findPassage(text, "ship it. Step   two")).toEqual({ start: 15, end: 33 });
  expect(text.slice(15, 33)).toBe("ship it.\n\nStep two");
  expect(findPassage(text, "Step three")).toBeNull();
  expect(findPassage(text, "   ")).toBeNull();
});

test("a passage across blocks of the rendered file is found again and covers the same text", () => {
  const { document } = new Window();
  const root = document.createElement("div");
  root.innerHTML = "<h1>Plan</h1><ol><li>Ship <code>it</code> on Friday</li><li>Tell Louis</li></ol>";
  const range = passageRange(root as unknown as Node, "it on Friday Tell");
  expect(range).not.toBeNull();
  expect(range && rangeText(range)).toBe("it on FridayTell");
  expect(passageRange(root as unknown as Node, "Ship it Monday")).toBeNull();
});
