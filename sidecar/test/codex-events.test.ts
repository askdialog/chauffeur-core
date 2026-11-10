import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { CodexEventMapper, errorText, shellScript } from "../src/agents/codex/events.ts";
import type { ServerNotification } from "../src/agents/codex/protocol.ts";
import type { ChatEvent } from "../src/protocol.ts";

/** Notifications recorded from codex-cli 0.162.1 app-server sessions (gpt-6-luna). */
function recorded(name: string): ServerNotification[] {
  return readFileSync(join(import.meta.dir, "fixtures", "codex", `${name}.jsonl`), "utf8")
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line) as ServerNotification & { id?: unknown })
    .filter((message) => message.id === undefined); // server requests go to the session
}

function mapAll(name: string): ChatEvent[] {
  const mapper = new CodexEventMapper();
  return recorded(name).flatMap((message) => mapper.map(message));
}

function only<T extends ChatEvent["type"]>(events: ChatEvent[], type: T): Extract<ChatEvent, { type: T }>[] {
  return events.filter((event): event is Extract<ChatEvent, { type: T }> => event.type === type);
}

describe("mapping recorded Codex sessions", () => {
  test("pong: a turn, streamed text under the final text's id, usage", () => {
    const events = mapAll("pong");
    expect(events.map((e) => e.type)).toEqual(["turn_started", "text_delta", "assistant_text", "context_usage", "turn_ended"]);
    // The last response's tokens against the model's window: no breakdown, no known threshold.
    const [reading] = only(events, "context_usage");
    expect(reading?.usage).toMatchObject({ categories: [], compaction: null, over_limit: false });
    expect(reading?.usage.used_tokens).toBe(only(events, "turn_ended")[0]?.context_tokens as number);
    expect(reading?.usage.max_tokens).toBeGreaterThan(100_000);
    const [text] = only(events, "assistant_text");
    expect(text).toMatchObject({ text: "pong", parent_tool_use_id: null });
    expect(only(events, "text_delta").map((d) => d.message_id)).toEqual([text?.message_id as string]);
    expect(only(events, "turn_ended")[0]).toEqual({
      type: "turn_ended",
      outcome: "success",
      duration_ms: expect.any(Number),
      cost_usd: null,
      tokens: expect.any(Number),
      context_tokens: expect.any(Number),
      error: null,
      fork_point: { session_id: "01a13421-3cc9-7490-a086-07855af2053b", up_to: "01a13421-3f0c-7a13-9191-ac9d4b464a67" },
    });
    expect(only(events, "turn_ended")[0]?.context_tokens).toBeGreaterThan(10_000);
  });

  test("tools: commands with their output, a file change, a steered message", () => {
    const events = mapAll("tools");
    const calls = only(events, "tool_call");
    expect(calls).toEqual([
      expect.objectContaining({ name: "Bash", input: { command: "sleep 6 && echo first-done", cwd: "/tmp/ws" } }),
      expect.objectContaining({
        name: "apply_patch",
        input: { changes: [{ path: "/tmp/ws/plan.md", kind: { type: "add" }, diff: "plan\n" }] },
      }),
    ]);
    const results = only(events, "tool_result");
    expect(results).toEqual([
      { type: "tool_result", tool_use_id: calls[0]?.tool_use_id as string, output: "first-done\n", is_error: false },
      { type: "tool_result", tool_use_id: calls[1]?.tool_use_id as string, output: "add /tmp/ws/plan.md", is_error: false },
    ]);
    // The user's (steered) messages are the daemon's to record, not events.
    expect(only(events, "assistant_text").at(-1)?.text).toContain("STEERED");
    for (const text of only(events, "assistant_text")) {
      const streamed = only(events, "text_delta").filter((d) => d.message_id === text.message_id).map((d) => d.text).join("");
      expect(streamed).toBe(text.text);
    }
  });

  test("approval: the approved command's call and result", () => {
    const events = mapAll("approval");
    expect(only(events, "tool_call")).toEqual([
      { type: "tool_call", tool_use_id: expect.any(String), name: "Bash", input: { command: "touch codex-approved.txt", cwd: "/tmp/repo" }, parent_tool_use_id: null },
    ]);
    expect(only(events, "tool_result")).toEqual([expect.objectContaining({ is_error: false })]);
  });

  test("reasoning summaries become thinking with matching deltas", () => {
    const events = mapAll("reasoning");
    const [thinking] = only(events, "thinking");
    expect(thinking?.text).toBe("**Counting the listed primes**");
    expect(only(events, "thinking_delta")).toEqual([{ type: "thinking_delta", message_id: thinking?.message_id as string, text: "**Counting the listed primes**" }]);
  });

  test("a guarded command shows up as a notice", () => {
    const events = mapAll("guard");
    const notices = only(events, "notice");
    expect(notices).toHaveLength(1);
    expect(notices[0]?.level).toBe("warning");
    expect(notices[0]?.message).toStartWith("A command was blocked: `git worktree add` is disabled");
    expect(only(events, "tool_call")).toEqual([]);
  });

  test("an interrupt closes the running command and ends the turn as interrupted", () => {
    const events = mapAll("interrupt");
    const [call] = only(events, "tool_call");
    expect(call).toMatchObject({ name: "Bash", input: { command: "sleep 30" } });
    expect(only(events, "tool_result")).toEqual([{ type: "tool_result", tool_use_id: call?.tool_use_id as string, output: "[interrupted]", is_error: true }]);
    expect(events.at(-1)).toMatchObject({ type: "turn_ended", outcome: "interrupted", error: null });
  });

  test("commands run through code mode's exec tool show with their output and exit status", () => {
    // gpt-6-luna, asked for a short, a failing, a long and a polled background command, a file,
    // and a web search: every call from its `commandExecution` item, nothing reported unknown.
    const events = mapAll("commands");
    expect(only(events, "notice")).toEqual([]);
    const calls = only(events, "tool_call");
    expect(calls.map((call) => [call.name, call.input])).toEqual([
      ["Bash", { command: "echo hello-short", cwd: "/tmp/ws" }],
      ["Bash", { command: "ls /definitely-not-here", cwd: "/tmp/ws" }],
      ["Bash", { command: "sleep 6; echo long-done", cwd: "/tmp/ws" }],
      ["Bash", { command: "sleep 15; echo bg-done", cwd: "/tmp/ws" }],
      ["apply_patch", { changes: [{ path: "/tmp/ws/alpha/notes.txt", kind: { type: "add" }, diff: "hi\n" }] }],
      ["WebSearch", { query: "site:github.com/openai/codex/releases latest Codex CLI release" }],
    ]);
    const results = new Map(only(events, "tool_result").map((result) => [result.tool_use_id, result]));
    expect(calls.map((call) => results.get(call.tool_use_id)?.is_error)).toEqual([false, true, false, false, false, false]);
    expect(results.get(calls[0]?.tool_use_id as string)?.output).toBe("hello-short\n");
    // As Claude Code's Bash reports a failure.
    expect(results.get(calls[1]?.tool_use_id as string)?.output).toBe("Exit code 1\nls: /definitely-not-here: No such file or directory\n");
    expect(results.get(calls[3]?.tool_use_id as string)?.output).toBe("bg-done\n");
    // Each call comes before its result, once.
    expect(new Set(calls.map((call) => call.tool_use_id)).size).toBe(calls.length);
    for (const call of calls) expect(events.indexOf(call)).toBeLessThan(events.indexOf(results.get(call.tool_use_id) as ChatEvent));
  });

  test("a command left running past its turn ends later under the same call", () => {
    // `sleep 40 … &`: Codex completes its item once the process exits, after `turn/completed`.
    const events = mapAll("background");
    const turnEnd = events.findIndex((event) => event.type === "turn_ended");
    const commands = only(events, "tool_call").filter((call) => call.name === "Bash");
    expect(commands).toHaveLength(1);
    const id = commands[0]?.tool_use_id as string;
    const results = only(events, "tool_result").filter((result) => result.tool_use_id === id);
    expect(results).toEqual([
      { type: "tool_result", tool_use_id: id, output: "[still running in the background]", is_error: false },
      { type: "tool_result", tool_use_id: id, output: "zsh:1: nice(5) failed: operation not permitted\n", is_error: false },
    ]);
    expect(events.indexOf(results[0] as ChatEvent)).toBeLessThan(turnEnd);
    expect(events.indexOf(results[1] as ChatEvent)).toBeGreaterThan(turnEnd);
    // The clock tool's wait is a step of its own.
    expect(only(events, "tool_call").find((call) => call.name === "Sleep")?.input).toEqual({ duration_ms: 2000, description: "Wait 2 s" });
  });

  test("a delegate call left open by a completed turn shows its delegated chat working on; an interrupt does not", () => {
    const item = { type: "dynamicToolCall", id: "call-d", namespace: "chauffeur", tool: "delegate", arguments: { agent: "codex", prompt: "Look" }, status: "inProgress", contentItems: null, success: null, durationMs: null };
    const ended = (status: "completed" | "interrupted") => {
      const mapper = new CodexEventMapper();
      mapper.map({ method: "turn/started", params: { threadId: "t", turn: { id: "turn-1", items: [], status: "inProgress", error: null } } } as never);
      mapper.map({ method: "item/started", params: { threadId: "t", turnId: "turn-1", startedAtMs: 1, item } } as never);
      const events = mapper.map({ method: "turn/completed", params: { threadId: "t", turn: { id: "turn-1", items: [], status, error: null } } } as never);
      return only(events, "tool_result");
    };
    expect(ended("completed")).toEqual([
      { type: "tool_result", tool_use_id: "call-d", output: "[the delegated chat works on: its answer will come as a new message]", is_error: false },
    ]);
    expect(ended("interrupted")).toEqual([{ type: "tool_result", tool_use_id: "call-d", output: "[interrupted]", is_error: true }]);
  });

  test("a command the sandbox refused before it ran has no item: only its escalated retry shows", () => {
    // Codex reports the refused attempt to the model only (its PreToolUse hook ran, no item).
    const messages = recorded("sandbox-denied");
    const hooked = messages.flatMap((message) =>
      message.method === "hook/completed" && message.params.run.eventName === "preToolUse" ? [message.params.run.id.split(":").at(-1)] : [],
    );
    const events = mapAll("sandbox-denied");
    const calls = only(events, "tool_call");
    expect(hooked).toHaveLength(2);
    expect(calls.map((call) => call.tool_use_id)).toEqual([hooked[1] as string]);
    expect(calls[0]?.input).toEqual({ command: "git -C alpha commit --allow-empty -m approval-test .", cwd: "/tmp/ws" });
    expect(only(events, "tool_result")).toEqual([{ type: "tool_result", tool_use_id: hooked[1] as string, output: "[cx/cx-ask 9fd97e1] approval-test\n", is_error: false }]);
    expect(only(events, "notice")).toEqual([]);
  });

  test("a local model's missing metadata says nothing; other warnings still show", () => {
    const mapper = new CodexEventMapper();
    mapper.local = true;
    const warning = (message: string) => ({ method: "warning", params: { threadId: "t", message } }) as ServerNotification;
    expect(mapper.map(warning("Model metadata for `qwen3.8` not found. Defaulting to fallback metadata; this can degrade performance and cause issues."))).toEqual([]);
    expect(mapper.map(warning("Something else"))).toEqual([{ type: "notice", level: "warning", message: "Something else" }]);
  });

  test("a failed turn carries the API's message; the warning becomes a notice", () => {
    const events = mapAll("failure");
    expect(only(events, "notice")).toEqual([
      { type: "notice", level: "warning", message: expect.stringContaining("Model metadata for `gpt-does-not-exist` not found") },
    ]);
    expect(events.at(-1)).toMatchObject({
      type: "turn_ended",
      outcome: "error",
      error: "The 'gpt-does-not-exist' model is not supported when using Codex with a ChatGPT account.",
    });
  });
});

describe("mapping details", () => {
  test("unwraps the shell Codex runs commands in", () => {
    expect(shellScript("/bin/zsh -lc 'git status'")).toBe("git status");
    expect(shellScript(`/bin/bash -c "echo 'a b' | wc -c"`)).toBe("echo 'a b' | wc -c");
    expect(shellScript("git status")).toBe("git status");
    expect(shellScript("/bin/zsh -lc 'a' extra")).toBe("/bin/zsh -lc 'a' extra");
  });

  test("keeps the human part of wrapped API errors", () => {
    expect(errorText('{"type":"error","status":400,"error":{"message":"nope"}}')).toBe("nope");
    expect(errorText("plain")).toBe("plain");
  });

  test("Codex read-state changes do not add notices to the conversation", () => {
    const mapper = new CodexEventMapper();
    // Newer Codex versions send this notification before our generated types include it.
    const notification = {
      method: "thread/readState/changed",
      params: { threadId: "t", readState: { firstUnread: null, revision: "1" } },
    } as unknown as ServerNotification;
    expect(mapper.map(notification)).toEqual([]);
    expect(mapper.map(notification)).toEqual([]);
  });

  test("retried errors are notices, unknown notifications are reported once", () => {
    const mapper = new CodexEventMapper();
    const retry = { method: "error", params: { error: { message: "overloaded", codexErrorInfo: null, additionalDetails: null, misalignment: null }, willRetry: true, threadId: "t", turnId: "u" } };
    expect(mapper.map(retry as ServerNotification)).toEqual([{ type: "notice", level: "warning", message: "Codex error, retrying: overloaded" }]);
    const brandNew = { method: "brand/new", params: {} } as unknown as ServerNotification;
    expect(mapper.map(brandNew)).toHaveLength(1);
    expect(mapper.map(brandNew)).toEqual([]);
  });

  test("auto review's denials are notices, its approvals are not", () => {
    const mapper = new CodexEventMapper();
    const warning = (message: string) => ({ method: "guardianWarning", params: { threadId: "t", message } }) as ServerNotification;
    expect(mapper.map(warning("Automatic approval review approved (risk: medium, authorization: high): committing a.txt"))).toEqual([]);
    expect(mapper.map(warning("Automatic approval review denied (risk: high): force push to main"))).toEqual([
      { type: "notice", level: "warning", message: "Codex: Automatic approval review denied (risk: high): force push to main" },
    ]);
  });

  test("the image Codex looked at goes to the daemon as its file", () => {
    const mapper = new CodexEventMapper();
    const item = { type: "imageView", id: "v1", path: "/tmp/ws/shot.png" };
    mapper.map({ method: "item/started", params: { item, threadId: "t", turnId: "u", startedAtMs: 1 } } as ServerNotification);
    expect(mapper.map({ method: "item/completed", params: { item, threadId: "t", turnId: "u", completedAtMs: 2 } } as ServerNotification)).toEqual([
      { type: "tool_result", tool_use_id: "v1", output: "", is_error: false, images: [{ path: "/tmp/ws/shot.png" }] },
    ]);
  });

  test("a web search's call waits for its query; an opened page is named by its URL", () => {
    const mapper = new CodexEventMapper();
    const started = { type: "webSearch", id: "w1", query: "", action: null, results: null };
    expect(mapper.map({ method: "item/started", params: { item: started, threadId: "t", turnId: "u", startedAtMs: 1 } } as ServerNotification)).toEqual([]);
    const opened = { ...started, query: "", action: { type: "openPage", url: "https://example.com/a" }, results: [] };
    expect(mapper.map({ method: "item/completed", params: { item: opened, threadId: "t", turnId: "u", completedAtMs: 2 } } as ServerNotification)).toEqual([
      { type: "tool_call", tool_use_id: "w1", name: "WebSearch", input: { url: "https://example.com/a" }, parent_tool_use_id: null },
      { type: "tool_result", tool_use_id: "w1", output: "[]", is_error: false },
    ]);
  });

  test("a generated image goes to the daemon as the file Codex saved", () => {
    const mapper = new CodexEventMapper();
    const item = { type: "imageGeneration", id: "g1", status: "completed", revisedPrompt: "a red square", result: "iVBOR", failure: null, savedPath: "/tmp/codex/g1.png" };
    expect(mapper.map({ method: "item/completed", params: { item, threadId: "t", turnId: "u", completedAtMs: 2 } } as ServerNotification)).toEqual([
      { type: "tool_call", tool_use_id: "g1", name: "ImageGeneration", input: { prompt: "a red square", description: "a red square" }, parent_tool_use_id: null },
      { type: "tool_result", tool_use_id: "g1", output: "/tmp/codex/g1.png", is_error: false, images: [{ path: "/tmp/codex/g1.png" }] },
    ]);
  });

  test("an interrupted command's late completion replaces its result, without a second call", () => {
    const mapper = new CodexEventMapper();
    const item = { type: "commandExecution", id: "c1", pluginId: null, scriptPath: null, command: "/bin/zsh -lc 'sleep 30'", cwd: "/tmp/ws", processId: "1", source: "agent", status: "inProgress", commandActions: [], aggregatedOutput: null, exitCode: null, durationMs: null };
    const turn = (status: string) => ({ id: "u", rootTurnId: "u", items: [], itemsView: "notLoaded", status, error: null, startedAt: 1, completedAt: 2, durationMs: 1000 });
    mapper.map({ method: "item/started", params: { item, threadId: "t", turnId: "u", startedAtMs: 1 } } as ServerNotification);
    expect(only(mapper.map({ method: "turn/completed", params: { threadId: "t", turn: turn("interrupted") } } as ServerNotification), "tool_result")).toEqual([
      { type: "tool_result", tool_use_id: "c1", output: "[interrupted]", is_error: true },
    ]);
    const done = { ...item, status: "failed", exitCode: 130, aggregatedOutput: "" };
    expect(mapper.map({ method: "item/completed", params: { item: done, threadId: "t", turnId: "u", completedAtMs: 3 } } as ServerNotification)).toEqual([
      { type: "tool_result", tool_use_id: "c1", output: "Exit code 130\n", is_error: true },
    ]);
  });

  test("an MCP tool call is named like Claude's MCP tools", () => {
    const mapper = new CodexEventMapper();
    const item = { type: "mcpToolCall", id: "m1", server: "linear", tool: "get_issue", status: "inProgress", arguments: { id: "XYZ-1" }, appContext: null, mcpAppUi: null, pluginId: null, readOnlyHint: true, result: null, error: null, durationMs: null };
    expect(mapper.map({ method: "item/started", params: { item, threadId: "t", turnId: "u", startedAtMs: 1 } } as ServerNotification)).toEqual([
      { type: "tool_call", tool_use_id: "m1", name: "mcp__linear__get_issue", input: { id: "XYZ-1" }, parent_tool_use_id: null },
    ]);
    const done = { ...item, status: "completed", result: { content: [{ type: "text", text: "XYZ-1: fix" }], structuredContent: null, _meta: null } };
    expect(mapper.map({ method: "item/completed", params: { item: done, threadId: "t", turnId: "u", completedAtMs: 2 } } as ServerNotification)).toEqual([
      { type: "tool_result", tool_use_id: "m1", output: "XYZ-1: fix", is_error: false },
    ]);
  });
});

describe("the plan", () => {
  test("Codex's plan becomes plan_updated, its statuses as the daemon names them", () => {
    const mapper = new CodexEventMapper();
    const events = mapper.map({
      method: "turn/plan/updated",
      params: {
        threadId: "th",
        turnId: "tu",
        explanation: null,
        plan: [
          { step: "Read the code", status: "completed" },
          { step: "Change it", status: "inProgress" },
          { step: "Test", status: "pending" },
        ],
      },
    } as ServerNotification);
    expect(events).toEqual([
      {
        type: "plan_updated",
        steps: [
          { text: "Read the code", status: "completed" },
          { text: "Change it", status: "in_progress" },
          { text: "Test", status: "pending" },
        ],
      },
    ]);
  });
});

test("a resumed Codex thread estimates new calls once, across turns and model changes", () => {
  const mapper = new CodexEventMapper();
  mapper.model = "gpt-6.1-sol";
  const turn = (id: string, status: "inProgress" | "completed") => ({
    id, status, rootTurnId: null, itemsView: "full" as const, items: [], error: null, startedAt: null, completedAt: null, durationMs: null,
  });
  const start = (id: string) => mapper.map({ method: "turn/started", params: { threadId: "t", turn: turn(id, "inProgress") } });
  const end = (id: string) => only(mapper.map({ method: "turn/completed", params: { threadId: "t", turn: turn(id, "completed") } }), "turn_ended")[0];
  const usage = (id: string, total: number, last: number) => mapper.map({ method: "thread/tokenUsage/updated", params: {
    threadId: "t", turnId: id, tokenUsage: { modelContextWindow: 500_000,
      total: { inputTokens: total, outputTokens: 0, totalTokens: total, cachedInputTokens: 0, cacheWriteInputTokens: 0, reasoningOutputTokens: 0 },
      last: { inputTokens: last, outputTokens: 0, totalTokens: last, cachedInputTokens: 0, cacheWriteInputTokens: 0, reasoningOutputTokens: 0 },
    },
  } });
  start("one");
  usage("one", 1_100_000, 100_000);
  usage("one", 1_200_000, 100_000);
  usage("one", 1_200_000, 100_000);
  const one = end("one");
  expect(one?.estimated_cost_usd).toBeCloseTo(0.4);
  // Each call counts once; the thread's total, history included, does not.
  expect(one?.tokens).toBe(200_000);
  mapper.model = "gpt-6-luna";
  mapper.fast = true;
  start("two");
  usage("two", 1_300_000, 100_000);
  expect(end("two")?.estimated_cost_usd).toBeCloseTo(0.02);
  start("three");
  const three = end("three");
  expect(three?.estimated_cost_usd).toBeUndefined();
  expect(three?.tokens).toBeUndefined();
  mapper.model = "unknown";
  start("four");
  usage("four", 1_400_000, 100_000);
  mapper.model = "gpt-6-luna";
  usage("four", 1_500_000, 100_000);
  expect(end("four")?.estimated_cost_usd).toBeUndefined();
  mapper.local = true;
  start("five");
  usage("five", 1_600_000, 100_000);
  const five = end("five");
  expect(five?.estimated_cost_usd).toBeUndefined();
  // A local model has no price, but its tokens count.
  expect(five?.tokens).toBe(100_000);
});
