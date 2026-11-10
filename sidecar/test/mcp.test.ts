import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { codexMcpServer, CodexSession } from "../src/agents/codex/session.ts";
import { mcpServerOf } from "../src/agents/claude/session.ts";
import { McpTracker, mcpToolServer, normalizeServerName } from "../src/agents/mcp.ts";
import type { ChatEvent, McpServer, SessionStartParams } from "../src/protocol.ts";
import { ClaudeSession } from "../src/agents/claude/session.ts";
import { codex, FakeAppServer, HOOK, THREAD_ID } from "./support/codex-fakes.ts";
import { FakeHost, FakeQuery, flush, sdk } from "./support/fakes.ts";

const server = (name: string, status: McpServer["status"], error: string | null = null): McpServer => ({
  name,
  status,
  error,
  failed_call: false,
  missing_env: [],
  uses_env: [],
  declared_in: null,
});

/** A tracker on a manual clock: timers run when the test says. */
function tracker(read: () => McpServer[]) {
  const events: ChatEvent[] = [];
  const timers: Array<{ at: number; fn: () => void }> = [];
  let now = 0;
  const mcp = new McpTracker({
    emit: (event) => events.push(event),
    read: async () => read(),
    setTimeout: (fn, ms) => {
      const timer = { at: now + ms, fn };
      timers.push(timer);
      return timer;
    },
    clearTimeout: (handle) => {
      const index = timers.indexOf(handle as (typeof timers)[number]);
      if (index >= 0) timers.splice(index, 1);
    },
    now: () => now,
  });
  const advance = async (ms: number) => {
    now += ms;
    for (const timer of timers.filter((t) => t.at <= now)) {
      timers.splice(timers.indexOf(timer), 1);
      timer.fn();
    }
    await flush();
  };
  const statuses = () => events.filter((e) => e.type === "mcp_status").map((e) => (e.type === "mcp_status" ? e.servers : []));
  return { mcp, events, statuses, advance, timers };
}

describe("MCP servers", () => {
  test("tool names name their server with characters outside [A-Za-z0-9_-] as _", () => {
    expect(mcpToolServer("mcp__claude_ai_Linear__list_issues")).toBe("claude_ai_Linear");
    expect(mcpToolServer("Bash")).toBeUndefined();
    expect(normalizeServerName("claude.ai Linear")).toBe("claude_ai_Linear");
  });

  test("tells the daemon when the servers change, not when they read the same", async () => {
    let servers = [server("linear", "pending")];
    const { mcp, statuses, advance } = tracker(() => servers);
    mcp.start();
    await flush();
    expect(statuses()).toEqual([[server("linear", "pending")]]);
    // Still connecting: read again a few seconds later.
    servers = [server("linear", "needs_auth")];
    await advance(3_000);
    expect(statuses().at(-1)).toEqual([server("linear", "needs_auth")]);
    await mcp.refresh();
    expect(statuses()).toHaveLength(2);
  });

  test("a tool call that fails on a broken server marks it needed until it connects again", async () => {
    let servers = [server("claude.ai Linear", "connected")];
    const { mcp, statuses } = tracker(() => servers);
    mcp.start();
    await flush();
    mcp.observe({ type: "tool_call", tool_use_id: "t1", name: "mcp__claude_ai_Linear__list_issues", input: {}, parent_tool_use_id: null });
    servers = [server("claude.ai Linear", "needs_auth")];
    mcp.observe({ type: "tool_result", tool_use_id: "t1", output: "Unauthorized", is_error: true });
    await flush();
    expect(statuses().at(-1)).toEqual([{ ...server("claude.ai Linear", "needs_auth"), failed_call: true }]);
    servers = [server("claude.ai Linear", "connected")];
    await mcp.refresh();
    expect(statuses().at(-1)).toEqual([server("claude.ai Linear", "connected")]);
  });

  test("a failed call on a server that still works marks nothing", async () => {
    const servers = [server("linear", "connected")];
    const { mcp, statuses } = tracker(() => servers);
    mcp.start();
    await flush();
    mcp.observe({ type: "tool_call", tool_use_id: "t1", name: "mcp__linear__get_issue", input: {}, parent_tool_use_id: null });
    mcp.observe({ type: "tool_result", tool_use_id: "t1", output: "Issue not found", is_error: true });
    await flush();
    expect(statuses()).toEqual([[server("linear", "connected")]]);
  });

  test("a login is followed until the server connects", async () => {
    let servers = [server("linear", "needs_auth")];
    let ticks = 0;
    const { mcp, statuses, advance, timers } = tracker(() => servers);
    mcp.start();
    await flush();
    mcp.watchLogin("linear", async () => void ticks++);
    await advance(3_000);
    expect(ticks).toBe(1);
    servers = [server("linear", "connected")];
    await advance(3_000);
    expect(statuses().at(-1)).toEqual([server("linear", "connected")]);
    expect(timers).toHaveLength(0);
  });

  test("Claude Code's and Codex's statuses map onto Chauffeur's", () => {
    expect(mcpServerOf({ name: "claude.ai Linear", status: "needs-auth" })).toEqual(server("claude.ai Linear", "needs_auth"));
    expect(mcpServerOf({ name: "x", status: "failed", error: "spawn ENOENT" })).toEqual(server("x", "failed", "spawn ENOENT"));
    const codexServer = (runtimeStatus: string | null, authStatus: string, toolsError: string | null = null) =>
      codexMcpServer({ name: "neon", runtimeStatus, authStatus, toolsError, pluginId: null, httpOrigin: null, serverInfo: null, serverCapabilities: null, tools: {}, resources: [], resourceTemplates: [] } as never);
    expect(codexServer("connected", "oAuth")).toEqual(server("neon", "connected"));
    expect(codexServer("authenticationRequired", "notLoggedIn")).toEqual(server("neon", "needs_auth"));
    expect(codexServer("starting", "unsupported")).toEqual(server("neon", "pending"));
    // Not started in a thread yet: its login and tools tell.
    expect(codexServer(null, "notLoggedIn", "MCP startup failed")).toEqual(server("neon", "needs_auth", "MCP startup failed"));
    expect(codexServer(null, "unsupported")).toEqual(server("neon", "connected"));
  });
});

const CLAUDE: SessionStartParams = {
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

async function claude(prepare: (query: FakeQuery) => void = () => {}, baseEnv: Record<string, string> = {}) {
  const host = new FakeHost();
  let query: FakeQuery | undefined;
  const session = new ClaudeSession(CLAUDE, host, {
    query: ({ prompt, options }) => {
      query = new FakeQuery(prompt, options);
      prepare(query);
      return query;
    },
    resolveExecutable: () => "/opt/claude",
    baseEnv,
    initTimeoutMs: 200,
    stopTimeoutMs: 200,
  });
  await session.start();
  await flush();
  return { session, host, query: query as FakeQuery };
}

describe("a Claude session's MCP servers and login", () => {
  test("reports its servers once started, not the SDK's in-process ones", async () => {
    const { host } = await claude((query) => {
      query.mcpServers = [
        { name: "claude.ai Linear", status: "needs-auth", scope: "claudeai" },
        { name: "chauffeur", status: "connected", source: "sdk" },
      ];
    });
    expect(host.events.filter((e) => e.type === "mcp_status")).toEqual([{ type: "mcp_status", servers: [server("claude.ai Linear", "needs_auth")] }]);
  });

  test("connecting a server that needs a login gives its page and follows it", async () => {
    const { session, query } = await claude((q) => {
      q.mcpServers = [{ name: "linear", status: "needs-auth" }];
    });
    expect(await session.mcpConnect("linear")).toEqual({ auth_url: "https://mcp.example.com/authorize" });
    expect(query.calls).toContain("mcpAuthenticate:linear");
    await session.stop();
  });

  test("connecting a failed server reconnects it", async () => {
    const { session, query } = await claude((q) => {
      q.mcpServers = [{ name: "gitnexus", status: "failed", error: "spawn ENOENT" }];
    });
    expect(await session.mcpConnect("gitnexus")).toEqual({ auth_url: null });
    expect(query.calls).toContain("reconnectMcpServer:gitnexus");
    await expect(session.mcpConnect("nope")).rejects.toThrow("no MCP server nope");
  });

  test("a failed server whose declaration needs an unset variable names it and its file", async () => {
    const home = mkdtempSync(join(tmpdir(), "chauffeur-mcp-env-"));
    try {
      // The status hides a stdio server's env: the declaration in ~/.claude.json has it.
      writeFileSync(
        join(home, ".claude.json"),
        JSON.stringify({ mcpServers: { slack: { type: "stdio", command: "slack-mcp", env: { TOKEN: "${SLACK_MCP_TOKEN}", TEAM: "${TEAM:-dialog}" } } } }),
      );
      const { host } = await claude(
        (query) => {
          query.mcpServers = [
            { name: "slack", status: "failed", error: "Connection closed", scope: "user", config: { type: "stdio", command: "slack-mcp", args: [] } },
            { name: "set", status: "failed", error: "Connection closed", scope: "user", config: { type: "http", url: "${SET_URL}/mcp" } },
          ];
        },
        { HOME: home, SET_URL: "https://mcp.example.com" },
      );
      expect(host.events.filter((e) => e.type === "mcp_status").at(-1)).toEqual({
        type: "mcp_status",
        servers: [
          { ...server("slack", "failed", "Connection closed"), missing_env: ["SLACK_MCP_TOKEN"], uses_env: ["SLACK_MCP_TOKEN", "TEAM"], declared_in: "~/.claude.json" },
          // Set: not missing, but a reload that changes it restarts the chat.
          { ...server("set", "failed", "Connection closed"), uses_env: ["SET_URL"] },
        ],
      });
    } finally {
      rmSync(home, { recursive: true, force: true });
    }
  });

  test("refused credentials tell the daemon once", async () => {
    const { host, query } = await claude();
    const refused = { ...sdk.text("m1", "Invalid API key · Please run /login"), error: "authentication_failed" } as never;
    query.push(refused, refused);
    await flush();
    expect(host.events.filter((e) => e.type === "account_lost")).toEqual([{ type: "account_lost", message: "Claude Code's login was refused" }]);
  });
});

async function codexSession(prepare: (server: FakeAppServer) => void = () => {}, baseEnv: Record<string, string> = {}, params: Partial<SessionStartParams> = {}) {
  const host = new FakeHost();
  let app: FakeAppServer | undefined;
  const session = new CodexSession({ ...CLAUDE, agent: "codex", cwd: "/w", ...params }, host, {
    spawn: (options) => {
      app = new FakeAppServer(options);
      prepare(app);
      return app;
    },
    resolveExecutable: () => "/usr/local/bin/codex",
    baseEnv,
    hookCommand: () => HOOK,
    initTimeoutMs: 500,
  });
  await session.start();
  await flush();
  return { session, host, server: app as FakeAppServer };
}

const codexStatus = (name: string, runtimeStatus: string, authStatus = "unsupported") => ({
  name,
  runtimeStatus,
  authStatus,
  toolsError: null,
  pluginId: null,
  httpOrigin: null,
  serverInfo: null,
  serverCapabilities: null,
  tools: {},
  resources: [],
  resourceTemplates: [],
});

describe("a Codex session's MCP servers and login", () => {
  test("reports its thread's servers, read again when one starts or fails", async () => {
    const { host, server: app } = await codexSession((s) => {
      s.mcpServers = [codexStatus("neon", "starting", "oAuth")];
    });
    expect(app.sent("mcpServerStatus/list")[0]).toMatchObject({ threadId: THREAD_ID, detail: "toolsAndAuthOnly" });
    app.mcpServers = [codexStatus("neon", "authenticationRequired", "notLoggedIn")];
    app.push({ method: "mcpServer/startupStatus/updated", params: { threadId: THREAD_ID, name: "neon", status: "failed", error: "auth", failureReason: "reauthenticationRequired" } });
    await flush();
    expect(host.events.filter((e) => e.type === "mcp_status").at(-1)).toEqual({ type: "mcp_status", servers: [server("neon", "needs_auth")] });
  });

  test("connecting a server that needs a login gives Codex's page; its completion reloads the servers", async () => {
    const { session, server: app } = await codexSession((s) => {
      s.mcpServers = [codexStatus("neon", "authenticationRequired", "notLoggedIn")];
    });
    expect(await session.mcpConnect("neon")).toEqual({ auth_url: "https://mcp.example.com/authorize?server=neon" });
    expect(app.sent("mcpServer/oauth/login")[0]).toEqual({ name: "neon", threadId: THREAD_ID });
    app.push({ method: "mcpServer/oauthLogin/completed", params: { name: "neon", threadId: THREAD_ID, success: true } });
    await flush();
    expect(app.sent("config/mcpServer/reload")).toHaveLength(1);
    await session.stop();
  });

  test("a server whose token variable is not set names it and its table in Codex's config", async () => {
    const error = "MCP startup failed: Environment variable SLACK_MCP_TOKEN for MCP server 'slack' is not set";
    const { host } = await codexSession((s) => {
      s.mcpServers = [{ ...codexStatus("slack", "failed", "bearerToken"), toolsError: error }];
    });
    expect(host.events.filter((e) => e.type === "mcp_status").at(-1)).toEqual({
      type: "mcp_status",
      servers: [{ ...server("slack", "failed", error), missing_env: ["SLACK_MCP_TOKEN"], uses_env: ["SLACK_MCP_TOKEN"], declared_in: "~/.codex/config.toml [mcp_servers.slack]" }],
    });
  });

  test("a server whose header or forwarded variable is not set names it, read from its entry in Codex's config", async () => {
    // Codex drops an unset `env_http_headers` or `env_vars` variable without a word: the server
    // fails on whatever it hits.
    const { session, host, server: app } = await codexSession((s) => {
      s.mcpServers = [
        { ...codexStatus("linear", "failed"), toolsError: "MCP startup failed: handshake failed: 401 Unauthorized" },
        { ...codexStatus("db", "failed"), toolsError: "MCP startup failed: connection closed" },
        { ...codexStatus("neon", "connected"), toolsError: null },
      ];
      s.config = {
        config: {
          mcp_servers: {
            linear: { url: "https://mcp.linear.app/mcp", env_http_headers: { Authorization: "LINEAR_TOKEN", "X-Team": "TEAM_ID" } },
            db: { command: "db-mcp", env: { MODE: "ro" }, env_vars: ["DATABASE_URL", { name: "REMOTE_ONLY", source: "remote" }] },
            neon: { url: "https://mcp.neon.tech", bearer_token_env_var: "NEON_TOKEN" },
          },
        },
        origins: {},
        layers: [
          { name: { type: "project", dotCodexFolder: "/w/.codex" }, config: { mcp_servers: { linear: { url: "https://mcp.linear.app/mcp", env_http_headers: { Authorization: "LINEAR_TOKEN", "X-Team": "TEAM_ID" } } } }, version: "1", disabledReason: null },
          { name: { type: "user", file: "/home/j/.codex/config.toml", profile: null }, config: { mcp_servers: { linear: { url: "https://old.example" }, db: { command: "db-mcp" } } }, version: "2", disabledReason: null },
        ],
      };
    }, { HOME: "/home/j", TEAM_ID: "t" });
    expect(app.sent("config/read")).toEqual([{ includeLayers: true, cwd: "/w" }]);
    expect(host.events.filter((e) => e.type === "mcp_status").at(-1)).toEqual({
      type: "mcp_status",
      servers: [
        // The project's table wins over the user's.
        { ...server("linear", "failed", "MCP startup failed: handshake failed: 401 Unauthorized"), missing_env: ["LINEAR_TOKEN"], uses_env: ["LINEAR_TOKEN", "TEAM_ID"], declared_in: "/w/.codex/config.toml [mcp_servers.linear]" },
        // A variable of the remote environment is not the session's to give.
        { ...server("db", "failed", "MCP startup failed: connection closed"), missing_env: ["DATABASE_URL"], uses_env: ["DATABASE_URL"], declared_in: "~/.codex/config.toml [mcp_servers.db]" },
        server("neon", "connected"),
      ],
    });
    // Read once a session; again once Codex reloads its servers' config (Reconnect).
    app.push({ method: "mcpServer/startupStatus/updated", params: { threadId: THREAD_ID, name: "db", status: "failed", error: "closed", failureReason: null } } as never);
    await flush();
    expect(app.sent("config/read")).toHaveLength(1);
    await session.mcpConnect("db");
    expect(app.sent("config/mcpServer/reload")).toHaveLength(1);
    expect(app.sent("config/read")).toHaveLength(2);
    await session.stop();
  });

  test("a server a repo's own .codex/config.toml declares is named in that file", async () => {
    // Codex starts above the repos: their layers are the thread's overrides, which `config/read`
    // for the workspace folder does not list.
    const root = mkdtempSync(join(tmpdir(), "mcp-repo-"));
    const alpha = join(root, "alpha");
    mkdirSync(join(alpha, ".codex"), { recursive: true });
    writeFileSync(join(alpha, ".codex", "config.toml"), "# faked\n");
    try {
      const { server: app, host, session } = await codexSession(
        (s) => {
          s.mcpServers = [{ ...codexStatus("linear", "failed"), toolsError: "MCP startup failed: handshake failed: 401 Unauthorized" }];
          s.configByCwd = {
            [alpha]: {
              config: {},
              origins: {},
              layers: [{ name: { type: "project", dotCodexFolder: join(alpha, ".codex") }, version: "1", disabledReason: null, config: { mcp_servers: { linear: { url: "https://mcp.linear.app/mcp", env_http_headers: { Authorization: "LINEAR_TOKEN" } } } } }],
            },
          };
          s.config = { config: {}, origins: {}, layers: [{ name: { type: "user", file: "/home/j/.codex/config.toml", profile: null }, config: {}, version: "1", disabledReason: null }] };
        },
        { HOME: "/home/j" },
        { additional_directories: [alpha] },
      );
      expect(app.sent("config/read")).toEqual([
        { includeLayers: true, cwd: alpha },
        { includeLayers: true, cwd: "/w" },
      ]);
      expect(host.events.filter((e) => e.type === "mcp_status").at(-1)).toEqual({
        type: "mcp_status",
        servers: [{ ...server("linear", "failed", "MCP startup failed: handshake failed: 401 Unauthorized"), missing_env: ["LINEAR_TOKEN"], uses_env: ["LINEAR_TOKEN"], declared_in: `${alpha}/.codex/config.toml [mcp_servers.linear]` }],
      });
      await session.stop();
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  test("a server whose variables are all set lacks none, and Codex's config is read for broken servers only", async () => {
    const { host, server: app } = await codexSession((s) => {
      s.mcpServers = [{ ...codexStatus("linear", "connected"), toolsError: null }];
      s.config = { config: { mcp_servers: { linear: { url: "https://x", env_http_headers: { Authorization: "LINEAR_TOKEN" } } } }, origins: {}, layers: [] };
    });
    expect(app.sent("config/read")).toHaveLength(0);
    app.mcpServers = [{ ...codexStatus("linear", "failed"), toolsError: "401" }];
    app.push({ method: "mcpServer/startupStatus/updated", params: { threadId: THREAD_ID, name: "linear", status: "failed", error: "401", failureReason: null } } as never);
    await flush();
    expect(app.sent("config/read")).toHaveLength(1);
    expect(host.events.filter((e) => e.type === "mcp_status").at(-1)).toEqual({
      type: "mcp_status",
      servers: [{ ...server("linear", "failed", "401"), missing_env: ["LINEAR_TOKEN"], uses_env: ["LINEAR_TOKEN"], declared_in: "~/.codex/config.toml [mcp_servers.linear]" }],
    });
  });

  test("an unauthorized turn tells the daemon Codex is signed out", async () => {
    const { host, server: app } = await codexSession();
    app.push(codex.turnCompleted("turn-1", "failed", { error: { message: "401 Unauthorized", codexErrorInfo: "unauthorized", additionalDetails: null, misalignment: null } }));
    await flush();
    expect(host.events.filter((e) => e.type === "account_lost")).toEqual([{ type: "account_lost", message: "401 Unauthorized" }]);
  });
});
