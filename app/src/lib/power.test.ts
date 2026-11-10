import { describe, expect, test } from "bun:test";
import type { PowerState } from "@chauffeur/protocol";
import { powerHint, powerReason } from "./power";

const state = (change: Partial<PowerState>): PowerState => ({
  keep_awake: "while_working",
  holding: "none",
  working_chats: 0,
  running_tasks: 0,
  creating_workspaces: 0,
  lingering_secs: null,
  low_battery: false,
  battery_percent: null,
  warning: null,
  ...change,
});

describe("powerReason", () => {
  test("names the agents, their tasks and the workspaces being created", () => {
    expect(powerReason(state({ working_chats: 2 }))).toBe("2 agents working");
    expect(powerReason(state({ working_chats: 1, running_tasks: 1, creating_workspaces: 2 }))).toBe(
      "1 agent working, 1 background task, 2 workspaces being created",
    );
  });

  test("once the work ended, how long it is kept", () => {
    expect(powerReason(state({ lingering_secs: 90 }))).toBe("the agents just finished; 2 more minutes in case they go on");
    expect(powerReason(state({ lingering_secs: 20 }))).toBe("the agents just finished; 1 more minute in case they go on");
  });
});

describe("powerHint", () => {
  test("nothing while the machine may sleep", () => {
    expect(powerHint(null, true)).toBeNull();
    expect(powerHint(state({}), true)).toBeNull();
    expect(powerHint(state({ keep_awake: "never", working_chats: 3 }), true)).toBeNull();
  });

  test("kept from idle sleep while agents work", () => {
    const hint = powerHint(state({ holding: "idle_sleep", working_chats: 2 }), true);
    expect(hint?.kind).toBe("awake");
    expect(hint?.text).toBe("Keeping the Mac awake: 2 agents working. The display may sleep; closing the lid still sleeps it.");
    expect(powerHint(state({ holding: "idle_sleep", working_chats: 1 }), false)?.text).toStartWith("Keeping the computer awake");
  });

  test("sleep off with the lid closed, and given back on low battery", () => {
    const off = powerHint(state({ keep_awake: "lid_closed", holding: "all_sleep", working_chats: 1 }), true);
    expect(off?.kind).toBe("lid_closed");
    expect(off?.text).toStartWith("Sleep is off, even with the lid closed: 1 agent working.");
    const low = powerHint(
      state({ keep_awake: "lid_closed", holding: "idle_sleep", working_chats: 1, low_battery: true, battery_percent: 14 }),
      true,
    );
    expect(low?.kind).toBe("low_battery");
    expect(low?.text).toStartWith("Battery low (14%): closing the lid sleeps the Mac again until it charges.");
  });

  test("a warning wins over the plain hint", () => {
    const hint = powerHint(state({ keep_awake: "lid_closed", holding: "idle_sleep", working_chats: 1, warning: "No rule" }), true);
    expect(hint).toEqual({ kind: "warning", text: "No rule" });
  });
});
