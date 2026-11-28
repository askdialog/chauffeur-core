import { expect, test } from "bun:test";
import { matchPrompts } from "./prompts";

const prompts = [
  { name: "agreed", text: "Agreed, carry on" },
  { name: "Compact", text: "/compact Keep the decisions" },
  { name: "yes", text: "Yes, go ahead" },
];

test("the menu keeps the prompts whose name or text holds the filter, ignoring case, in their order", () => {
  expect(matchPrompts(prompts, "")).toEqual(prompts);
  expect(matchPrompts(prompts, "COMPACT").map((p) => p.name)).toEqual(["Compact"]);
  expect(matchPrompts(prompts, "go AH").map((p) => p.name)).toEqual(["yes"]);
  expect(matchPrompts(prompts, "a").map((p) => p.name)).toEqual(["agreed", "Compact", "yes"]);
  expect(matchPrompts(prompts, "carry on!")).toEqual([]);
});
