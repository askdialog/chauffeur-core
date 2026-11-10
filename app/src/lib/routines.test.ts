import { describe, expect, test } from "bun:test";
import type { RoutineFindInfo, RoutineInfo, RoutineSuggestion, WorkspaceInfo } from "@chauffeur/protocol";
import { routineOfWorkspace, routinesSection, scheduleText, stateText, suggestionsText, whenText } from "./routines";

const routine = (more: Partial<RoutineInfo> = {}) =>
  ({ id: "r", workspace_id: "w", state: "draft", pending: false, next_run_at: null, ...more }) as RoutineInfo;

describe("routines", () => {
  test("a schedule reads as a person would say it", () => {
    expect(scheduleText({ days: ["fri", "mon", "tue", "wed", "thu"], times: ["08:00"] })).toBe("Weekdays at 08:00");
    expect(scheduleText({ days: ["mon", "tue", "wed", "thu", "fri", "sat", "sun"], times: ["17:30", "08:00"] })).toBe(
      "Every day at 08:00 and 17:30",
    );
    expect(scheduleText({ days: ["sun", "sat"], times: ["10:00"] })).toBe("Weekends at 10:00");
    expect(scheduleText({ days: ["thu", "mon"], times: ["09:00"] })).toBe("Mon, Thu at 09:00");
    expect(scheduleText({ days: [], times: ["09:00"] })).toBe("No schedule");
    expect(scheduleText({ days: [], times: ["09:00"], every_days: 3, from: "2026-11-10" })).toBe("Every 3 days at 09:00");
    expect(scheduleText({ days: [], times: ["09:00"], every_days: 1, from: "2026-11-10" })).toBe("Every day at 09:00");
  });

  test("the next run says the day only when it is not today", () => {
    const now = new Date(2026, 10, 9, 7, 0); // Monday 07:00
    expect(whenText(new Date(2026, 10, 9, 8, 0).toISOString(), now)).toBe("08:00");
    expect(whenText(new Date(2026, 10, 10, 8, 0).toISOString(), now)).toBe("Tue 08:00");
    expect(whenText(new Date(2026, 10, 20, 8, 0).toISOString(), now)).toBe("20 Nov 08:00");
  });

  test("only an active routine shows its next run, and a due one says so", () => {
    const now = new Date(2026, 10, 9, 7, 0);
    const next = new Date(2026, 10, 9, 8, 0).toISOString();
    expect(stateText(routine(), now)).toBe("draft");
    expect(stateText(routine({ state: "paused", next_run_at: null }), now)).toBe("paused");
    expect(stateText(routine({ state: "active", next_run_at: next }), now)).toBe("active · 08:00");
    expect(stateText(routine({ state: "active", next_run_at: next, pending: true }), now)).toBe("active · run due");
  });

  test("a workspace is a routine's when the routine names it", () => {
    expect(routineOfWorkspace({ r: routine() }, "w")?.id).toBe("r");
    expect(routineOfWorkspace({ r: routine() }, "other")).toBeUndefined();
  });

  test("the routines' section is the one their workspaces are in", () => {
    const ws = (id: string, section_id: string | null) => ({ id, section_id }) as WorkspaceInfo;
    const workspaces = { a: ws("a", "s1"), b: ws("b", "s2"), c: ws("c", "s2"), d: ws("d", null) };
    const routines = {
      r1: routine({ id: "r1", workspace_id: "a" }),
      r2: routine({ id: "r2", workspace_id: "b" }),
      r3: routine({ id: "r3", workspace_id: "c" }),
    };
    expect(routinesSection(routines, workspaces)).toBe("s2");
    expect(routinesSection({ r: routine({ workspace_id: "d" }) }, workspaces)).toBeNull();
    expect(routinesSection({}, workspaces)).toBeNull();
  });

  test("the Suggestions entry shows while an analysis runs or suggestions wait", () => {
    const find = (more: Partial<RoutineFindInfo>) =>
      ({ running: null, last_ended_at: null, last_error: null, last_found: null, suggestions: [], ...more }) as RoutineFindInfo;
    expect(suggestionsText(null)).toBeNull();
    expect(suggestionsText(find({}))).toBeNull();
    expect(suggestionsText(find({ suggestions: [{} as RoutineSuggestion, {} as RoutineSuggestion] }))).toBe("Suggestions (2)");
    expect(
      suggestionsText(find({ running: { started_at: "", days: 14, agent: "claude", messages: 10 } })),
    ).toBe("Finding routines…");
  });
});
