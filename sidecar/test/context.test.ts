import { describe, expect, test } from "bun:test";
import type { SDKControlGetContextUsageResponse } from "@anthropic-ai/claude-agent-sdk";
import { contextUsageOf } from "../src/agents/claude/context.ts";
import { ClaudeSession } from "../src/agents/claude/session.ts";
import { RpcError } from "../src/rpc/peer.ts";
import type { SessionStartParams } from "../src/protocol.ts";
import { FakeHost, FakeQuery, flush, sdk } from "./support/fakes.ts";

/** A reading as Claude Code 2.x answers it (trimmed), on a 1M window. */
const READING = {
  categories: [
    { name: "System prompt", tokens: 141.4, color: "promptBorder", kind: "used" },
    { name: "System tools", tokens: 4999, color: "inactive", kind: "used" },
    { name: "MCP tools (deferred)", tokens: 581_503, color: "inactive", isDeferred: true, kind: "deferred" },
    { name: "Memory files", tokens: 14, color: "claude", kind: "used" },
    { name: "Skills", tokens: 0, color: "warning", kind: "used" },
    { name: "Messages", tokens: 6013, color: "purple", kind: "used" },
    { name: "Autocompact buffer", tokens: 33_000, color: "inactive", kind: "buffer" },
    { name: "Free space", tokens: 942_304, color: "promptBorder", kind: "free" },
  ],
  totalTokens: 24_696,
  maxTokens: 1_000_000,
  rawMaxTokens: 1_000_000,
  percentage: 2,
  gridRows: [[{ color: "x", isFilled: true, categoryName: "Messages", tokens: 1, percentage: 1, squareFullness: 1 }]],
  model: "claude-haiku-5-5",
  memoryFiles: [{ path: "/w/web/CLAUDE.md", type: "Project", tokens: 14 }],
  mcpTools: [
    { name: "mcp__linear__get_issue", serverName: "linear", tokens: 264, isLoaded: false },
    { name: "mcp__claap__list_users", serverName: "claap", tokens: 300 },
  ],
  agents: [{ agentType: "reviewer", source: "projectSettings", tokens: 40 }],
  skills: { totalSkills: 1, includedSkills: 1, tokens: 80, skillFrontmatter: [{ name: "release", source: "projectSettings", tokens: 80 }] },
  autoCompactThreshold: 967_000,
  isAutoCompactEnabled: true,
  apiUsage: null,
} as unknown as SDKControlGetContextUsageResponse;

describe("a /context reading as a ContextUsage", () => {
  test("the summary: totals, every category in order, Claude Code's autocompact, no lists", () => {
    const usage = contextUsageOf(READING);
    expect(usage).toEqual({
      model: "claude-haiku-5-5",
      used_tokens: 24_696,
      max_tokens: 1_000_000,
      percent: 2,
      over_limit: false,
      categories: [
        { name: "System prompt", tokens: 141, kind: "used" },
        { name: "System tools", tokens: 4999, kind: "used" },
        { name: "MCP tools (deferred)", tokens: 581_503, kind: "deferred" },
        { name: "Memory files", tokens: 14, kind: "used" },
        { name: "Skills", tokens: 0, kind: "used" },
        { name: "Messages", tokens: 6013, kind: "used" },
        { name: "Autocompact buffer", tokens: 33_000, kind: "buffer" },
        { name: "Free space", tokens: 942_304, kind: "free" },
      ],
      compaction: { by: "claude_code", threshold_percent: 96.7 },
    });
    // The grid and the per-item lists stay out of the per-turn event.
    expect(JSON.stringify(usage)).not.toContain("gridRows");
    expect(usage.details).toBeUndefined();
  });

  test("Jev's threshold when Jev compacts the chat", () => {
    expect(contextUsageOf(READING, { jevThreshold: 60 }).compaction).toEqual({ by: "jev", threshold_percent: 60 });
  });

  test("without a threshold, the buffer says where autocompact starts; none when it is off", () => {
    expect(contextUsageOf({ ...READING, autoCompactThreshold: undefined }).compaction).toEqual({ by: "claude_code", threshold_percent: 96.7 });
    expect(contextUsageOf({ ...READING, isAutoCompactEnabled: false }).compaction).toBeNull();
  });

  test("over the window", () => {
    const usage = contextUsageOf({ ...READING, totalTokens: 1_020_000, percentage: 102 });
    expect(usage).toMatchObject({ over_limit: true, percent: 102 });
  });

  test("the full detail lists MCP tools (deferred or loaded), memory files, skills and agents", () => {
    expect(contextUsageOf(READING, { details: true }).details).toEqual({
      mcp_tools: [
        { name: "mcp__linear__get_issue", server: "linear", tokens: 264, loaded: false },
        { name: "mcp__claap__list_users", server: "claap", tokens: 300, loaded: true },
      ],
      memory_files: [{ path: "/w/web/CLAUDE.md", source: "Project", tokens: 14 }],
      skills: [{ name: "release", source: "projectSettings", tokens: 80 }],
      agents: [{ name: "reviewer", source: "projectSettings", tokens: 40 }],
    });
  });

  test("a reading missing its fields still maps", () => {
    const usage = contextUsageOf({ percentage: 12 } as SDKControlGetContextUsageResponse, { details: true });
    expect(usage).toMatchObject({ used_tokens: 0, max_tokens: 0, percent: 12, categories: [], compaction: null });
    expect(usage.details).toEqual({ mcp_tools: [], memory_files: [], skills: [], agents: [] });
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

async function started(params: Partial<SessionStartParams> = {}) {
  const host = new FakeHost();
  let query: FakeQuery | undefined;
  const session = new ClaudeSession({ ...BASE, ...params }, host, {
    query: ({ prompt, options }) => (query = new FakeQuery(prompt, options)),
    resolveExecutable: () => "/opt/claude",
    baseEnv: { PATH: "/usr/bin" },
    initTimeoutMs: 200,
    stopTimeoutMs: 200,
    contextTimeoutMs: 50,
    jevPluginDir: () => "/tmp/jev-plugin",
  });
  await session.start();
  if (!query) throw new Error("no query");
  query.contextPercentage = 30;
  return { session, host, query };
}

describe("the session's context readings", () => {
  test("on demand, in the detail asked, with the lists in a full one", async () => {
    const { session, query } = await started({ jev: { api_key: "k", threshold_percent: 60 } });
    const summary = await session.contextUsage("summary");
    expect(summary).toMatchObject({ percent: 30, used_tokens: 60_000, max_tokens: 200_000, compaction: { by: "jev", threshold_percent: 60 } });
    expect(summary.details).toBeUndefined();
    const full = await session.contextUsage("full");
    expect(full.details?.mcp_tools.map((t) => t.server)).toEqual(["linear", "linear"]);
    expect(full.details?.memory_files).toEqual([{ path: "/w/CLAUDE.md", source: "Project", tokens: 120 }]);
    expect(query.contextReads).toEqual(["summary", "full"]);
  });

  test("refused once the session stopped; a failed read says why", async () => {
    const { session, query } = await started();
    query.contextError = new Error("control request failed");
    expect(session.contextUsage("full")).rejects.toThrow("reading the context failed: control request failed");
    await session.stop();
    const stopped = await session.contextUsage("summary").catch((error: unknown) => error);
    expect(stopped).toBeInstanceOf(RpcError);
    expect((stopped as RpcError).code).toBe(404);
  });

  test("a turn still ends when the reading fails or hangs, without a reading", async () => {
    const { session, host, query } = await started();
    query.contextError = new Error("no");
    session.send("hi");
    await query.waitForSent(1);
    query.push(sdk.text("m1", "hello"), sdk.result());
    await flush();
    expect(host.types()).toEqual(["turn_started", "assistant_text", "turn_ended"]);

    query.contextError = undefined;
    query.getContextUsage = () => new Promise(() => {});
    session.send("again");
    await query.waitForSent(2);
    query.push(sdk.text("m2", "ok"), sdk.result());
    await new Promise((resolve) => setTimeout(resolve, 80));
    await flush();
    expect(host.types().slice(-3)).toEqual(["turn_started", "assistant_text", "turn_ended"]);
  });
});
