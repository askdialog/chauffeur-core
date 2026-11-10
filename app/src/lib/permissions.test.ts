import { describe, expect, test } from "bun:test";
import type { PermissionMode } from "@chauffeur/protocol";
import { DEFAULT_PERMISSION_MODE, PERMISSION_MODES, permissionMode, shownPermissionMode } from "./permissions";

describe("permission modes", () => {
  test("every mode appears once, auto first and bypass last", () => {
    const values = PERMISSION_MODES.map((mode) => mode.value);
    const all: PermissionMode[] = ["auto", "default", "accept_edits", "plan", "bypass_permissions"];
    expect([...values].sort()).toEqual([...all].sort());
    expect(values[0]).toBe("auto");
    expect(values.at(-1)).toBe("bypass_permissions");
    expect(DEFAULT_PERMISSION_MODE).toBe("auto");
  });

  test("labels, descriptions and the warning tint", () => {
    expect(PERMISSION_MODES.map((mode) => mode.label)).toEqual(["Auto", "Ask", "Accept edits", "Plan", "Bypass"]);
    expect(permissionMode("auto").description).toBe("Asks only for risky actions");
    for (const mode of PERMISSION_MODES) expect(mode.description).not.toBe("");
    expect(PERMISSION_MODES.filter((mode) => mode.risky).map((mode) => mode.value)).toEqual(["bypass_permissions"]);
  });

  test("a mode the app does not know yet still shows by its name", () => {
    const unknown = permissionMode("someday" as PermissionMode);
    expect(unknown.label).toBe("someday");
    expect(unknown.risky).toBe(false);
  });
});

describe("the mode shown on the picker", () => {
  test("auto that Claude Code fell back from shows as asking, with a hint and the reason", () => {
    const shown = shownPermissionMode("auto", "default", "claude-haiku-4-5-20251001");
    expect(shown.mode.value).toBe("default");
    expect(shown.mode.label).toBe("Ask");
    expect(shown.hint).toBe("(Auto unavailable)");
    expect(shown.tooltip).toContain("Auto mode is unavailable for this chat with claude-haiku-4-5-20251001");
  });

  test("the mode asked shows when the agent runs in it, has not said yet, or changed it itself", () => {
    for (const effective of ["auto", null, undefined, "plan"] as const) {
      const shown = shownPermissionMode("auto", effective);
      expect(shown.mode.label).toBe("Auto");
      expect(shown.hint).toBeNull();
      expect(shown.tooltip).toBeNull();
    }
    // Asking was asked: nothing fell back.
    expect(shownPermissionMode("default", "default").hint).toBeNull();
    expect(shownPermissionMode("accept_edits", "accept_edits").mode.label).toBe("Accept edits");
  });
});
