import type { TerminalInfo } from "@chauffeur/protocol";
import { describe, expect, test } from "bun:test";
import {
  commandChanged,
  listedTerminals,
  terminalExited,
  terminalGone,
  terminalOpened,
  terminalSeen,
  terminalsIndicator,
} from "./terminalActivity";

function info(id: string, over: Partial<TerminalInfo> = {}): TerminalInfo {
  return {
    id,
    workspace_id: "w",
    cwd: "/w",
    title: "",
    name: null,
    cols: 80,
    rows: 24,
    running: true,
    exit_code: null,
    command_running: false,
    command_finished_at: null,
    ...over,
  };
}

describe("terminal activity", () => {
  test("a command that finishes in a terminal not on screen stays unseen until viewed", () => {
    let state = commandChanged({}, "t1", true, false);
    expect(state.t1).toEqual({ running: true, unseen: false });
    state = commandChanged(state, "t1", false, false);
    expect(state.t1).toEqual({ running: false, unseen: true });
    expect(terminalsIndicator(state)).toBe("unseen");
    state = terminalSeen(state, "t1");
    expect(state.t1).toEqual({ running: false, unseen: false });
    expect(terminalsIndicator(state)).toBeNull();
  });

  test("a command that finishes in the terminal on screen is seen at once", () => {
    const state = commandChanged(commandChanged({}, "t1", true, true), "t1", false, true);
    expect(state.t1).toEqual({ running: false, unseen: false });
  });

  test("a new command keeps the mark of the one that finished unseen", () => {
    const state = commandChanged(commandChanged({}, "t1", false, false), "t1", true, false);
    expect(state.t1).toEqual({ running: true, unseen: true });
  });

  test("the list sets what runs, keeps the unseen marks and forgets closed terminals", () => {
    const before = commandChanged(commandChanged({}, "t1", false, false), "t2", false, false);
    const state = listedTerminals(before, [info("t1", { command_running: true }), info("t3", { running: false, command_running: true })]);
    expect(state).toEqual({ t1: { running: true, unseen: true }, t3: { running: false, unseen: false } });
  });

  test("the workspace's tab shows a finished command before a running one", () => {
    expect(terminalsIndicator({})).toBeNull();
    expect(terminalsIndicator({ a: { running: true, unseen: false } })).toBe("running");
    expect(terminalsIndicator({ a: { running: true, unseen: false }, b: { running: false, unseen: true } })).toBe("unseen");
  });

  test("a closed terminal is forgotten", () => {
    const state = commandChanged({}, "t1", false, false);
    expect(terminalGone(state, "t1")).toEqual({});
    expect(terminalGone(state, "t2")).toBe(state);
  });

  test("a terminal opened from the app counts before any command runs in it", () => {
    const state = terminalOpened({}, "t1");
    expect(state).toEqual({ t1: { running: false, unseen: false } });
    expect(terminalOpened(state, "t1")).toBe(state);
  });

  test("a terminal whose shell exited stays open until closed, and one already closed is not brought back", () => {
    const state = terminalExited(commandChanged({}, "t1", true, false), "t1");
    expect(state).toEqual({ t1: { running: false, unseen: false } });
    expect(terminalExited(state, "t2")).toBe(state);
  });
});
