import { describe, expect, test } from "bun:test";
import type { ChatEvent, EventEnvelope } from "@chauffeur/protocol";
import { buildChatView, type ChatItem, reduceChat } from "./chat";
import {
  liveItems,
  mergeConversation,
  panelStack,
  parseAgentResult,
  subagentSummary,
  targetToolUse,
  transcriptItems,
  withoutReport,
} from "./subagent";

let seq = 0;
function p(event: ChatEvent, ts = "2026-10-18T10:00:00Z"): EventEnvelope {
  seq += 1;
  return { seq, ts, event: { scope: "chat", workspace_id: "w", chat_id: "c", event } };
}
function e(event: ChatEvent): EventEnvelope {
  return { seq: null, ts: "2026-10-18T10:00:01Z", event: { scope: "chat", workspace_id: "w", chat_id: "c", event } };
}

const REPORT = "Lighthouses guide ships.";
const FOREGROUND_RESULT = `${REPORT}\nagentId: a1 (use SendMessage with to: 'a1', summary: '<5-10 word recap>' to continue this agent)\n<usage>subagent_tokens: 1200\ntool_uses: 2\nduration_ms: 4200</usage>`;

/** A chat where the agent ran a foreground subagent, as its events come. */
function chat(): EventEnvelope[] {
  return [
    p({ type: "turn_started" }),
    p({
      type: "tool_call",
      tool_use_id: "toolu_a",
      name: "Agent",
      input: { description: "Lighthouses", prompt: "Write one sentence", subagent_type: "general-purpose" },
      parent_tool_use_id: null,
    }),
    p({
      type: "task_started",
      task_id: "a1",
      kind: "subagent",
      description: "Lighthouses",
      subagent_type: "general-purpose",
      tool_use_id: "toolu_a",
      background: false,
      parent_task_id: null,
    }, "2026-10-18T10:00:00Z"),
    e({ type: "assistant_text", message_id: "m:0", text: "Thinking about lighthouses.", parent_tool_use_id: "toolu_a" }),
    p({ type: "tool_call", tool_use_id: "toolu_b", name: "Bash", input: { command: "echo hi" }, parent_tool_use_id: "toolu_a" }),
    p({ type: "tool_result", tool_use_id: "toolu_b", output: "hi", is_error: false }),
    // A nested subagent: its own steps are its own.
    p({ type: "tool_call", tool_use_id: "toolu_n", name: "Agent", input: { description: "Nested", prompt: "Look" }, parent_tool_use_id: "toolu_a" }),
    p({ type: "tool_call", tool_use_id: "toolu_nb", name: "Read", input: { file_path: "/w/x" }, parent_tool_use_id: "toolu_n" }),
    e({ type: "assistant_text", message_id: "m:1", text: REPORT, parent_tool_use_id: "toolu_a" }),
  ];
}

describe("a subagent's view", () => {
  test("live: its tool calls from the chat and its text, in the order they came", () => {
    const view = buildChatView(chat());
    const items = liveItems(view, "toolu_a");
    expect(items.map((i) => (i.kind === "tool" ? `tool:${i.name}` : `${i.kind}:${(i as { text: string }).text}`))).toEqual([
      "assistant:Thinking about lighthouses.",
      "tool:Bash",
      "tool:Agent",
      `assistant:${REPORT}`,
    ]);
    expect(items.every((i) => i.depth === 0)).toBe(true);
    // The nested subagent's view has its own steps.
    expect(liveItems(view, "toolu_n").map((i) => i.kind === "tool" && i.name)).toEqual(["Read"]);
  });

  test("while it runs: status, prompt, header, and the time it has run", () => {
    const view = buildChatView(chat());
    const now = Date.parse("2026-10-18T10:00:30Z");
    const summary = subagentSummary(view, { toolUseId: "toolu_a", agentId: null }, liveItems(view, "toolu_a"), now);
    expect(summary).toMatchObject({
      type: "general-purpose",
      description: "Lighthouses",
      prompt: "Write one sentence",
      status: "running",
      taskId: "a1",
      durationMs: 30_000,
      report: null,
      background: false,
    });
  });

  test("once done: its report from the Agent call's result, how long and how many tools", () => {
    let view = buildChatView(chat());
    view = reduceChat(view, p({ type: "tool_result", tool_use_id: "toolu_a", output: FOREGROUND_RESULT, is_error: false }));
    view = reduceChat(view, p({ type: "task_ended", task_id: "a1", status: "completed", summary: null }));
    const conversation = liveItems(view, "toolu_a");
    const summary = subagentSummary(view, { toolUseId: "toolu_a", agentId: null }, conversation, Date.now());
    expect(summary).toMatchObject({ status: "completed", report: REPORT, durationMs: 4200, toolUses: 2 });
    // The report is shown once, not again as its last message.
    expect(withoutReport(conversation, summary.report).map((i) => i.kind)).toEqual(["assistant", "tool", "tool"]);
  });

  test("opened from the tasks bar by its agent id, it finds its Agent call", () => {
    const view = buildChatView(chat());
    expect(targetToolUse(view, { toolUseId: null, agentId: "a1" })).toBe("toolu_a");
    expect(targetToolUse(view, { toolUseId: null, agentId: "unknown" })).toBeNull();
  });

  test("a background subagent: its launch message is no report, its last message is", () => {
    let view = buildChatView([
      p({ type: "tool_call", tool_use_id: "toolu_bg", name: "Agent", input: { description: "Bg", prompt: "Go", run_in_background: true }, parent_tool_use_id: null }),
      p({ type: "tool_result", tool_use_id: "toolu_bg", output: "Async agent launched successfully.\nagentId: abg (internal ID - do not mention to user.)", is_error: false }),
    ]);
    const transcript = transcriptItems([{ type: "assistant_text", message_id: "x:0", text: "All done.", parent_tool_use_id: null }]);
    const summary = subagentSummary(view, { toolUseId: "toolu_bg", agentId: null }, transcript, Date.now());
    expect(summary).toMatchObject({ status: "completed", report: "All done.", background: true });
    view = reduceChat(view, p({ type: "turn_started" }));
    expect(parseAgentResult("Async agent launched successfully.\nagentId: abg (x)")).toEqual({ report: null, agentId: "abg", toolUses: null, durationMs: null });
  });

  test("an older chat without task events: done or failed by its result", () => {
    const view = buildChatView([
      p({ type: "tool_call", tool_use_id: "toolu_old", name: "Task", input: { description: "Old", prompt: "Go" }, parent_tool_use_id: null }),
      p({ type: "tool_result", tool_use_id: "toolu_old", output: "boom", is_error: true }),
    ]);
    expect(subagentSummary(view, { toolUseId: "toolu_old", agentId: null }, [], Date.now())).toMatchObject({ status: "failed", failed: true, report: "boom" });
    const unfinished = buildChatView([
      p({ type: "tool_call", tool_use_id: "toolu_u", name: "Agent", input: { prompt: "Go" }, parent_tool_use_id: null }),
    ]);
    expect(subagentSummary(unfinished, { toolUseId: "toolu_u", agentId: null }, [], Date.now()).status).toBe("stopped");
  });
});

describe("the transcript", () => {
  const events: ChatEvent[] = [
    { type: "thinking", message_id: "m:9", text: "Hmm." },
    { type: "assistant_text", message_id: "m:0", text: "Thinking about lighthouses.", parent_tool_use_id: null },
    { type: "tool_call", tool_use_id: "toolu_b", name: "Bash", input: { command: "echo hi" }, parent_tool_use_id: null },
  ];

  test("becomes items keyed apart from the chat's", () => {
    const items = transcriptItems(events);
    expect(items.map((i) => i.kind)).toEqual(["thinking", "assistant", "tool"]);
    expect(items.every((i) => i.key.startsWith("t-") && i.depth === 0)).toBe(true);
  });

  test("merges with what came live: no duplicates, live results and later steps added", () => {
    const view = buildChatView(chat());
    const merged = mergeConversation(transcriptItems(events), liveItems(view, "toolu_a"));
    const labels = merged.map((i: ChatItem) => (i.kind === "tool" ? `tool:${i.toolUseId}:${i.result ? "done" : "pending"}` : `${i.kind}:${(i as { messageId: string }).messageId}`));
    expect(labels).toEqual(["thinking:m:9", "assistant:m:0", "tool:toolu_b:done", "tool:toolu_n:pending", "assistant:m:1"]);
    // Without a transcript (not found), the live view alone.
    expect(mergeConversation(null, liveItems(view, "toolu_a"))).toHaveLength(4);
  });
});

describe("an Agent call's result", () => {
  test("its report without the trailer for the main agent", () => {
    expect(parseAgentResult(FOREGROUND_RESULT)).toEqual({ report: REPORT, agentId: "a1", toolUses: 2, durationMs: 4200 });
    expect(parseAgentResult("[Subagent hand-back] PONG").report).toBe("PONG");
    expect(parseAgentResult("Fork started — processing in background").report).toBeNull();
  });

  test("handed back through SubagentHandback: the report is the call's message, the result only points to it", () => {
    const pointer = 'This agent\'s report was delivered to you as a message from "a84b4b71b9b5b3e0a" (its SubagentHandback call). Read it there; it is not repeated here.';
    expect(parseAgentResult(pointer)).toMatchObject({ report: null, agentId: "a84b4b71b9b5b3e0a" });
    const view = buildChatView([
      p({ type: "tool_call", tool_use_id: "toolu_h", name: "Agent", input: { description: "H", prompt: "Go" }, parent_tool_use_id: null }),
      p({ type: "tool_call", tool_use_id: "toolu_hb", name: "SubagentHandback", input: { message: "All three paragraphs written." }, parent_tool_use_id: "toolu_h" }),
      p({ type: "tool_result", tool_use_id: "toolu_h", output: pointer, is_error: false }),
    ]);
    const summary = subagentSummary(view, { toolUseId: "toolu_h", agentId: null }, liveItems(view, "toolu_h"), Date.now());
    expect(summary).toMatchObject({ status: "completed", report: "All three paragraphs written." });
  });
});

describe("the panel", () => {
  const a = { toolUseId: "toolu_a", agentId: null };
  const n = { toolUseId: "toolu_n", agentId: null };
  const bar = { toolUseId: "toolu_x", agentId: "ax" };

  test("opens one, goes into a nested one and back, closes", () => {
    let stack = panelStack([], { type: "open", target: a });
    expect(stack).toEqual([a]);
    stack = panelStack(stack, { type: "push", target: n });
    expect(stack).toEqual([a, n]);
    expect(panelStack(stack, { type: "push", target: n })).toBe(stack);
    expect(panelStack(stack, { type: "back", index: 0 })).toEqual([a]);
    // Opening another from the chat or the tasks bar starts over.
    expect(panelStack(stack, { type: "open", target: bar })).toEqual([bar]);
    expect(panelStack(stack, { type: "close" })).toEqual([]);
  });
});
