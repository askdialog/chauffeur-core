import { expect, test } from "bun:test";
import { matchPrompts, opensPrompts } from "./prompts";

const prompts = [
  { name: "agreed", text: "Agreed, carry on" },
  { name: "Compact", text: "/compact Keep the decisions" },
  { name: "yes", text: "Yes, go ahead" },
];
const press = (fields: Partial<Parameters<typeof opensPrompts>[0]>) => ({
  key: "Tab",
  shiftKey: true,
  metaKey: false,
  ctrlKey: false,
  altKey: false,
  ...fields,
});

test("the menu keeps the prompts whose name or text holds the filter, ignoring case, in their order", () => {
  expect(matchPrompts(prompts, "")).toEqual(prompts);
  expect(matchPrompts(prompts, "COMPACT").map((p) => p.name)).toEqual(["Compact"]);
  expect(matchPrompts(prompts, "go AH").map((p) => p.name)).toEqual(["yes"]);
  expect(matchPrompts(prompts, "a").map((p) => p.name)).toEqual(["agreed", "Compact", "yes"]);
  expect(matchPrompts(prompts, "carry on!")).toEqual([]);
});

test("⇧Tab alone opens the prompts, not Tab nor ⇧Tab with another modifier", () => {
  expect(opensPrompts(press({}))).toBe(true);
  expect(opensPrompts(press({ shiftKey: false }))).toBe(false);
  expect(opensPrompts(press({ ctrlKey: true }))).toBe(false);
  expect(opensPrompts(press({ metaKey: true }))).toBe(false);
  expect(opensPrompts(press({ altKey: true }))).toBe(false);
  expect(opensPrompts(press({ key: "Enter" }))).toBe(false);
});
