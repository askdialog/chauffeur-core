import { describe, expect, test } from "bun:test";
import type { SDKMessage } from "@anthropic-ai/claude-agent-sdk";
import { ClaudeEventMapper } from "../src/agents/claude/events.ts";
import { ClaudeSession } from "../src/agents/claude/session.ts";
import { ErrorCode, type ChatEvent, type SessionStartParams } from "../src/protocol.ts";
import { RpcError } from "../src/rpc/peer.ts";
import { FakeHost, FakeQuery, flush, sdk } from "./support/fakes.ts";

/** Claude Code's task messages, with the fields the sidecar reads. */
const task = {
  started(taskId: string, fields: Record<string, unknown> = {}): SDKMessage {
    return {
      type: "system",
      subtype: "task_started",
      task_id: taskId,
      description: "Run the test suite",
      task_type: "local_bash",
      is_backgrounded: true,
      uuid: "u",
      session_id: "s",
      ...fields,
    } as unknown as SDKMessage;
  },
  progress(taskId: string, fields: Record<string, unknown> = {}): SDKMessage {
    return {
      type: "system",
      subtype: "task_progress",
      task_id: taskId,
      description: "Explore the repo",
      usage: { total_tokens: 1200, tool_uses: 3, duration_ms: 4500 },
      last_tool_name: "Grep",
      uuid: "u",
      session_id: "s",
      ...fields,
    } as unknown as SDKMessage;
  },
  updated(taskId: string, patch: Record<string, unknown>): SDKMessage {
    return { type: "system", subtype: "task_updated", task_id: taskId, patch, uuid: "u", session_id: "s" } as unknown as SDKMessage;
  },
  notification(taskId: string, status: string, summary = "done"): SDKMessage {
    return {
      type: "system",
      subtype: "task_notification",
      task_id: taskId,
      status,
      summary,
      output_file: "/tmp/out",
      uuid: "u",
      session_id: "s",
    } as unknown as SDKMessage;
  },
};

function mapAll(messages: SDKMessage[], mapper = new ClaudeEventMapper()): ChatEvent[] {
  return messages.flatMap((message) => mapper.map(message));
}

describe("mapping Claude Code's tasks", () => {
  test("a background command: started, progress, ended", () => {
    const events = mapAll([
      task.started("b1", { tool_use_id: "toolu_1" }),
      task.progress("b1", { description: "Run the test suite", last_tool_name: undefined }),
      task.notification("b1", "completed", "Tests passed"),
    ]);
    expect(events).toEqual([
      {
        type: "task_started",
        task_id: "b1",
        kind: "command",
        description: "Run the test suite",
        subagent_type: null,
        tool_use_id: "toolu_1",
        background: true,
        parent_task_id: null,
      },
      {
        type: "task_progress",
        task_id: "b1",
        activity: null,
        last_tool: null,
        summary: null,
        duration_ms: 4500,
        tool_uses: 3,
      },
      { type: "task_ended", task_id: "b1", status: "completed", summary: "Tests passed" },
    ]);
  });

  test("a subagent shows with its type, in the foreground too, and its progress", () => {
    const events = mapAll([
      task.started("a1", {
        task_type: "local_agent",
        subagent_type: "Explore",
        description: "Explore the repo",
        is_backgrounded: false,
        parent_task_id: "a0",
      }),
      task.progress("a1", { summary: "Reading the router" }),
    ]);
    expect(events[0]).toMatchObject({
      type: "task_started",
      kind: "subagent",
      subagent_type: "Explore",
      background: false,
      parent_task_id: "a0",
    });
    expect(events[1]).toMatchObject({ type: "task_progress", activity: null, last_tool: "Grep", summary: "Reading the router" });
    const [step] = mapAll([task.progress("a1", { description: "Running Wait 15 seconds" })], (() => {
      const mapper = new ClaudeEventMapper();
      mapper.map(task.started("a1", { task_type: "local_agent", description: "Explore the repo" }));
      return mapper;
    })());
    expect(step).toMatchObject({ type: "task_progress", activity: "Running Wait 15 seconds" });
  });

  test("workflows and MCP tasks are other tasks", () => {
    const [started] = mapAll([task.started("w1", { task_type: "local_workflow", is_backgrounded: undefined, description: "" , workflow_name: "spec" })]);
    expect(started).toMatchObject({ kind: "other", description: "spec", background: true });
  });

  test("ambient and skip_transcript tasks are dropped, with everything that follows about them", () => {
    const events = mapAll([
      task.started("w1", { ambient: true }),
      task.started("w2", { skip_transcript: true }),
      task.progress("w1"),
      task.updated("w2", { status: "killed" }),
      task.notification("w1", "completed"),
    ]);
    expect(events).toEqual([]);
  });

  test("a foreground command shows once moved to the background", () => {
    const mapper = new ClaudeEventMapper();
    expect(mapAll([task.started("f1", { is_backgrounded: false }), task.progress("f1")], mapper)).toEqual([]);
    const moved = mapAll([task.updated("f1", { is_backgrounded: true })], mapper);
    expect(moved).toEqual([expect.objectContaining({ type: "task_started", task_id: "f1", kind: "command", background: true })]);
    expect(mapper.tasks.running("f1")).toBe(true);
  });

  test("a foreground command that ends in the foreground is never shown", () => {
    const events = mapAll([task.started("f1", { is_backgrounded: false, tool_use_id: "toolu_9" }), sdk.toolResult("toolu_9", "ok"), sdk.result()]);
    expect(events.map((e) => e.type)).toEqual(["tool_result", "turn_ended"]);
  });

  test("a status patch ends a task; the notification after it adds nothing", () => {
    const events = mapAll([task.started("b1"), task.updated("b1", { status: "killed" }), task.notification("b1", "stopped")]);
    expect(events.map((e) => e.type)).toEqual(["task_started", "task_ended"]);
    expect(events[1]).toEqual({ type: "task_ended", task_id: "b1", status: "stopped", summary: null });
    const failed = mapAll([task.started("b2"), task.updated("b2", { status: "failed", error: "exit 1" })]);
    expect(failed[1]).toEqual({ type: "task_ended", task_id: "b2", status: "failed", summary: "exit 1" });
  });

  test("a renamed task is sent again with its new description", () => {
    const events = mapAll([task.started("b1"), task.updated("b1", { description: "Run the e2e suite" })]);
    expect(events[1]).toMatchObject({ type: "task_started", task_id: "b1", description: "Run the e2e suite" });
  });

  test("a foreground subagent ends with its tool result, or at the latest with the turn", () => {
    const fg = { task_type: "local_agent", is_backgrounded: false };
    const byResult = mapAll([task.started("a1", { ...fg, tool_use_id: "toolu_a" }), sdk.toolResult("toolu_a", "found it", true)]);
    expect(byResult.at(-1)).toEqual({ type: "task_ended", task_id: "a1", status: "failed", summary: null });
    const byTurn = mapAll([task.started("a2", fg), task.started("b1"), sdk.result()]);
    expect(byTurn.map((e) => e.type)).toEqual(["task_started", "task_started", "task_ended", "turn_ended"]);
    expect(byTurn[2]).toMatchObject({ task_id: "a2", status: "completed" });
  });

  test("a background task outlives the turn and its placeholder tool result", () => {
    const events = mapAll([task.started("b1", { tool_use_id: "toolu_b" }), sdk.toolResult("toolu_b", "Started in the background"), sdk.result()]);
    expect(events.map((e) => e.type)).toEqual(["task_started", "tool_result", "turn_ended"]);
  });

  test("ending the session stops every task still running", () => {
    const mapper = new ClaudeEventMapper();
    mapAll([task.started("b1"), task.started("b2"), task.notification("b2", "completed"), task.started("f1", { is_backgrounded: false })], mapper);
    expect(mapper.tasks.endAll("gone")).toEqual([{ type: "task_ended", task_id: "b1", status: "stopped", summary: "gone" }]);
    expect(mapper.tasks.endAll("gone")).toEqual([]);
  });
});

const BASE: SessionStartParams = {
  chat_id: "chat-1",
  workspace_id: "ws-1",
  agent: "claude",
  cwd: "/workspaces/ws-1",
  additional_directories: [],
  fast: false,
  prompt_attachments: [],
  permission_mode: "default",
  read_only: false,
  prompt_suggestions: false,
  env: {},
};

async function started() {
  const host = new FakeHost();
  let query: FakeQuery | undefined;
  const session = new ClaudeSession(BASE, host, {
    query: ({ prompt, options }) => (query = new FakeQuery(prompt, options)),
    resolveExecutable: () => "/opt/claude",
    baseEnv: {},
    initTimeoutMs: 200,
    stopTimeoutMs: 200,
  });
  await session.start();
  return { session, host, query: query as FakeQuery };
}

describe("a session's tasks", () => {
  test("a running task is stopped through the SDK, which reports it stopped", async () => {
    const { session, host, query } = await started();
    query.push(task.started("b1"));
    await flush();
    await session.stopTask("b1");
    expect(query.calls).toContain("stopTask:b1");
    query.push(task.notification("b1", "stopped", "Stopped by the user"));
    await flush();
    expect(host.events.filter((e) => e.type.startsWith("task_"))).toEqual([
      expect.objectContaining({ type: "task_started", task_id: "b1" }),
      { type: "task_ended", task_id: "b1", status: "stopped", summary: "Stopped by the user" },
    ]);
  });

  test("stopping a task that is not running, or a hidden one, is refused", async () => {
    const { session, query } = await started();
    query.push(task.started("w1", { ambient: true }), task.started("b1"), task.notification("b1", "completed"));
    await flush();
    for (const id of ["w1", "b1", "nope"]) {
      const error = await session.stopTask(id).catch((e: unknown) => e);
      expect(error).toBeInstanceOf(RpcError);
      expect((error as RpcError).code).toBe(ErrorCode.NOT_FOUND);
    }
    expect(query.calls.filter((c) => c.startsWith("stopTask"))).toEqual([]);
  });

  test("an SDK failure to stop is reported", async () => {
    const { session, query } = await started();
    query.push(task.started("b1"));
    await flush();
    query.stopTaskError = new Error("no such task");
    const error = (await session.stopTask("b1").catch((e: unknown) => e)) as RpcError;
    expect(error.code).toBe(ErrorCode.INTERNAL);
    expect(error.message).toContain("no such task");
  });

  test("closing the session ends its tasks", async () => {
    const { session, host, query } = await started();
    query.push(task.started("b1"));
    await flush();
    await session.stop();
    expect(host.events.at(-1)).toEqual({ type: "task_ended", task_id: "b1", status: "stopped", summary: "The agent session was closed" });
  });

  test("a CLI that exits ends its tasks", async () => {
    const { host, query } = await started();
    query.push(task.started("b1"));
    await flush();
    query.exit();
    await flush();
    expect(host.events).toContainEqual({ type: "task_ended", task_id: "b1", status: "stopped", summary: "The agent process stopped" });
    expect(host.endedCount).toBe(1);
  });

  test("an interrupted turn ends its foreground subagent; background ones keep running", async () => {
    const { session, host, query } = await started();
    session.send("go");
    await query.waitForSent(1);
    query.push(
      task.started("bg", { task_type: "local_agent", subagent_type: "general-purpose", is_backgrounded: true }),
      task.started("fg", { task_type: "local_agent", subagent_type: "Explore", is_backgrounded: false }),
      {
        type: "system",
        subtype: "background_tasks_changed",
        tasks: [{ task_id: "bg", task_type: "local_agent", description: "x" }],
        uuid: "u",
        session_id: "s",
      } as never,
    );
    await flush();
    await session.interrupt();
    query.push(sdk.result({ subtype: "error_during_execution", is_error: true, errors: ["Request was aborted"] }));
    await flush();
    expect(host.events.filter((e) => e.type === "task_ended")).toEqual([
      { type: "task_ended", task_id: "fg", status: "completed", summary: null },
    ]);
    expect(host.events.at(-1)).toMatchObject({ type: "turn_ended", outcome: "interrupted" });
    // Still stoppable, and the session stays open for it.
    await session.stopTask("bg");
    expect(query.calls).toContain("stopTask:bg");
    expect(session.idle()).toBe(false);
  });

  test("background tasks still keep an idle session open", async () => {
    const { session, query } = await started();
    query.push(task.started("b1"), {
      type: "system",
      subtype: "background_tasks_changed",
      tasks: [{ task_id: "b1", task_type: "local_bash", description: "x" }],
      uuid: "u",
      session_id: "s",
    } as never);
    await flush();
    expect(session.idle()).toBe(false);
  });
});
