import { describe, expect, test } from "bun:test";
import type { ChatEvent, EventEnvelope, WorkspaceEvent } from "@chauffeur/protocol";
import {
  appendLine,
  emptyWorkspaceLive,
  MAX_PROCESS_LINES,
  mergeProcessList,
  processLines,
  reduceWorkspace,
  type WorkspaceLive,
} from "./workspaceLive";

let seq = 0;
function w(event: WorkspaceEvent, persisted = true): EventEnvelope {
  return { seq: persisted ? ++seq : null, ts: "2026-10-13T00:00:00Z", event: { scope: "workspace", workspace_id: "w", event } };
}

function run(envelopes: EventEnvelope[]): WorkspaceLive {
  return envelopes.reduce(reduceWorkspace, emptyWorkspaceLive);
}

describe("reduceWorkspace", () => {
  test("is live once the daemon says the history was sent, not before", () => {
    const replaying = run([w({ type: "progress", message: "fetching alpha" })]);
    expect(replaying.live).toBe(false);
    const live = reduceWorkspace(replaying, w({ type: "replayed" }, false));
    expect(live.live).toBe(true);
    // A second marker (after a reconnection) changes nothing.
    expect(reduceWorkspace(live, w({ type: "replayed" }, false))).toBe(live);
  });

  test("logs progress, warnings and state changes", () => {
    const state = run([
      w({ type: "progress", message: "fetching alpha" }),
      w({ type: "warning", message: "could not fetch beta" }),
      w({ type: "state_changed", state: "ready", error: null }),
      w({ type: "state_changed", state: "cleanup_failed", error: "archive script of alpha exited with 1" }),
    ]);
    expect(state.log.map((l) => [l.level, l.message])).toEqual([
      ["progress", "fetching alpha"],
      ["warning", "could not fetch beta"],
      ["state", "ready"],
      ["error", "cleanup failed: archive script of alpha exited with 1"],
    ]);
  });

  test("keeps the latest transfer the daemon reported, outside the log", () => {
    const transfer = {
      id: "t",
      workspace_id: "w",
      slug: "w",
      source: "mac",
      destination: "vm",
      phase: "running" as const,
      log: ["checking the destination"],
      error: null,
      workspace_path: null,
      kept_copy: null,
      returned: false,
      keep_local_copy: false,
      started_at: "2026-10-26T10:00:00Z",
      finished_at: null,
      working_agents: 0,
      interrupt: false,
      archive: false,
    };
    const state = run([
      w({ type: "transfer", transfer }, false),
      w({ type: "transfer", transfer: { ...transfer, phase: "done", log: [...transfer.log, "restoring it on the destination"] } }, false),
    ]);
    expect(state.transfer?.phase).toBe("done");
    expect(state.transfer?.log).toHaveLength(2);
    expect(state.log).toEqual([]);
  });

  test("keeps whether the machine the workspace moved to answers, outside the log", () => {
    const down = run([w({ type: "relay", machine: "vm", connected: false, error: "the connection closed" }, false)]);
    expect(down.relay).toEqual({ machine: "vm", connected: false, error: "the connection closed" });
    expect(down.log).toEqual([]);
    const back = reduceWorkspace(down, w({ type: "relay", machine: "vm", connected: true, error: null }, false));
    expect(back.relay?.connected).toBe(true);
  });

  test("follows a script from start to exit with its live output", () => {
    const state = run([
      w({ type: "process_started", process_id: "p1", kind: "run", repo: "alpha", name: "alpha:server", command: "./serve" }),
      w({ type: "process_output", process_id: "p1", stream: "stdout", line: "listening" }, false),
      w({ type: "process_output", process_id: "p1", stream: "stderr", line: "warn" }, false),
      w({ type: "process_exited", process_id: "p1", exit_code: 0 }),
    ]);
    expect(state.processOrder).toEqual(["p1"]);
    expect(state.processes.p1).toMatchObject({ name: "alpha:server", running: false, exitCode: 0 });
    expect(processLines(state.processes.p1.output)).toEqual([
      { stream: "stdout", line: "listening" },
      { stream: "stderr", line: "warn" },
    ]);
  });

  test("caps a process's lines", () => {
    let state = run([w({ type: "process_started", process_id: "p", kind: "run", repo: null, name: "x", command: "x" })]);
    for (let i = 0; i < MAX_PROCESS_LINES + 10; i++) {
      state = reduceWorkspace(state, w({ type: "process_output", process_id: "p", stream: "stdout", line: `${i}` }, false));
    }
    const lines = processLines(state.processes.p.output);
    expect(lines).toHaveLength(MAX_PROCESS_LINES);
    expect(lines[0].line).toBe("10");
    expect(lines.at(-1)?.line).toBe(`${MAX_PROCESS_LINES + 9}`);
  });

  test("a noisy process's lines are added without copying them, the buffer kept under twice the cap", () => {
    let output = appendLine({ buffer: [], start: 0, end: 0 }, { stream: "stdout", line: "0" });
    const buffer = output.buffer;
    for (let i = 1; i < MAX_PROCESS_LINES * 2 - 1; i++) output = appendLine(output, { stream: "stdout", line: `${i}` });
    expect(output.buffer).toBe(buffer);
    output = appendLine(output, { stream: "stdout", line: "last" });
    expect(output.buffer).not.toBe(buffer);
    expect(output.buffer).toHaveLength(MAX_PROCESS_LINES);
    expect(processLines(output).at(-1)?.line).toBe("last");
  });

  test("an earlier state keeps its lines when later ones add theirs, even from the same state", () => {
    const start = appendLine({ buffer: [], start: 0, end: 0 }, { stream: "stdout", line: "a" });
    const left = appendLine(start, { stream: "stdout", line: "b" });
    const right = appendLine(start, { stream: "stderr", line: "c" });
    expect(processLines(start).map((l) => l.line)).toEqual(["a"]);
    expect(processLines(left).map((l) => l.line)).toEqual(["a", "b"]);
    expect(processLines(right).map((l) => l.line)).toEqual(["a", "c"]);
  });

  test("routes chat events to their chat and counts finished turns", () => {
    const chat = (chatId: string, event: ChatEvent): EventEnvelope => ({
      seq: ++seq,
      ts: "",
      event: { scope: "chat", workspace_id: "w", chat_id: chatId, event },
    });
    const state = run([
      chat("a", { type: "user_message", text: "hello", queued: false }),
      chat("b", { type: "user_message", text: "other", queued: false }),
      chat("a", { type: "turn_ended", outcome: "success", duration_ms: 1, cost_usd: null, context_tokens: null, error: null }),
    ]);
    expect(Object.keys(state.chats).sort()).toEqual(["a", "b"]);
    expect(state.chats.a.items.map((i) => i.kind)).toEqual(["user", "turn"]);
    expect(state.turnsEnded).toBe(1);
  });

  test("terminal output is not kept, and duplicates are ignored", () => {
    const first = w({ type: "worktree_added", worktree: { id: "t", repo: "alpha", path: "/p", branch: "b", base_commit: null, base_branch: "main", origin: "workspace", group: null, removed: false } });
    let state = run([first, w({ type: "terminal_output", terminal_id: "t1", data: "aGk=", offset: 0 }, false)]);
    expect(state.worktreesChanged).toBe(1);
    const again = reduceWorkspace(state, first);
    expect(again).toBe(state);
    state = reduceWorkspace(state, w({ type: "worktree_removed", worktree_id: "t" }));
    expect(state.worktreesChanged).toBe(2);
  });
});

describe("mergeProcessList", () => {
  test("fills processes and tails that were not seen live", () => {
    const state = mergeProcessList(emptyWorkspaceLive, [
      { id: "p9", workspace_id: "w", kind: "setup", repo: "beta", name: "setup beta", command: "make", running: false, exit_code: 2, tail: ["a", "b"] },
    ]);
    expect(state.processes.p9).toMatchObject({ kind: "setup", exitCode: 2, running: false });
    expect(processLines(state.processes.p9.output).map((l) => l.line)).toEqual(["a", "b"]);
    expect(state.processOrder).toEqual(["p9"]);
  });
});
