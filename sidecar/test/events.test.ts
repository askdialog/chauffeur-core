import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import type { SDKMessage } from "@anthropic-ai/claude-agent-sdk";
import { ClaudeEventMapper, TOOL_OUTPUT_LIMIT, toolImages, toolOutput } from "../src/agents/claude/events.ts";
import type { ChatEvent } from "../src/protocol.ts";
import { sdk } from "./support/fakes.ts";

/** Two turns recorded from Claude Code 2.1.296 (haiku): "pong", then a guarded `git worktree add`. */
const recorded = readFileSync(join(import.meta.dir, "fixtures", "claude-session.jsonl"), "utf8")
  .trim()
  .split("\n")
  .map((line) => JSON.parse(line) as SDKMessage);

function mapAll(messages: SDKMessage[], mapper = new ClaudeEventMapper()): ChatEvent[] {
  return messages.flatMap((message) => mapper.map(message));
}

function only<T extends ChatEvent["type"]>(events: ChatEvent[], type: T): Extract<ChatEvent, { type: T }>[] {
  return events.filter((event): event is Extract<ChatEvent, { type: T }> => event.type === type);
}

describe("mapping a recorded session", () => {
  const events = mapAll(recorded);

  test("reports the session once although the CLI sends init every turn", () => {
    const started = only(events, "session_started");
    expect(started).toHaveLength(1);
    expect(started[0]).toEqual({
      type: "session_started",
      agent_session_id: "6a4ce1f9-ca32-44d6-8fa2-c679652260fe",
      model: "claude-haiku-4-5-20251001",
    });
  });

  test("streams text deltas under the final text's message id", () => {
    const texts = only(events, "assistant_text");
    expect(texts[0]).toEqual({
      type: "assistant_text",
      message_id: "msg_011CfynAgqwwUxU2gusTbX6X:1",
      text: "pong",
      parent_tool_use_id: null,
    });
    const deltas = only(events, "text_delta");
    for (const text of texts) {
      const streamed = deltas.filter((d) => d.message_id === text.message_id).map((d) => d.text).join("");
      expect(streamed).toBe(text.text);
    }
  });

  test("maps the tool call and its denied result", () => {
    const calls = only(events, "tool_call");
    expect(calls.map((c) => c.name)).toEqual(["Bash", "ToolSearch"]);
    expect(calls[0]).toMatchObject({
      type: "tool_call",
      parent_tool_use_id: null,
      input: { command: "git worktree add /tmp/repo-wt -b e2e-wt" },
    });
    const results = only(events, "tool_result");
    expect(results[0]).toMatchObject({ tool_use_id: calls[0]?.tool_use_id, is_error: true });
    expect(results[0]?.output).toContain("chauffeur worktree add --repo <repo>");
    // Structured content (a tool_reference block) is stringified.
    expect(results[1]).toMatchObject({ is_error: false, output: '{"type":"tool_reference","tool_name":"EnterWorktree"}' });
  });

  test("ends each turn with cost, duration and the last call's context size", () => {
    const ended = only(events, "turn_ended");
    expect(ended).toHaveLength(2);
    expect(ended[0]).toMatchObject({ outcome: "success", error: null });
    for (const turn of ended) {
      expect(turn.duration_ms).toBeGreaterThan(0);
      expect(turn.cost_usd).toBeGreaterThan(0);
    }
    // The second turn made three API calls: its result usage sums them (75k cache reads), the
    // context after the turn is the last call's input + output only.
    const contexts = ended.map((turn) => turn.context_tokens ?? 0);
    expect(contexts[0]).toBeGreaterThan(18_000);
    expect(contexts[1]).toBeGreaterThan(contexts[0] as number);
    expect(contexts[1]).toBeLessThan(40_000);
  });

  test("ends each finished turn where a fork from it stops: the session and its last message", () => {
    const session = "6a4ce1f9-ca32-44d6-8fa2-c679652260fe";
    expect(only(events, "turn_ended").map((turn) => turn.fork_point)).toEqual([
      { session_id: session, up_to: "155d76c3-402f-4005-a112-aebd129537ad" },
      { session_id: session, up_to: "34478b23-844e-4f00-92ca-3dbf1817f76c" },
    ]);
  });

  test("emits nothing for empty thinking, lifecycle frames and status messages", () => {
    expect(only(events, "thinking")).toEqual([]);
    expect(only(events, "notice")).toEqual([]);
    expect(new Set(events.map((e) => e.type))).toEqual(
      new Set(["session_started", "text_delta", "assistant_text", "tool_call", "tool_result", "turn_ended"]),
    );
  });
});

describe("mapping fabricated messages", () => {
  test("thinking deltas and blocks share ids; tool calls inside subagents keep their parent", () => {
    const mapper = new ClaudeEventMapper();
    const events = mapAll(
      [
        { type: "stream_event", parent_tool_use_id: null, event: { type: "message_start", message: { id: "m1", usage: { input_tokens: 3 } } } },
        { type: "stream_event", parent_tool_use_id: null, event: { type: "content_block_start", index: 0, content_block: { type: "thinking", thinking: "" } } },
        { type: "stream_event", parent_tool_use_id: null, event: { type: "content_block_delta", index: 0, delta: { type: "thinking_delta", thinking: "Hmm" } } },
        { type: "assistant", parent_tool_use_id: null, message: { id: "m1", content: [{ type: "thinking", thinking: "Hmm", signature: "s" }] } },
        { type: "assistant", parent_tool_use_id: "toolu_task", message: { id: "m2", content: [{ type: "tool_use", id: "toolu_2", name: "Read", input: { file_path: "/a" } }] } },
      ] as unknown as SDKMessage[],
      mapper,
    );
    expect(events).toEqual([
      { type: "thinking_delta", message_id: "m1:0", text: "Hmm" },
      { type: "thinking", message_id: "m1:0", text: "Hmm" },
      { type: "tool_call", tool_use_id: "toolu_2", name: "Read", input: { file_path: "/a" }, parent_tool_use_id: "toolu_task" },
    ]);
  });

  test("numbers blocks without stream events by delivery order", () => {
    const events = mapAll([
      {
        type: "assistant",
        parent_tool_use_id: null,
        message: { id: "m9", content: [{ type: "text", text: "one" }, { type: "text", text: "two" }] },
      } as unknown as SDKMessage,
      sdk.text("m9", "three"),
    ]);
    expect(only(events, "assistant_text").map((e) => e.message_id)).toEqual(["m9:0", "m9:1", "m9:2"]);
  });

  test("maps a tool_result's text blocks and truncates long outputs", () => {
    const [joined] = mapAll([sdk.toolResult("t1", [{ type: "text", text: "a" }, { type: "image" }, { type: "text", text: "b" }])]);
    expect(joined).toEqual({ type: "tool_result", tool_use_id: "t1", output: "a\n[image]\nb", is_error: false });

    const long = "x".repeat(TOOL_OUTPUT_LIMIT + 500);
    const [cut] = mapAll([sdk.toolResult("t2", long, true)]);
    expect(cut?.type === "tool_result" && cut.is_error).toBe(true);
    expect(cut?.type === "tool_result" && cut.output).toEndWith("… [truncated 500 characters]");
    expect(toolOutput({ structured: [1, 2] })).toBe('{"structured":[1,2]}');
    expect(toolOutput(undefined)).toBe("");
  });

  test("ignores user messages without tool results (echoes of what the user sent)", () => {
    expect(mapAll([{ type: "user", parent_tool_use_id: null, message: { role: "user", content: "hello" } } as SDKMessage])).toEqual([]);
  });

  test("reports a changed session id (resume, fork) again", () => {
    const events = mapAll([sdk.init("a"), sdk.init("a"), sdk.init("b")]);
    expect(only(events, "session_started").map((e) => e.agent_session_id)).toEqual(["a", "b"]);
  });

  test("turns errors and interruptions into turn outcomes", () => {
    const mapper = new ClaudeEventMapper();
    const errored = mapper.map(sdk.result({ subtype: "error_max_turns", is_error: true, errors: [] }));
    expect(errored[0]).toMatchObject({ type: "turn_ended", outcome: "error", error: "Claude reached the maximum number of turns" });

    const apiError = mapper.map(sdk.result({ is_error: true, result: "API Error: 529 overloaded" }));
    expect(apiError[0]).toMatchObject({ outcome: "error", error: "API Error: 529 overloaded" });

    const aborted = mapper.map(sdk.result({ subtype: "error_during_execution", is_error: true, errors: ["aborted"], terminal_reason: "aborted_streaming" }));
    expect(aborted[0]).toMatchObject({ outcome: "interrupted", error: null });

    const asked = mapper.map(sdk.result(), { interrupted: true });
    expect(asked[0]).toMatchObject({ outcome: "interrupted" });
  });

  test("reports the session's running token total from every model, subagents and cache included", () => {
    const mapper = new ClaudeEventMapper();
    const model = (input: number, read: number, write: number, output: number) =>
      ({ inputTokens: input, cacheReadInputTokens: read, cacheCreationInputTokens: write, outputTokens: output, costUSD: 0 });
    const ended = mapper.map(sdk.result({ modelUsage: { "claude-opus-5-5": model(10, 1_000_000, 20_000, 5_000), "claude-haiku-5-5": model(100, 0, 0, 50) } }));
    expect(ended[0]).toMatchObject({ type: "turn_ended", session_tokens: 1_025_160 });
    // A crashed result carries zeroed usage: no total rather than a restart to zero.
    expect(mapper.map(sdk.result({ modelUsage: {} }))[0]).not.toHaveProperty("session_tokens");
  });

  test("gives no fork point to a turn that did not finish, nor for a subagent's last message", () => {
    const mapper = new ClaudeEventMapper();
    const withUuid = (message: SDKMessage, uuid: string) => ({ ...message, uuid }) as SDKMessage;
    mapper.map(sdk.init("sess-1"));
    mapper.map(withUuid(sdk.text("m1", "Done"), "u1"));
    mapper.map(withUuid(sdk.text("m2", "Sub", "toolu_1"), "u2"));
    expect(mapper.map(sdk.result())[0]).toMatchObject({ fork_point: { session_id: "sess-1", up_to: "u1" } });
    // The next turn starts afresh: no message of its own yet, no point.
    expect(mapper.map(sdk.result())[0]).not.toHaveProperty("fork_point");
    mapper.map(withUuid(sdk.toolResult("toolu_2", "out"), "u3"));
    expect(mapper.map(sdk.result({ subtype: "error_during_execution", is_error: true, errors: ["aborted"] }))[0]).not.toHaveProperty("fork_point");
    mapper.map(withUuid(sdk.toolResult("toolu_3", "out"), "u4"));
    expect(mapper.map(sdk.result())[0]).toMatchObject({ fork_point: { session_id: "sess-1", up_to: "u4" } });
  });

  test("falls back to the result's usage for a single-call turn only", () => {
    const single = new ClaudeEventMapper().map(sdk.result({ num_turns: 1 }));
    expect(single[0]).toMatchObject({ context_tokens: 115 });
    const multi = new ClaudeEventMapper().map(sdk.result({ num_turns: 3 }));
    expect(multi[0]).toMatchObject({ context_tokens: null });
  });

  test("surfaces retries, compaction, auth failures and usage limits as notices", () => {
    const mapper = new ClaudeEventMapper();
    const events = mapAll(
      [
        { type: "system", subtype: "api_retry", attempt: 1, max_retries: 10, retry_delay_ms: 2000, error_status: 529, error: "overloaded" },
        { type: "system", subtype: "compact_boundary", compact_metadata: { trigger: "auto", pre_tokens: 150000 } },
        { type: "auth_status", isAuthenticating: false, output: [], error: "token expired" },
        { type: "rate_limit_event", rate_limit_info: { status: "allowed" } },
        { type: "rate_limit_event", rate_limit_info: { status: "rejected", rateLimitType: "five_hour" } },
        { type: "rate_limit_event", rate_limit_info: { status: "rejected", rateLimitType: "five_hour" } },
        { type: "system", subtype: "informational", level: "info", content: "transcript only" },
        { type: "system", subtype: "informational", level: "warning", content: "Stop hook said no" },
      ] as unknown as SDKMessage[],
      mapper,
    );
    expect(events).toEqual([
      { type: "notice", level: "warning", message: "Claude API error 529 (overloaded), retry 1/10 in 2s" },
      { type: "notice", level: "info", message: "Conversation compacted (auto, 150000 tokens before)" },
      { type: "notice", level: "error", message: "Claude authentication failed: token expired" },
      { type: "notice", level: "error", message: "Claude usage limit reached (five_hour)" },
      { type: "notice", level: "warning", message: "Stop hook said no" },
    ]);
  });

  test("auto mode's classifier denials are notices, other denials are not", () => {
    const denied = (fields: Record<string, unknown>) => ({
      type: "system",
      subtype: "permission_denied",
      tool_name: "Bash",
      tool_use_id: "toolu_1",
      message: "Permission for this action was denied by the Claude Code auto mode classifier. Reason: force push",
      uuid: "u",
      session_id: "s",
      ...fields,
    });
    const events = mapAll([
      denied({ decision_reason_type: "classifier", decision_reason: "\u001b[1mForce push\u001b[0m to the default branch" }),
      denied({ decision_reason_type: "classifier" }),
      denied({ decision_reason_type: "classifier", decision_reason: "[Credential Materialization]\n" }),
      denied({ decision_reason_type: "classifier", decision_reason: "[PII Data Handling]." }),
      denied({ decision_reason_type: "rule", decision_reason: "Bash(rm:*) is denied" }),
    ] as unknown as SDKMessage[]);
    const warning = (message: string, badge?: string) => ({
      type: "notice" as const,
      level: "warning" as const,
      message,
      ...(badge ? { badge } : {}),
    });
    expect(events).toEqual([
      warning("Auto mode blocked a Bash call: Force push to the default branch"),
      warning("Auto mode blocked a Bash call: force push"),
      warning("Auto mode blocked a Bash call", "Credential materialization"),
      warning("Auto mode blocked a Bash call", "PII data handling"),
    ]);
  });

  test("auto mode's permission check on a tool call is passed on, in a subagent too", () => {
    const check = (tool_use_id: string, status: string, fields: Record<string, unknown> = {}) => ({
      type: "system",
      subtype: "permission_check_status",
      tool_use_id,
      status,
      uuid: "u",
      session_id: "s",
      ...fields,
    });
    const events = mapAll([
      check("toolu_1", "checking"),
      check("toolu_1", "done"),
      check("toolu_2", "checking", { agent_id: "a1b2" }),
      check("toolu_2", "done", { agent_id: "a1b2" }),
      check("toolu_3", "unknown"),
      check("", "checking"),
    ] as unknown as SDKMessage[]);
    expect(events).toEqual([
      { type: "permission_check", tool_use_id: "toolu_1", checking: true },
      { type: "permission_check", tool_use_id: "toolu_1", checking: false },
      { type: "permission_check", tool_use_id: "toolu_2", checking: true },
      { type: "permission_check", tool_use_id: "toolu_2", checking: false },
    ]);
  });

  test("a check whose 'done' is lost ends with the call's denial, or once asked", () => {
    const mapper = new ClaudeEventMapper();
    const checking = (tool_use_id: string) =>
      ({ type: "system", subtype: "permission_check_status", tool_use_id, status: "checking", uuid: "u", session_id: "s" }) as unknown as SDKMessage;
    const denied = {
      type: "system",
      subtype: "permission_denied",
      tool_name: "Bash",
      tool_use_id: "toolu_1",
      decision_reason_type: "classifier",
      decision_reason: "Force push",
      message: "denied",
      uuid: "u",
      session_id: "s",
    } as unknown as SDKMessage;
    expect(mapAll([checking("toolu_1"), denied], mapper)).toEqual([
      { type: "permission_check", tool_use_id: "toolu_1", checking: true },
      { type: "permission_check", tool_use_id: "toolu_1", checking: false },
      { type: "notice", level: "warning", message: "Auto mode blocked a Bash call: Force push" },
    ]);
    mapAll([checking("toolu_2")], mapper);
    expect(mapper.checkOver("toolu_2")).toEqual([{ type: "permission_check", tool_use_id: "toolu_2", checking: false }]);
    // Once only, and never for a call that was not being checked.
    expect(mapper.checkOver("toolu_2")).toEqual([]);
    expect(mapper.checkOver("toolu_9")).toEqual([]);
    // A call that got its result is no longer checked.
    mapAll([checking("toolu_3"), sdk.toolResult("toolu_3", "ok")], mapper);
    expect(mapper.checkOver("toolu_3")).toEqual([]);
  });

  test("a plugin's terminal UI is not reported, its toasts are shown", () => {
    const events = mapAll([
      { type: "system", subtype: "ui_log", plugin: "my-mod", text: "debug line", uuid: "u1", session_id: "s" },
      { type: "system", subtype: "ui_status", plugin: "my-mod", text: "3 tasks", uuid: "u2", session_id: "s" },
      { type: "system", subtype: "ui_toast", plugin: "my-mod", text: "Tests passed", timeout_ms: 4000, uuid: "u3", session_id: "s" },
    ] as unknown as SDKMessage[]);
    expect(events).toEqual([{ type: "notice", level: "info", message: "my-mod: Tests passed" }]);
  });

  test("a commit or push seen by Claude Code's shell is not reported", () => {
    const events = mapAll([
      { type: "system", subtype: "vcs_state_changed", kind: "push", branch: "main", cwd: "/w", uuid: "u1", session_id: "s" },
      { type: "system", subtype: "code_change_published", uuid: "u2", session_id: "s" },
    ] as unknown as SDKMessage[]);
    expect(events).toEqual([]);
  });

  test("a rewind stand-in row is not reported", () => {
    const events = mapAll([
      { type: "system", subtype: "rewind_stand_in", source_uuid: "u0", content: "", level: "info", uuid: "u1", session_id: "s" },
    ] as unknown as SDKMessage[]);
    expect(events).toEqual([]);
  });

  test("Claude Code's own title for a session (a forked one's) is not reported", () => {
    const events = mapAll([
      { type: "system", subtype: "session_title_changed", title: "Plan the login page (fork)", uuid: "u1", session_id: "s" },
    ] as unknown as SDKMessage[]);
    expect(events).toEqual([]);
  });

  test("a prompt suggestion is passed on, trimmed; an empty one is not", () => {
    expect(mapAll([sdk.suggestion("  Run the tests\n"), sdk.suggestion("   ")])).toEqual([
      { type: "prompt_suggestion", text: "Run the tests" },
    ]);
  });

  test("reports an unknown message type once", () => {
    const events = mapAll([{ type: "brand_new" }, { type: "brand_new" }, { type: "system", subtype: "hook_started" }] as unknown as SDKMessage[]);
    expect(events).toEqual([{ type: "notice", level: "info", message: 'Chauffeur does not display Claude "brand_new" messages yet' }]);
  });
});

describe("a tool result's images", () => {
  const png = "iVBORw0KGgo=";

  test("an image the agent read or a screenshot goes to the daemon with the result", () => {
    const message = {
      type: "user",
      message: {
        role: "user",
        content: [
          {
            type: "tool_result",
            tool_use_id: "toolu_1",
            content: [
              { type: "text", text: "Took a screenshot" },
              { type: "image", source: { type: "base64", media_type: "image/png", data: png } },
            ],
          },
        ],
      },
      parent_tool_use_id: null,
      session_id: "s",
    } as unknown as SDKMessage;
    expect(new ClaudeEventMapper().map(message)).toEqual([
      {
        type: "tool_result",
        tool_use_id: "toolu_1",
        output: "Took a screenshot\n[image]",
        is_error: false,
        images: [{ data: png, media_type: "image/png" }],
      },
    ]);
  });

  test("an image file the agent read keeps its name", () => {
    const mapper = new ClaudeEventMapper();
    mapper.map({
      type: "assistant",
      message: { id: "m", role: "assistant", content: [{ type: "tool_use", id: "toolu_2", name: "Read", input: { file_path: "/ws/shop-web/chart.png" } }] },
      parent_tool_use_id: null,
      session_id: "s",
    } as unknown as SDKMessage);
    const result = mapper.map({
      type: "user",
      message: { role: "user", content: [{ type: "tool_result", tool_use_id: "toolu_2", content: [{ type: "image", source: { type: "base64", media_type: "image/png", data: png } }] }] },
      parent_tool_use_id: null,
      session_id: "s",
    } as unknown as SDKMessage);
    expect(result).toEqual([expect.objectContaining({ images: [{ data: png, media_type: "image/png", name: "chart.png" }] })]);
  });

  test("MCP and Codex images are read too; a result without any has no images field", () => {
    expect(toolImages([{ type: "image", data: png, mimeType: "image/jpeg" }])).toEqual([{ data: png, media_type: "image/jpeg" }]);
    expect(toolImages([{ type: "inputImage", imageUrl: `data:image/webp;base64,${png}` }])).toEqual([
      { data: png, media_type: "image/webp" },
    ]);
    expect(toolImages([{ type: "inputImage", imageUrl: "https://example.com/a.png" }])).toEqual([]);
    expect(toolImages("text only")).toEqual([]);
    const plain = { type: "user", message: { role: "user", content: [{ type: "tool_result", tool_use_id: "t", content: "ok" }] }, parent_tool_use_id: null, session_id: "s" } as unknown as SDKMessage;
    expect(new ClaudeEventMapper().map(plain)).toEqual([{ type: "tool_result", tool_use_id: "t", output: "ok", is_error: false }]);
  });
});
