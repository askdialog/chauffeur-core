import { describe, expect, test } from "bun:test";
import type { ChatEvent, ChatInfo, ContextUsage, EventEnvelope } from "@chauffeur/protocol";
import { formatElapsed, formatTokens } from "./format";
import {
  type ActivityGroup,
  activitySummary,
  type AssistantItem,
  buildChatView,
  delegatedAgent,
  delegatedChat,
  DELEGATE_TOOL,
  groupActivity,
  emptyChatView,
  openRequests,
  type PermissionItem,
  permissionTitle,
  repoProposal,
  repoProposalOutcome,
  ruleLabel,
  type QuestionItem,
  reduceChat,
  type ThinkingItem,
  type ToolItem,
  type TaskView,
  taskStep,
  toolSummary,
  type TurnItem,
  type UserItem,
} from "./chat";

let seq = 0;
/** A persisted chat event. */
function p(event: ChatEvent): EventEnvelope {
  seq += 1;
  return { seq, ts: `2026-10-13T10:00:${String(seq % 60).padStart(2, "0")}Z`, event: { scope: "chat", workspace_id: "w", chat_id: "c", event } };
}
/** An ephemeral chat event. */
function e(event: ChatEvent): EventEnvelope {
  return { seq: null, ts: "2026-10-13T10:00:00Z", event: { scope: "chat", workspace_id: "w", chat_id: "c", event } };
}

describe("reduceChat", () => {
  test("switching provider keeps the conversation and cost, clears context and disables old session forks", () => {
    const view = buildChatView([
      p({ type: "user_message", text: "keep this", queued: false }),
      p({ type: "session_started", agent_session_id: "claude-1", model: "haiku" }),
      p({ type: "assistant_text", message_id: "m", text: "prior reply", parent_tool_use_id: null }),
      p({ type: "turn_ended", outcome: "success", duration_ms: 1, cost_usd: 0.25, context_tokens: null, error: null, fork_point: { session_id: "claude-1", up_to: "u1" } }),
      p({ type: "agent_changed", from: "claude", to: "codex" }),
    ]);
    expect(view.sessionId).toBeNull();
    expect(view.context).toBeNull();
    expect(view.reportedCostUsd).toBe(0);
    expect(view.costUsd).toBe(0.25);
    expect(view.items.some(i => i.kind === "assistant" && i.text === "prior reply")).toBe(true);
    expect(view.items.filter(i => i.kind === "turn").every(i => !i.forkable)).toBe(true);
    expect(view.items.at(-1)).toMatchObject({ kind: "notice", message: expect.stringContaining("Switched from Claude Code to Codex") });
    const next = reduceChat(view, p({ type: "turn_ended", outcome: "success", duration_ms: 1, cost_usd: 0.1, context_tokens: null, error: null, fork_point: { session_id: "codex-1", up_to: "u2" } }));
    expect(next.items.at(-1)).toMatchObject({ kind: "turn", forkable: true });
    expect(next.costUsd).toBeCloseTo(0.35);
  });

  test("a user message keeps its images; older events have none", () => {
    const shot = { file: "0192-shot.png", name: "Screenshot.png", media_type: "image/png", size: 10, path: "/w/.chauffeur/attachments/0192-shot.png" };
    const view = buildChatView([
      p({ type: "user_message", text: "look", queued: false, attachments: [shot] }),
      p({ type: "user_message", text: "old", queued: false }),
    ]);
    expect(view.items.map((i) => (i as UserItem).attachments)).toEqual([[shot], []]);
  });

  test("a shell command shows running in place, then with its output; a replay shows it ended", () => {
    const started = e({ type: "shell_started", shell_id: "sh1", command: "npm test" });
    const ended = p({ type: "shell_ended", shell_id: "sh1", command: "npm test", exit_code: 1, output: "1 failed\n", timed_out: false });
    const running = buildChatView([started]);
    expect(running.items).toMatchObject([{ kind: "shell", command: "npm test", running: true }]);
    const done = reduceChat(running, ended);
    expect(done.items).toHaveLength(1);
    expect(done.items[0]).toMatchObject({ kind: "shell", key: running.items[0].key, running: false, exitCode: 1, output: "1 failed\n" });
    expect(buildChatView([ended]).items).toMatchObject([{ kind: "shell", running: false, exitCode: 1 }]);
  });

  test("streamed text deltas are replaced by the final assistant_text with the same message id", () => {
    const streaming = [
      p({ type: "user_message", text: "hi", queued: false }),
      p({ type: "turn_started" }),
      e({ type: "text_delta", message_id: "m1", text: "Hel" }),
      e({ type: "text_delta", message_id: "m1", text: "lo **wor" }),
    ];
    let view = buildChatView(streaming);
    expect(view.working).toBe(true);
    expect(view.items.map((i) => i.kind)).toEqual(["user", "assistant"]);
    const partial = view.items[1] as AssistantItem;
    expect(partial.text).toBe("Hello **wor");
    expect(partial.streaming).toBe(true);

    view = reduceChat(view, p({ type: "assistant_text", message_id: "m1", text: "Hello **world**", parent_tool_use_id: null }));
    const final = view.items[1] as AssistantItem;
    expect(view.items).toHaveLength(2);
    expect(final.text).toBe("Hello **world**");
    expect(final.streaming).toBe(false);
    // Same list key: the row is updated in place, not re-created.
    expect(final.key).toBe(partial.key);
  });

  test("a replay without deltas gives the same messages", () => {
    const live = buildChatView([
      p({ type: "turn_started" }),
      e({ type: "text_delta", message_id: "m1", text: "Hi" }),
      p({ type: "assistant_text", message_id: "m1", text: "Hi there", parent_tool_use_id: null }),
    ]);
    const replay = buildChatView(
      [p({ type: "turn_started" }), p({ type: "assistant_text", message_id: "m1", text: "Hi there", parent_tool_use_id: null })],
    );
    const text = (v: typeof live) => v.items.map((i) => (i.kind === "assistant" ? i.text : i.kind));
    expect(text(live)).toEqual(text(replay));
  });

  test("a second text block of the same message gets its own row", () => {
    const view = buildChatView([
      e({ type: "text_delta", message_id: "m1", text: "first" }),
      p({ type: "assistant_text", message_id: "m1", text: "first", parent_tool_use_id: null }),
      p({ type: "tool_call", tool_use_id: "t1", name: "Read", input: { file_path: "/a.ts" }, parent_tool_use_id: null }),
      e({ type: "text_delta", message_id: "m1", text: "sec" }),
      p({ type: "assistant_text", message_id: "m1", text: "second", parent_tool_use_id: null }),
    ]);
    expect(view.items.map((i) => i.kind)).toEqual(["assistant", "tool", "assistant"]);
    expect((view.items[2] as AssistantItem).text).toBe("second");
    expect(new Set(view.items.map((i) => i.key)).size).toBe(3);
  });

  test("thinking deltas collapse into the final thinking block", () => {
    const view = buildChatView([
      e({ type: "thinking_delta", message_id: "m1", text: "Let me " }),
      e({ type: "thinking_delta", message_id: "m1", text: "think" }),
      p({ type: "thinking", message_id: "m1", text: "Let me think." }),
    ]);
    expect(view.items).toHaveLength(1);
    const item = view.items[0] as ThinkingItem;
    expect(item.kind).toBe("thinking");
    expect(item.text).toBe("Let me think.");
    expect(item.streaming).toBe(false);
  });

  test("tool results attach to their call", () => {
    const view = buildChatView([
      p({ type: "tool_call", tool_use_id: "t1", name: "Bash", input: { command: "ls -la\necho hi" }, parent_tool_use_id: null }),
      p({ type: "tool_call", tool_use_id: "t2", name: "Read", input: { file_path: "/x/y.rs" }, parent_tool_use_id: null }),
      p({ type: "tool_result", tool_use_id: "t1", output: "total 0", is_error: false }),
      p({ type: "tool_result", tool_use_id: "t2", output: "no such file", is_error: true }),
    ]);
    const [bash, read] = view.items as ToolItem[];
    expect(bash.summary).toBe("ls -la");
    expect(bash.result).toEqual({ output: "total 0", isError: false, attachments: [] });
    expect(read.summary).toBe("/x/y.rs");
    expect(read.result?.isError).toBe(true);
  });

  test("a tool result keeps the images the daemon saved", () => {
    const shot = { file: "0193-shot.png", name: "image.png", media_type: "image/png", size: 10, path: "/ws/.chauffeur/attachments/0193-shot.png" };
    const view = buildChatView([
      p({ type: "tool_call", tool_use_id: "t1", name: "mcp__playwright__browser_take_screenshot", input: {}, parent_tool_use_id: null }),
      p({ type: "tool_result", tool_use_id: "t1", output: "[image]", is_error: false, attachments: [shot] }),
    ]);
    expect((view.items[0] as ToolItem).result?.attachments).toEqual([shot]);
  });

  test("subagent calls are nested under the Task call; its text goes to its own view", () => {
    const view = buildChatView([
      p({ type: "tool_call", tool_use_id: "task", name: "Task", input: { description: "Explore", prompt: "…" }, parent_tool_use_id: null }),
      p({ type: "tool_call", tool_use_id: "g", name: "Grep", input: { pattern: "foo", path: "src" }, parent_tool_use_id: "task" }),
      p({ type: "assistant_text", message_id: "sub", text: "found it", parent_tool_use_id: "task" }),
      p({ type: "tool_result", tool_use_id: "task", output: "done", is_error: false }),
    ]);
    expect(view.items.map((i) => i.depth)).toEqual([0, 1]);
    expect(view.subagents.task?.items.map((i) => [i.kind, i.text, i.anchor])).toEqual([["assistant", "found it", 2]]);
    expect((view.items[0] as ToolItem).summary).toBe("Explore");
    expect((view.items[1] as ToolItem).summary).toBe("foo in src");
  });

  test("a fork's hand-back replaces its started placeholder; its own steps nest under the Agent call", () => {
    // As Claude Code 2.1.302 sends it: a placeholder result, the fork's work under the call, then
    // the hand-back as a second result for the same call.
    const view = buildChatView([
      p({ type: "tool_call", tool_use_id: "fork", name: "Agent", input: { subagent_type: "fork", description: "slow pong", prompt: "…" }, parent_tool_use_id: null }),
      p({ type: "tool_result", tool_use_id: "fork", output: "Fork started — processing in background", is_error: false }),
      p({ type: "tool_call", tool_use_id: "b", name: "Bash", input: { command: "sleep 8" }, parent_tool_use_id: "fork" }),
      p({ type: "assistant_text", message_id: "sub", text: "PONG", parent_tool_use_id: "fork" }),
      p({ type: "tool_result", tool_use_id: "fork", output: "[Subagent hand-back] PONG", is_error: false }),
    ]);
    expect(view.items.map((i) => [i.kind, i.depth])).toEqual([["tool", 0], ["tool", 1]]);
    expect(view.subagents.fork?.items.map((i) => i.text)).toEqual(["PONG"]);
    expect((view.items[0] as ToolItem).summary).toBe("slow pong");
    expect((view.items[0] as ToolItem).result?.output).toBe("[Subagent hand-back] PONG");
  });

  test("permission requests open, resolve, and expire at the end of the turn", () => {
    let view = buildChatView([
      p({ type: "turn_started" }),
      p({ type: "permission_request", request_id: "r1", tool_name: "Bash", input: { command: "rm -rf build" }, title: null }),
      p({ type: "status_changed", status: "needs_input" }),
    ]);
    expect(view.status).toBe("needs_input");
    expect(openRequests(view).map((r) => r.requestId)).toEqual(["r1"]);
    expect((view.items[0] as PermissionItem).summary).toBe("rm -rf build");

    view = reduceChat(view, p({ type: "request_resolved", request_id: "r1", outcome: "allowed" }));
    expect(openRequests(view)).toHaveLength(0);
    expect((view.items[0] as PermissionItem).outcome).toBe("allowed");
    expect((view.items[0] as PermissionItem).rules).toBeNull();

    view = reduceChat(view, p({ type: "permission_request", request_id: "r2", tool_name: "Write", input: { file_path: "/a" }, title: "Write /a?" }));
    view = reduceChat(view, p({ type: "turn_ended", outcome: "interrupted", duration_ms: 1200, cost_usd: 0.01, context_tokens: null, error: null }));
    const r2 = view.items.find((i) => i.kind === "permission" && i.requestId === "r2") as PermissionItem;
    expect(r2.state).toBe("expired");
    expect(view.working).toBe(false);
  });

  test("a Codex rule names the workspace's own folder plainly", () => {
    expect(ruleLabel("Bash(make) in /ws/foo", "/ws/foo/")).toBe("Bash(make) in the workspace folder");
    expect(ruleLabel("Bash(make) in /private/tmp/ws", "/tmp/ws")).toBe("Bash(make) in the workspace folder");
    expect(ruleLabel("Bash(make) in /ws/foo/alpha", "/ws/foo")).toBe("Bash(make) in /ws/foo/alpha");
    expect(ruleLabel("Bash(npm test:*)", "/ws/foo")).toBe("Bash(npm test:*)");
    expect(ruleLabel("Bash(make) in /ws/foo", null)).toBe("Bash(make) in /ws/foo");
  });

  test("a request allowed always keeps the rules the chat remembers", () => {
    const view = buildChatView([
      p({ type: "permission_request", request_id: "r1", tool_name: "mcp__notes__write_note", input: { text: "a" }, title: null }),
      p({ type: "permission_request", request_id: "r2", tool_name: "mcp__notes__write_note", input: { text: "b" }, title: null }),
      p({ type: "request_resolved", request_id: "r1", outcome: "allowed", rules: ["mcp__notes__write_note"] }),
      p({ type: "request_resolved", request_id: "r2", outcome: "allowed", rules: ["mcp__notes__write_note"] }),
    ]);
    expect(view.items.map((i) => (i as PermissionItem).rules)).toEqual([["mcp__notes__write_note"], ["mcp__notes__write_note"]]);
    expect(openRequests(view)).toHaveLength(0);
  });

  test("a subagent's permission request names it; the agent's own and older ones name none", () => {
    const agent = { id: "a1", subagent_type: "general-purpose", description: "Find the flaky tests", background: true };
    const view = buildChatView([
      p({ type: "permission_request", request_id: "r1", tool_name: "Bash", input: { command: "ls" }, title: "Claude wants to run ls", agent }),
      p({ type: "permission_request", request_id: "r2", tool_name: "Bash", input: { command: "ls" }, title: "Claude wants to run ls" }),
    ]);
    const [sub, own] = view.items as PermissionItem[];
    expect(sub.agent).toEqual(agent);
    expect(own.agent).toBeNull();
    expect(permissionTitle(sub)).toBe("general-purpose (background) wants to run ls");
    expect(permissionTitle(own)).toBe("Claude wants to run ls");
    expect(permissionTitle({ ...sub, title: null })).toBe("general-purpose (background) wants to use Bash");
    expect(permissionTitle({ ...sub, title: "Run the migration?" })).toBe("general-purpose (background): Run the migration?");
    expect(permissionTitle({ ...own, title: null })).toBe("Allow Bash?");
    expect(permissionTitle({ ...sub, agent: { id: "a2", subagent_type: null, description: null, background: false } })).toBe(
      "A subagent wants to run ls",
    );
  });

  test("questions keep their options and resolve", () => {
    let view = buildChatView([
      p({
        type: "question",
        request_id: "q1",
        questions: [
          {
            question: "Which DB?",
            header: "DB",
            multi_select: false,
            options: [
              { label: "Postgres", description: "" },
              { label: "SQLite", description: "small" },
            ],
          },
        ],
      }),
    ]);
    const item = view.items[0] as QuestionItem;
    expect(item.questions[0].options.map((o) => o.label)).toEqual(["Postgres", "SQLite"]);
    expect(item.state).toBe("open");
    view = reduceChat(view, p({ type: "request_resolved", request_id: "q1", outcome: "answered" }));
    expect((view.items[0] as QuestionItem).state).toBe("resolved");
  });

  test("the answers come back from the AskUserQuestion tool result", () => {
    const view = buildChatView([
      p({ type: "tool_call", tool_use_id: "ask", name: "AskUserQuestion", input: {}, parent_tool_use_id: null }),
      p({
        type: "question",
        request_id: "q",
        questions: [
          { question: "Which color?", header: "", multi_select: false, options: [{ label: "Red", description: "" }] },
          { question: 'Say "hi"?', header: "", multi_select: false, options: [] },
        ],
      }),
      p({ type: "request_resolved", request_id: "q", outcome: "answered" }),
      p({
        type: "tool_result",
        tool_use_id: "ask",
        output: 'Your questions have been answered: "Which color?"="Red", "Say \\"hi\\"?"="no". You can now continue.',
        is_error: false,
      }),
    ]);
    expect((view.items[1] as QuestionItem).answers).toEqual({ "Which color?": "Red", 'Say "hi"?': "no" });
  });

  test("an AskUserQuestion call is shown through its question only, whichever comes first", () => {
    const questions = [{ question: "Which color?", header: "", multi_select: false, options: [] }];
    const input = { questions: [{ question: "Which color?" }] };
    const call = () => p({ type: "tool_call", tool_use_id: "ask", name: "AskUserQuestion", input, parent_tool_use_id: null });
    const asked = () => p({ type: "question", request_id: "q", questions });
    for (const order of [[call, asked], [asked, call]]) {
      const tool = buildChatView(order.map((make) => make())).items.find((i) => i.kind === "tool") as ToolItem;
      expect(tool.asked).toBe(true);
    }
    // Claude Code answered the call itself: nothing asked, the call stays visible.
    const alone = buildChatView([call()]).items[0] as ToolItem;
    expect(alone.asked).toBe(false);
  });

  test("Codex estimates add per turn and remain estimated beside a fork's Claude costs", () => {
    let view = emptyChatView;
    const ended = { type: "turn_ended", outcome: "success", duration_ms: 1, cost_usd: null, context_tokens: null, error: null } as const;
    view = reduceChat(view, p({ type: "session_started", agent_session_id: "claude-original", model: null }));
    view = reduceChat(view, p({ ...ended, cost_usd: 0.05 }));
    view = reduceChat(view, p({ type: "session_started", agent_session_id: "codex-fork", model: null }));
    view = reduceChat(view, p({ ...ended, estimated_cost_usd: 0.2 }));
    view = reduceChat(view, p({ ...ended, estimated_cost_usd: 0.3 }));
    view = reduceChat(view, p({ type: "session_started", agent_session_id: "claude-fork", model: null }));
    view = reduceChat(view, p({ ...ended, cost_usd: 0.1 }));
    view = reduceChat(view, p({ ...ended, cost_usd: 0.15 }));
    expect(view.costUsd).toBeCloseTo(0.7);
    expect(view.costEstimated).toBe(true);
    expect(view.items.filter((item) => item.kind === "turn").map((item) => item.costEstimated)).toEqual([false, true, true, false, false]);
  });

  test("turn summaries carry duration and cost, and close streaming rows", () => {
    let view = buildChatView([
      p({ type: "user_message", text: "go", queued: false }),
      p({ type: "turn_started" }),
      e({ type: "text_delta", message_id: "m", text: "partial" }),
    ]);
    expect(view.turnStartedAt).not.toBeNull();
    view = reduceChat(view, p({ type: "turn_ended", outcome: "success", duration_ms: 4200, cost_usd: 0.25, context_tokens: 12000, error: null }));
    view = reduceChat(view, p({ type: "turn_ended", outcome: "error", duration_ms: 10, cost_usd: 0.3, context_tokens: null, error: "boom" }));
    // The agent session restarted: its running total starts again.
    view = reduceChat(view, p({ type: "turn_ended", outcome: "success", duration_ms: 10, cost_usd: 0.02, context_tokens: null, error: null }));
    const turns = view.items.filter((i): i is TurnItem => i.kind === "turn");
    expect(turns.map((t) => t.outcome)).toEqual(["success", "error", "success"]);
    expect(turns[1].error).toBe("boom");
    // cost_usd is the session's running total: each turn shows its own share.
    expect(turns.map((t) => Number(t.costUsd?.toFixed(2)))).toEqual([0.25, 0.05, 0.02]);
    expect(view.costUsd).toBeCloseTo(0.32);
    expect(view.turns).toBe(3);
    expect((view.items[1] as AssistantItem).streaming).toBe(false);
    expect(view.turnStartedAt).toBeNull();
  });

  test("each turn shows the tokens it used: Codex's own count, Claude Code's running total less the last", () => {
    const end = (fields: object) =>
      p({ type: "turn_ended", outcome: "success", duration_ms: 1, cost_usd: null, context_tokens: 80_000, error: null, ...fields } as ChatEvent);
    let view = buildChatView([end({ session_tokens: 1_000_000 }), end({ session_tokens: 3_500_000 })]);
    // An older turn reported no tokens; the session restarted (a lower total); Codex counts its turn.
    view = reduceChat(view, end({}));
    view = reduceChat(view, end({ session_tokens: 200_000 }));
    view = reduceChat(view, end({ tokens: 15_300_000 }));
    const turns = view.items.filter((i): i is TurnItem => i.kind === "turn");
    expect(turns.map((t) => t.tokens)).toEqual([1_000_000, 2_500_000, null, 200_000, 15_300_000]);
    expect(turns.every((t) => t.contextTokens === 80_000)).toBe(true);
  });

  test("turn summaries are numbered as chat.fork counts them, forkable where the agent said where they end", () => {
    const point = { session_id: "s", up_to: "u" };
    const view = buildChatView([
      p({ type: "turn_ended", outcome: "success", duration_ms: 1, cost_usd: null, context_tokens: null, error: null, fork_point: point }),
      // Older events carry no point; a turn that did not finish is never forked.
      p({ type: "turn_ended", outcome: "success", duration_ms: 1, cost_usd: null, context_tokens: null, error: null }),
      p({ type: "turn_ended", outcome: "interrupted", duration_ms: 1, cost_usd: null, context_tokens: null, error: null, fork_point: point }),
    ]);
    const turns = view.items.filter((i): i is TurnItem => i.kind === "turn");
    expect(turns.map((t) => [t.turn, t.forkable])).toEqual([
      [1, true],
      [2, false],
      [3, false],
    ]);
  });

  test("queued user messages, notices and session info", () => {
    const view = buildChatView([
      p({ type: "session_started", agent_session_id: "sess", model: "opus" }),
      p({ type: "user_message", text: "also do this", queued: true }),
      p({ type: "notice", level: "error", message: "could not start the agent: sidecar missing" }),
      p({ type: "notice", level: "warning", message: "Auto mode blocked a Bash call", badge: "Credential materialization" }),
    ]);
    expect(view.sessionId).toBe("sess");
    expect(view.model).toBe("opus");
    expect(view.items[0]).toMatchObject({ kind: "user", queued: true });
    expect(view.items[1]).toMatchObject({ kind: "notice", level: "error" });
    expect(view.items[1]).not.toHaveProperty("badge");
    expect(view.items[2]).toMatchObject({ kind: "notice", message: "Auto mode blocked a Bash call", badge: "Credential materialization" });
  });

  test("identical notices in a row fold into one that counts them", () => {
    const retry = { type: "notice", level: "warning", message: "Codex error, retrying: Reconnecting... waiting for network" } as const;
    const view = buildChatView([
      p(retry),
      p(retry),
      p(retry),
      p({ type: "notice", level: "warning", message: "Codex error, retrying: another one" }),
      p(retry),
    ]);
    expect(view.items.map((item) => (item.kind === "notice" ? item.count : "other"))).toEqual([3, undefined, undefined]);
  });

  test("a message sent mid-turn steers until the agent reads it; the queue follows queue_changed", () => {
    let view = buildChatView([
      p({ type: "turn_started" }),
      p({ type: "user_message", text: "and this", queued: true, message_id: "m1" }),
      p({ type: "user_message", text: "old style", queued: true }),
    ]);
    expect((view.items as UserItem[]).map((i) => i.queued)).toEqual([true, true]);
    view = reduceChat(view, p({ type: "user_message_delivered", message_id: "m1" }));
    expect((view.items as UserItem[]).map((i) => i.queued)).toEqual([false, true]);
    // Older events carry no delivery report: the end of the turn clears them.
    view = reduceChat(view, p({ type: "turn_ended", outcome: "success", duration_ms: 1, cost_usd: null, context_tokens: null, error: null }));
    expect(view.items.filter((i) => i.kind === "user").map((i) => (i as UserItem).queued)).toEqual([false, false]);

    const queued = { id: "q1", text: "later", attachments: [], created_at: "2026-10-13T10:00:00Z" };
    view = reduceChat(view, p({ type: "queue_changed", messages: [queued] }));
    expect(view.queue).toEqual([queued]);
    view = reduceChat(view, p({ type: "queue_changed", messages: [] }));
    expect(view.queue).toEqual([]);
  });

  test("persisted events already applied are ignored; the input is never mutated", () => {
    const first = p({ type: "user_message", text: "once", queued: false });
    const view = reduceChat(emptyChatView, first);
    const frozen = JSON.stringify(view);
    const again = reduceChat(view, first);
    expect(again).toBe(view);
    reduceChat(view, p({ type: "user_message", text: "twice", queued: false }));
    expect(JSON.stringify(view)).toBe(frozen);
    expect(emptyChatView.items).toHaveLength(0);
  });

  test("events of other scopes are ignored", () => {
    const view = reduceChat(emptyChatView, {
      seq: 999,
      ts: "",
      event: { scope: "workspace", workspace_id: "w", event: { type: "progress", message: "x" } },
    });
    expect(view).toBe(emptyChatView);
  });
});

describe("toolSummary", () => {
  test("shows the command, path or pattern", () => {
    expect(toolSummary("Bash", { command: "bun test", description: "Run tests" })).toBe("bun test");
    expect(toolSummary("Edit", { file_path: "/repo/a.ts", old_string: "a", new_string: "b" })).toBe("/repo/a.ts");
    expect(toolSummary("Glob", { pattern: "**/*.ts" })).toBe("**/*.ts");
    expect(toolSummary("WebFetch", { url: "https://example.com", prompt: "x" })).toBe("https://example.com");
    expect(toolSummary("exec_command", { command: ["git", "status"] })).toBe("git status");
    expect(toolSummary("Mystery", null)).toBe("");
  });
});

describe("old event logs", () => {
  test("a TodoWrite call (Claude Code no longer offers it) is a plain tool row", () => {
    const view = buildChatView([
      p({ type: "turn_started" }),
      p({
        type: "tool_call",
        tool_use_id: "t1",
        name: "TodoWrite",
        input: { todos: [{ content: "Write tests", status: "in_progress", activeForm: "Writing tests" }] },
        parent_tool_use_id: null,
      }),
      p({ type: "tool_result", tool_use_id: "t1", output: "Todos have been modified successfully.", is_error: false }),
    ]);
    const tool = view.items[0] as ToolItem;
    expect(tool).toMatchObject({ kind: "tool", name: "TodoWrite", summary: "", result: { isError: false } });
    expect(groupActivity(view.items)).toEqual([tool]);
  });
});

describe("groupActivity", () => {
  const call = (id: string, name: string, parent: string | null = null) =>
    p({ type: "tool_call", tool_use_id: id, name, input: { command: "ls" }, parent_tool_use_id: parent });
  const result = (id: string, isError = false) => p({ type: "tool_result", tool_use_id: id, output: "", is_error: isError });

  test("tool calls and thinking between two messages fold into one group; a lone call stays as is", () => {
    const view = buildChatView([
      p({ type: "user_message", text: "go", queued: false }),
      p({ type: "turn_started" }),
      p({ type: "thinking", message_id: "m0", text: "hmm" }),
      call("t1", "Bash"),
      result("t1"),
      call("t2", "mcp__gitnexus__list_repos"),
      result("t2", true),
      p({ type: "assistant_text", message_id: "m1", text: "Found it.", parent_tool_use_id: null }),
      call("t3", "Read"),
      result("t3"),
      p({ type: "assistant_text", message_id: "m2", text: "Done.", parent_tool_use_id: null }),
    ]);
    const rows = groupActivity(view.items);
    expect(rows.map((r) => r.kind)).toEqual(["user", "activity", "assistant", "tool", "assistant"]);
    const group = rows[1] as ActivityGroup;
    expect(group.items.map((i) => i.kind)).toEqual(["thinking", "tool", "tool"]);
    expect(group.key).toBe(`activity-${view.items[1].key}`);
    expect(activitySummary(group)).toEqual({
      tools: 2,
      names: [
        { name: "Bash", count: 1 },
        { name: "gitnexus list_repos", count: 1 },
      ],
      failed: 1,
      pending: false,
    });
  });

  test("a subagent's work joins the group; a permission request splits it", () => {
    const view = buildChatView([
      p({ type: "turn_started" }),
      call("t1", "Task"),
      call("t1a", "Grep", "t1"),
      result("t1a"),
      p({ type: "assistant_text", message_id: "sub", text: "Here.", parent_tool_use_id: "t1" }),
      result("t1"),
      call("t2", "Bash"),
      p({ type: "permission_request", request_id: "r1", tool_name: "Bash", input: { command: "rm -rf x" }, title: null }),
      call("t3", "Bash"),
      call("t4", "Bash"),
    ]);
    const rows = groupActivity(view.items);
    expect(rows.map((r) => r.kind)).toEqual(["activity", "permission", "activity"]);
    expect((rows[0] as ActivityGroup).items.map((i) => i.kind)).toEqual(["tool", "tool", "tool"]);
    const tail = activitySummary(rows[2] as ActivityGroup);
    expect(tail).toEqual({ tools: 2, names: [{ name: "Bash", count: 2 }], failed: 0, pending: true });
  });

  test("a group keeps its key while it grows", () => {
    const events = [p({ type: "turn_started" }), call("t1", "Bash"), call("t2", "Bash")];
    const before = groupActivity(buildChatView(events).items);
    const after = groupActivity(buildChatView([...events, call("t3", "Read")]).items);
    expect(after[0].key).toBe(before[0].key);
    expect((after[0] as ActivityGroup).items).toHaveLength(3);
  });

  test("a group stays the same object while text streams after it, and changes with one of its steps", () => {
    const grouped = buildChatView([p({ type: "turn_started" }), call("t1", "Bash"), call("t2", "Bash")]);
    const delta = (text: string) => e({ type: "text_delta", message_id: "m1", text });
    const streaming = reduceChat(grouped, delta("Hel"));
    const before = groupActivity(streaming.items);
    const after = groupActivity(reduceChat(streaming, delta("lo")).items);
    expect(after[0]).toBe(before[0]);
    expect(after[1]).not.toBe(before[1]);
    const answered = groupActivity(reduceChat(streaming, result("t1")).items);
    expect(answered[0]).not.toBe(before[0]);
    expect(answered[0].key).toBe(before[0].key);
  });
});

describe("the agent's tasks", () => {
  const started = (taskId: string, fields: Partial<Extract<ChatEvent, { type: "task_started" }>> = {}): ChatEvent => ({
    type: "task_started",
    task_id: taskId,
    kind: "command",
    description: "sleep 60",
    subagent_type: null,
    tool_use_id: null,
    background: true,
    parent_task_id: null,
    ...fields,
  });

  test("started, then its progress, then gone at its end", () => {
    const first = p(started("b1"));
    let view = buildChatView([
      first,
      p(started("a1", { kind: "subagent", subagent_type: "Explore", description: "Explore the API" })),
    ]);
    expect(view.tasks.map((t) => [t.taskId, t.kind, t.subagentType])).toEqual([
      ["b1", "command", null],
      ["a1", "subagent", "Explore"],
    ]);
    expect(view.tasks[0]?.startedAt).toBe(first.ts);
    view = reduceChat(view, e({ type: "task_progress", task_id: "a1", activity: null, last_tool: "Grep", summary: null, duration_ms: 3000, tool_uses: 4 }));
    expect(view.tasks[1]).toMatchObject({ lastTool: "Grep", toolUses: 4, summary: null, description: "Explore the API" });
    view = reduceChat(view, e({ type: "task_progress", task_id: "a1", activity: "Running ls", last_tool: null, summary: "Reading routes.py", duration_ms: 5000, tool_uses: 6 }));
    expect(view.tasks[1]).toMatchObject({ lastTool: "Grep", activity: "Running ls", summary: "Reading routes.py", toolUses: 6, description: "Explore the API" });
    view = reduceChat(view, p({ type: "task_ended", task_id: "a1", status: "completed", summary: "Found it" }));
    expect(view.tasks.map((t) => t.taskId)).toEqual(["b1"]);
    // Nothing in the conversation itself: tasks live beside it.
    expect(view.items).toEqual([]);
  });

  test("a task sent again (moved to the background) keeps its start and its progress", () => {
    const first = p(started("a1", { kind: "subagent", background: false }));
    let view = buildChatView([first]);
    view = reduceChat(view, e({ type: "task_progress", task_id: "a1", activity: null, last_tool: "Read", summary: null, duration_ms: 1, tool_uses: 1 }));
    view = reduceChat(view, p(started("a1", { kind: "subagent", background: true })));
    expect(view.tasks).toHaveLength(1);
    expect(view.tasks[0]).toMatchObject({ background: true, startedAt: first.ts, lastTool: "Read" });
  });

  test("progress or an end for a task never shown changes nothing", () => {
    const view = buildChatView([p(started("b1"))]);
    expect(reduceChat(view, e({ type: "task_progress", task_id: "x", activity: null, last_tool: "Bash", summary: null, duration_ms: 1, tool_uses: 1 })).tasks).toBe(view.tasks);
    expect(reduceChat(view, p({ type: "task_ended", task_id: "x", status: "stopped", summary: null })).tasks).toBe(view.tasks);
  });

  test("a session restart clears the tasks: each still running is reported stopped", () => {
    const view = buildChatView([
      p({ type: "session_started", agent_session_id: "s1", model: "haiku" }),
      p(started("b1")),
      p(started("b2")),
      p({ type: "notice", level: "error", message: "the agent process stopped; send a message to resume this chat" }),
      p({ type: "task_ended", task_id: "b1", status: "stopped", summary: "The agent process stopped" }),
      p({ type: "task_ended", task_id: "b2", status: "stopped", summary: "The agent process stopped" }),
      p({ type: "session_started", agent_session_id: "s1", model: "haiku" }),
    ]);
    expect(view.tasks).toEqual([]);
  });

  test("a closed chat runs nothing", () => {
    const view = buildChatView([p(started("b1")), p({ type: "status_changed", status: "closed" })]);
    expect(view.tasks).toEqual([]);
  });
});

describe("a subagent's text", () => {
  const agent = () =>
    p({ type: "tool_call", tool_use_id: "toolu_a", name: "Agent", input: { description: "Lighthouses", prompt: "Write" }, parent_tool_use_id: null });

  test("never enters the chat: its view gets it, live and streamed alike", () => {
    let view = buildChatView([
      p({ type: "turn_started" }),
      agent(),
      p({
        type: "task_started",
        task_id: "a1",
        kind: "subagent",
        description: "Lighthouses",
        subagent_type: "general-purpose",
        tool_use_id: "toolu_a",
        background: false,
        parent_task_id: null,
      }),
      e({ type: "thinking", message_id: "m:0", text: "Hmm.", parent_tool_use_id: "toolu_a" }),
      e({ type: "assistant_text", message_id: "m:1", text: "Lighthouses guide ships.", parent_tool_use_id: "toolu_a" }),
      p({ type: "tool_call", tool_use_id: "toolu_b", name: "Bash", input: { command: "echo hi" }, parent_tool_use_id: "toolu_a" }),
      e({ type: "text_delta", message_id: "m2:0", text: "Do", parent_tool_use_id: "toolu_a" }),
      e({ type: "text_delta", message_id: "m2:0", text: "ne", parent_tool_use_id: "toolu_a" }),
    ]);
    // The chat itself: the Agent call and the subagent's tool call, nothing it said.
    expect(view.items.map((i) => i.kind)).toEqual(["tool", "tool"]);
    const log = view.subagents.toolu_a!;
    expect(log.taskId).toBe("a1");
    expect(log.items.map((i) => [i.kind, i.text, i.streaming, i.anchor])).toEqual([
      ["thinking", "Hmm.", false, 1],
      ["assistant", "Lighthouses guide ships.", false, 1],
      ["assistant", "Done", true, 2],
    ]);
    view = reduceChat(view, e({ type: "assistant_text", message_id: "m2:0", text: "Done.", parent_tool_use_id: "toolu_a" }));
    expect(view.subagents.toolu_a!.items.at(-1)).toMatchObject({ text: "Done.", streaming: false });
    expect(view.items.map((i) => i.kind)).toEqual(["tool", "tool"]);

    // Its task is kept after it ends, with how it ended.
    view = reduceChat(view, e({ type: "task_progress", task_id: "a1", activity: null, last_tool: "Bash", summary: null, duration_ms: 1500, tool_uses: 1 }));
    view = reduceChat(view, p({ type: "task_ended", task_id: "a1", status: "completed", summary: null }));
    expect(view.tasks).toEqual([]);
    expect(view.subagents.toolu_a).toMatchObject({ taskId: "a1", status: "completed", durationMs: 1500, toolUses: 1 });
    expect(view.subagents.toolu_a!.endedAt).not.toBeNull();
  });

  test("the agent's own text and thinking stay in the chat", () => {
    const view = buildChatView([
      p({ type: "thinking", message_id: "t:0", text: "Plan." }),
      p({ type: "assistant_text", message_id: "t:1", text: "Hello.", parent_tool_use_id: null }),
    ]);
    expect(view.items.map((i) => i.kind)).toEqual(["thinking", "assistant"]);
    expect(view.subagents).toEqual({});
  });
});

describe("a task's row", () => {
  const task: TaskView = {
    taskId: "a1",
    kind: "subagent",
    toolUseId: "toolu_a1",
    description: "Explore",
    subagentType: "Explore",
    background: true,
    parentTaskId: null,
    startedAt: "2026-10-16T10:00:00Z",
    lastTool: null,
    activity: null,
    summary: null,
    toolUses: null,
  };

  test("its last step: its summary, else its last tool, with how many tools it used", () => {
    expect(taskStep(task)).toBeNull();
    expect(taskStep({ ...task, lastTool: "mcp__gitnexus__query", toolUses: 1 })).toBe("gitnexus query · 1 tool use");
    expect(taskStep({ ...task, lastTool: "Read", summary: "Reading the router", toolUses: 5 })).toBe("Reading the router · 5 tool uses");
    expect(taskStep({ ...task, toolUses: 2 })).toBe("2 tool uses");
    expect(taskStep({ ...task, lastTool: "Bash", activity: "Running sleep 15" })).toBe("Running sleep 15");
  });

  test("how long it has run", () => {
    expect(formatElapsed(-5)).toBe("0s");
    expect(formatElapsed(42_900)).toBe("42s");
    expect(formatElapsed(185_000)).toBe("3m 05s");
    expect(formatElapsed(3_725_000)).toBe("1h 02m");
  });

  test("a token count", () => {
    expect(formatTokens(950)).toBe("950 tokens");
    expect(formatTokens(80_073)).toBe("80k tokens");
    expect(formatTokens(999_600)).toBe("1.0M tokens");
    expect(formatTokens(15_296_687)).toBe("15.3M tokens");
    expect(formatTokens(123_000_000)).toBe("123M tokens");
  });
});

describe("auto mode checking a tool call", () => {
  const call = (id: string, parent: string | null = null): ChatEvent => ({
    type: "tool_call",
    tool_use_id: id,
    name: "Bash",
    input: { command: "git push" },
    parent_tool_use_id: parent,
  });
  const check = (id: string, checking: boolean): ChatEvent => ({ type: "permission_check", tool_use_id: id, checking });
  const result = (id: string): ChatEvent => ({ type: "tool_result", tool_use_id: id, output: "ok", is_error: false });
  const started = () => buildChatView([p({ type: "turn_started" }), p(call("t1")), p(call("t2", "agent_1"))]);

  test("shows while the check runs, a subagent's call too, and goes when it is done", () => {
    let view = reduceChat(started(), e(check("t1", true)));
    view = reduceChat(view, e(check("t2", true)));
    view = reduceChat(view, e(check("t2", true)));
    expect(view.checking).toEqual(["t1", "t2"]);
    view = reduceChat(view, e(check("t1", false)));
    expect(view.checking).toEqual(["t2"]);
    // A "done" for a call not being checked changes nothing.
    expect(reduceChat(view, e(check("t9", false))).checking).toBe(view.checking);
  });

  test("goes with the call's result when the 'done' was lost, and never comes back late", () => {
    let view = reduceChat(started(), e(check("t1", true)));
    view = reduceChat(view, p(result("t1")));
    expect(view.checking).toEqual([]);
    expect((view.items.find((i) => i.kind === "tool" && i.toolUseId === "t1") as ToolItem).result?.output).toBe("ok");
    expect(reduceChat(view, e(check("t1", true))).checking).toEqual([]);
  });

  test("goes when the turn ends or the agent stops", () => {
    const checking = reduceChat(reduceChat(started(), e(check("t1", true))), e(check("t2", true)));
    const ended = reduceChat(
      checking,
      p({ type: "turn_ended", outcome: "interrupted", duration_ms: 1, cost_usd: null, context_tokens: null, error: null }),
    );
    expect(ended.checking).toEqual([]);
    expect(reduceChat(checking, p({ type: "status_changed", status: "needs_input" })).checking).toEqual(["t1", "t2"]);
    expect(reduceChat(checking, p({ type: "status_changed", status: "idle" })).checking).toEqual([]);
    expect(reduceChat(checking, p({ type: "status_changed", status: "error" })).checking).toEqual([]);
  });
});

describe("the agent's suggested next message", () => {
  const turnEnded = (): ChatEvent => ({ type: "turn_ended", outcome: "success", duration_ms: 1, cost_usd: null, context_tokens: null, error: null });
  const suggestion = (text: string): ChatEvent => ({ type: "prompt_suggestion", text });

  test("arrives after the turn, and goes when the next turn starts or the chat leaves idle", () => {
    let view = buildChatView([p({ type: "turn_started" }), p(turnEnded())]);
    view = reduceChat(view, e(suggestion("Run the tests")));
    expect(view.suggestion?.text).toBe("Run the tests");
    const next = reduceChat(view, e(suggestion("Commit it")));
    expect(next.suggestion?.text).toBe("Commit it");
    expect(next.suggestion?.key).not.toBe(view.suggestion?.key);
    expect(reduceChat(view, p({ type: "turn_started" })).suggestion).toBeNull();
    expect(reduceChat(view, p({ type: "status_changed", status: "working" })).suggestion).toBeNull();
    expect(reduceChat(view, p({ type: "status_changed", status: "idle" })).suggestion?.text).toBe("Run the tests");
  });

  test("one arriving while a turn runs is dropped; a replay never has one", () => {
    const working = buildChatView([p({ type: "turn_started" })]);
    expect(reduceChat(working, e(suggestion("late"))).suggestion).toBeNull();
    expect(buildChatView([p({ type: "turn_started" }), p(turnEnded())]).suggestion).toBeNull();
  });
});

describe("the context reading", () => {
  const usage = (percent: number): ContextUsage => ({
    model: "claude-opus-5-5",
    used_tokens: percent * 10_000,
    max_tokens: 1_000_000,
    percent,
    over_limit: false,
    categories: [{ name: "Messages", tokens: percent * 10_000, kind: "used" }],
    compaction: { by: "claude_code", threshold_percent: 96.7 },
  });

  test("the last one of the log stays, with when it was read, across a reload", () => {
    const events = [p({ type: "context_usage", usage: usage(12) }), p({ type: "turn_ended", outcome: "success", duration_ms: 1, cost_usd: null, context_tokens: null, error: null }), p({ type: "context_usage", usage: usage(15) })];
    const view = buildChatView(events);
    expect(view.context?.usage.percent).toBe(15);
    expect(view.context?.readAt).toBe(events[2]?.ts as string);
    expect(buildChatView([]).context).toBeNull();
  });
});

describe("repo proposals", () => {
  const request = (input: unknown, toolName = "mcp__chauffeur__add_repo") => ({ toolName, input: input as never });

  test("an add_repo request proposes its repo, with the agent's reason and the repo's description", () => {
    expect(repoProposal(request({ repo: "analytics", reason: " Les dashboards ", description: "Dashboards" }))).toEqual({
      repo: "analytics",
      reason: "Les dashboards",
      description: "Dashboards",
    });
    expect(repoProposal(request({ repo: "analytics", reason: "", description: null }))).toEqual({
      repo: "analytics",
      reason: null,
      description: null,
    });
  });

  test("any other request, or one without a repo, is no proposal", () => {
    expect(repoProposal(request({ repo: "analytics" }, "Bash"))).toBeNull();
    expect(repoProposal(request({ reason: "r" }))).toBeNull();
    expect(repoProposal(request(null))).toBeNull();
  });

  test("its card says how it ended", () => {
    expect(repoProposalOutcome({ state: "open", outcome: null })).toBeNull();
    expect(repoProposalOutcome({ state: "resolved", outcome: "allowed" })).toBe("accepted");
    expect(repoProposalOutcome({ state: "resolved", outcome: "denied" })).toBe("not now");
    expect(repoProposalOutcome({ state: "resolved", outcome: "cancelled" })).toBe("withdrawn");
    expect(repoProposalOutcome({ state: "expired", outcome: null })).toBe("expired");
  });

  test("its tool call is summed up by the repo", () => {
    expect(toolSummary("mcp__chauffeur__add_repo", { repo: "analytics", reason: "r" })).toBe("analytics");
  });
});

describe("delegated chats", () => {
  const delegateCall = (id: string, input: Record<string, string>) =>
    p({ type: "tool_call", tool_use_id: id, name: DELEGATE_TOOL, input, parent_tool_use_id: null });
  const chat = (id: string, fields: Partial<ChatInfo> = {}): ChatInfo => ({
    id,
    workspace_id: "w",
    agent: "codex",
    model: null,
    effort: null,
    fast: false,
    permission_mode: "auto",
    title: null,
    status: "working",
    agent_session_id: null,
    created_at: "2026-10-19T10:00:00Z",
    running_tasks: 0,
    closed: false,
    last_activity_at: null,
    mcp_servers: [],
    kind: "chat",
    ...fields,
  });

  test("a delegate call stands as its own row among the folded steps, summed up by its task", () => {
    const view = buildChatView([
      p({ type: "turn_started" }),
      p({ type: "tool_call", tool_use_id: "t1", name: "Bash", input: { command: "ls" }, parent_tool_use_id: null }),
      delegateCall("t2", { agent: "codex", prompt: "Review the diff\nof web" }),
      p({ type: "tool_call", tool_use_id: "t3", name: "Read", input: { file_path: "/a" }, parent_tool_use_id: null }),
      p({ type: "tool_call", tool_use_id: "t4", name: "Read", input: { file_path: "/b" }, parent_tool_use_id: null }),
    ]);
    const rows = groupActivity(view.items);
    expect(rows.map((r) => r.kind)).toEqual(["tool", "tool", "activity"]);
    expect((rows[1] as ToolItem).summary).toBe("Review the diff");
  });

  test("the call finds the chat it started, or the one it gave more work, among its own chat's", () => {
    const started = (buildChatView([delegateCall("toolu_1", { agent: "codex", prompt: "Review" })]).items[0]) as ToolItem;
    const chats = [
      chat("sub-a", { parent_chat: "parent", parent_tool_use: "toolu_1" }),
      chat("sub-b", { parent_chat: "parent", parent_tool_use: "toolu_0", agent: "claude" }),
      chat("other", { parent_chat: "elsewhere", parent_tool_use: "toolu_1" }),
    ];
    expect(delegatedChat(started, "parent", chats)?.id).toBe("sub-a");
    expect(delegatedChat(started, "elsewhere", chats)?.id).toBe("other");
    expect(delegatedChat(started, "nobody", chats)).toBeNull();
    const more = (buildChatView([delegateCall("toolu_2", { agent: "codex", prompt: "Fix it", chat: "sub-b" })]).items[0]) as ToolItem;
    const named = delegatedChat(more, "parent", chats);
    expect(named?.id).toBe("sub-b");
    // The delegated chat's own agent, whatever the input says.
    expect(delegatedAgent(more, named)).toBe("claude");
    expect(delegatedAgent(more, null)).toBe("codex");
    // A chat the input names but that is not this chat's delegated chat is not shown as one.
    expect(delegatedChat(more, "elsewhere", chats)).toBeNull();
  });
});


test("MCP confirmations survive replay, resolve, and expire without retaining submitted content", () => {
  const request: ChatEvent = { type: "mcp_elicitation", request_id: "mcp-1", server_name: "computer-use", message: "Allow Chrome?", schema: { type: "object", properties: {} }, url: null, error: null };
  const events = [p(request)];
  const view = buildChatView(events);
  expect(openRequests(view)).toHaveLength(1);
  expect(view.items[0]).toMatchObject({ kind: "elicitation", serverName: "computer-use", state: "open" });
  const resolved = reduceChat(view, p({ type: "request_resolved", request_id: "mcp-1", outcome: "denied" }));
  expect(openRequests(resolved)).toHaveLength(0);
  expect(resolved.items[0]).toMatchObject({ state: "resolved", outcome: "denied" });
  const expired = reduceChat(view, p({ type: "turn_ended", outcome: "interrupted", duration_ms: null, cost_usd: null, context_tokens: null, error: null }));
  expect(expired.items[0]).toMatchObject({ state: "expired" });
  expect(openRequests(expired)).toHaveLength(0);
  expect(buildChatView(events)).toEqual(view);
});

test("an MCP confirmation accepted always keeps its rule", () => {
  const rule = 'cua_repl: Allow Computer Use to use "Google Chrome"?';
  const view = buildChatView([
    p({ type: "mcp_elicitation", request_id: "mcp-1", server_name: "cua_repl", message: 'Allow Computer Use to use "Google Chrome"?', schema: { type: "object", properties: {} }, url: null, error: null, rule }),
  ]);
  expect(view.items[0]).toMatchObject({ kind: "elicitation", rule, rules: null });
  const resolved = reduceChat(view, p({ type: "request_resolved", request_id: "mcp-1", outcome: "allowed", rules: [rule] }));
  expect(resolved.items[0]).toMatchObject({ state: "resolved", outcome: "allowed", rules: [rule] });
});

describe("a message sent from a guest's suggestion", () => {
  test("names the guest who suggested it; the user's own messages name nobody", () => {
    const view = buildChatView([
      p({ type: "user_message", text: "split per repo", queued: false, suggested_by: { name: "Bob", email: "bob@example.com" } }),
      p({ type: "user_message", text: "mine", queued: false }),
    ]);
    const users = view.items.filter((item): item is UserItem => item.kind === "user");
    expect(users.map((u) => u.suggestedBy?.name ?? null)).toEqual(["Bob", null]);
  });
});

test("a delegated chat's answer Chauffeur sent once the turn had ended names that chat", () => {
  const delegated = { chat: "sub-1", agent: "codex" as const, title: "Review the diff" };
  const view = buildChatView([
    p({ type: "user_message", text: "Chauffeur: your turn ended before…", queued: false, delegated_answer: delegated }),
    p({ type: "user_message", text: "mine", queued: false }),
  ]);
  const users = view.items.filter((item): item is UserItem => item.kind === "user");
  expect(users.map((u) => u.delegatedAnswer ?? null)).toEqual([delegated, null]);
});
