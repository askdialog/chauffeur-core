import { expect, test } from "bun:test";
import { searchSettings } from "./settingsSearch";

test("option words and aliases find their settings without knowing the section", () => {
  expect(searchSettings("font", false).map((r) => r.target)).toEqual(["font-interface", "font-code", "font-terminal"]);
  expect(searchSettings("startup", false)[0]?.target).toBe("start-at-login");
  expect(searchSettings("sound", false)[0]?.section).toBe("notifications");
  expect(searchSettings("  TERMINAL font  ", false).map((r) => r.target)).toEqual(["font-terminal"]);
});

test("empty and unmatched queries return no unrelated options", () => {
  expect(searchSettings("   ", true)).toEqual([]);
  expect(searchSettings("unfindable-setting", true)).toEqual([]);
  expect(searchSettings("font sound", true)).toEqual([]);
});
