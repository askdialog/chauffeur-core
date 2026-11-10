import { describe, expect, test } from "bun:test";
import type { AgentUsage, ChatInfo, EnvReloadResult, McpServer } from "@chauffeur/protocol";
import {
  awaitedAfterReload,
  brokenConnectors,
  exportLine,
  lacksEnv,
  loginCommand,
  mcpLoginCommand,
  mcpStatusText,
  newlyNeeded,
  profileName,
  reloadSummary,
  settledServers,
  signedOutSince,
  sortedServers,
} from "./connectors";

const server = (name: string, status: McpServer["status"], more: Partial<McpServer> = {}): McpServer => ({
  name,
  status,
  error: null,
  failed_call: false,
  missing_env: [],
  uses_env: [],
  declared_in: null,
  ...more,
});

const reload = (more: Partial<EnvReloadResult> = {}): EnvReloadResult => ({
  shell: "/bin/zsh",
  profile: "~/.zshenv",
  added: [],
  changed: [],
  removed: [],
  still_missing: [],
  restarted: [],
  deferred: [],
  ...more,
});

const chat = (id: string, agent: ChatInfo["agent"], mcp_servers: McpServer[], more: Partial<ChatInfo> = {}): ChatInfo => ({
  id,
  kind: "chat",
  workspace_id: "w",
  agent,
  model: null,
  effort: null,
  fast: false,
  permission_mode: "auto",
  title: id,
  status: "idle",
  agent_session_id: null,
  created_at: "2026-10-18T10:00:00Z",
  running_tasks: 0,
  closed: false,
  last_activity_at: null,
  mcp_servers,
  ...more,
});

const usage = (agent: AgentUsage["agent"], account: AgentUsage["account"]): AgentUsage => ({
  agent,
  plan: "max",
  windows: [],
  error: null,
  read_at: null,
  failed_at: null,
  extra_usage: null,
  account,
  account_error: null,
});

describe("connectors", () => {
  test("a server's status in words", () => {
    expect(mcpStatusText(server("linear", "needs_auth"))).toBe("Needs you to log in again");
    expect(mcpStatusText(server("x", "failed", { error: "spawn ENOENT" }))).toBe("Failed: spawn ENOENT");
    expect(mcpStatusText(server("x", "failed"))).toBe("Failed to connect");
  });

  test("broken servers come first", () => {
    const sorted = sortedServers([server("0", "disabled"), server("a", "connected"), server("b", "pending"), server("c", "failed"), server("d", "needs_auth")]);
    expect(sorted.map((s) => s.name)).toEqual(["c", "d", "b", "a", "0"]);
  });

  test("a server is newly needed when a tool call needed it, once", () => {
    const before = [server("linear", "needs_auth")];
    const after = [server("linear", "needs_auth", { failed_call: true })];
    expect(newlyNeeded(before, after).map((s) => s.name)).toEqual(["linear"]);
    expect(newlyNeeded(after, after)).toEqual([]);
    expect(newlyNeeded(undefined, [server("neon", "failed")])).toEqual([]);
  });

  test("an agent's broken servers across its open chats, each once, the chat that needed it first", () => {
    const chats = [
      chat("a", "claude", [server("claude.ai Linear", "needs_auth"), server("gitnexus", "connected")]),
      chat("b", "claude", [server("claude.ai Linear", "needs_auth", { failed_call: true })]),
      chat("c", "codex", [server("neon", "needs_auth")]),
      chat("d", "claude", [server("playwright", "failed")], { closed: true }),
    ];
    const claude = brokenConnectors(chats, "claude");
    expect(claude.map((c) => [c.server.name, c.chat.id])).toEqual([["claude.ai Linear", "b"]]);
    expect(brokenConnectors(chats, "codex").map((c) => c.server.name)).toEqual(["neon"]);
  });

  test("the commands that log in from a terminal", () => {
    expect(loginCommand("claude")).toBe("claude auth login");
    expect(loginCommand("codex")).toBe("codex login");
    expect(mcpLoginCommand("claude", "claude.ai Linear")).toBe("claude /mcp");
    expect(mcpLoginCommand("codex", "neon")).toBe("codex mcp login neon");
    expect(mcpLoginCommand("codex", "it's")).toBe(`codex mcp login 'it'\\''s'`);
  });

  test("a server lacking variables asks for them rather than shows its error", () => {
    const slack = server("slack", "failed", {
      error: "MCP startup failed: Environment variable SLACK_MCP_TOKEN for MCP server 'slack' is not set",
      missing_env: ["SLACK_MCP_TOKEN"],
    });
    expect(lacksEnv(slack)).toBe(true);
    expect(mcpStatusText(slack)).toBe("Needs SLACK_MCP_TOKEN");
    expect(mcpStatusText({ ...slack, missing_env: ["A", "B", "C"] })).toBe("Needs A, B and C");
    // Connected again, whatever it lacked before.
    expect(lacksEnv({ ...slack, status: "connected" })).toBe(false);
    expect(exportLine("SLACK_MCP_TOKEN")).toBe("export SLACK_MCP_TOKEN=…");
    expect(profileName("~/.zshenv")).toBe("~/.zshenv");
    expect(profileName(undefined)).toBe("your shell's profile");
  });

  test("a reload says what is still missing, else what it restarted", () => {
    expect(reloadSummary(reload({ still_missing: ["SLACK_MCP_TOKEN"] }))).toEqual({
      message: "SLACK_MCP_TOKEN is still not set in ~/.zshenv's environment",
      level: "error",
    });
    expect(reloadSummary(reload()).message).toBe("Environment reloaded: nothing changed in ~/.zshenv");
    expect(reloadSummary(reload({ added: ["SLACK_MCP_TOKEN"], restarted: ["a"], deferred: ["b", "c"] })).message).toBe(
      "Environment reloaded (1 variable changed): restarting 1 chat for its MCP servers: 2 working chats get it at their next message",
    );
  });

  test("the servers a reload restarted chats for are announced once they connect or fail again", () => {
    const slack = server("slack", "failed", { missing_env: ["SLACK_MCP_TOKEN"], uses_env: ["SLACK_MCP_TOKEN"] });
    // Broken for another reason, or on a variable the reload left alone: not awaited.
    const timeout = server("timeout", "failed", { error: "timed out" });
    const neon = server("neon", "failed", { uses_env: ["NEON_TOKEN"] });
    const linear = server("linear", "failed", { uses_env: ["LINEAR_TOKEN"] });
    const before = { a: chat("a", "codex", [slack, server("ok", "connected"), timeout, neon, linear]), b: chat("b", "codex", [slack]) };
    const awaited = awaitedAfterReload(reload({ added: ["SLACK_MCP_TOKEN"], changed: ["LINEAR_TOKEN"], removed: ["NEON_TOKEN"], restarted: ["a"], deferred: ["b"] }), before);
    expect(awaited).toEqual([
      { chatId: "a", server: "slack" },
      { chatId: "a", server: "linear" },
    ]);
    // The new session has not reported it yet, then it is starting: still awaited.
    expect(settledServers(awaited, chat("a", "codex", [])).rest).toEqual(awaited);
    expect(settledServers(awaited, chat("a", "codex", [server("slack", "pending")])).rest).toEqual(awaited);
    expect(settledServers(awaited, chat("b", "codex", [server("slack", "connected")])).connected).toEqual([]);
    expect(settledServers(awaited, chat("a", "codex", [server("slack", "connected")]))).toEqual({ connected: ["slack"], broken: [], rest: [{ chatId: "a", server: "linear" }] });
    expect(settledServers(awaited, chat("a", "codex", [slack])).broken).toEqual([slack]);
  });

  test("an agent is newly signed out between two readings, not at the first one", () => {
    const before = [usage("claude", "signed_in"), usage("codex", "unknown")];
    expect(signedOutSince(null, [usage("claude", "signed_out")])).toEqual([]);
    expect(signedOutSince(before, [usage("claude", "signed_out"), usage("codex", "signed_out")]).map((u) => u.agent)).toEqual(["claude", "codex"]);
    expect(signedOutSince([usage("claude", "signed_out")], [usage("claude", "signed_out")])).toEqual([]);
  });
});
