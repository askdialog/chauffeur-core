import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { HookGuard } from "../src/agents/codex/hook.ts";
import { designSandboxPolicy, designThreadConfig, LOCAL_CONTEXT_WINDOW, PLAN_NOTE, SANDBOX_NOTE } from "../src/agents/codex/options.ts";
import type { ServerNotification } from "../src/agents/codex/protocol.ts";
import { codexChauffeurTools } from "../src/agents/codex/repo-tools.ts";
import { CodexSession } from "../src/agents/codex/session.ts";
import { ErrorCode, type SessionStartParams } from "../src/protocol.ts";
import { RpcError } from "../src/rpc/peer.ts";
import { codex, FakeAppServer, HOOK, THREAD_ID } from "./support/codex-fakes.ts";
import { FakeHost, flush } from "./support/fakes.ts";

const BASE: SessionStartParams = {
  chat_id: "chat-1",
  workspace_id: "ws-1",
  agent: "codex",
  cwd: "/w",
  additional_directories: [],
  fast: false,
  prompt_attachments: [],
  permission_mode: "default",
  read_only: false,
  prompt_suggestions: false,
  env: { CHAUFFEUR_WORKSPACE_ID: "ws-1" },
};

async function started(params: Partial<SessionStartParams> = {}, options: { hook?: string | undefined; prepare?: (server: FakeAppServer) => void } = {}) {
  const host = new FakeHost();
  let server: FakeAppServer | undefined;
  let ids = 0;
  let guard: HookGuard | undefined;
  const session = new CodexSession({ ...BASE, ...params }, host, {
    spawn: (spawnOptions) => {
      server = new FakeAppServer(spawnOptions);
      options.prepare?.(server);
      return server;
    },
    resolveExecutable: () => "/usr/local/bin/codex",
    baseEnv: { PATH: "/usr/bin", HOME: "/home/me" },
    hookCommand: (asked) => {
      guard = asked;
      return "hook" in options ? options.hook : HOOK;
    },
    initTimeoutMs: 500,
    newRequestId: () => `req-${++ids}`,
  });
  const result = await session.start();
  return { session, host, result, server: server as FakeAppServer, guard };
}

test("start exposes the owned native root PID only when supplied by the spawned app-server", async () => {
  const { result, session } = await started({}, { prepare: (server) => { Object.defineProperty(server, "pid", { value: 12345 }); } });
  expect(result.native_pid).toBe(12345);
  await session.stop();
});

test("Codex stop waits for native exit even when close returns early, and can retry a timeout", async () => {
  const host = new FakeHost();
  let server!: FakeAppServer;
  const session = new CodexSession(BASE, host, {
    spawn: (options) => (server = new FakeAppServer(options)),
    resolveExecutable: () => "/opt/codex", baseEnv: {}, hookCommand: () => undefined,
    initTimeoutMs: 500, stopTimeoutMs: 20,
  });
  await session.start();
  let exited!: () => void;
  Object.defineProperty(server, "exited", { value: new Promise<string>((resolve) => { exited = () => resolve("gone"); }) });
  server.close = async () => {};
  await expect(session.stop()).rejects.toMatchObject({ code: ErrorCode.UNAVAILABLE });
  expect(() => session.send("no replacement")).toThrow();
  exited();
  await expect(session.stop()).resolves.toBeUndefined();
});

describe("starting", () => {
  test("spawns the app-server with the guard hook, trusts it, and opens a thread", async () => {
    const { server, host, result } = await started({ model: "gpt-6-luna", append_system_prompt: "Ticket XYZ-1", additional_directories: ["/extra"] });
    expect(result).toEqual({ agent_session_id: THREAD_ID });
    expect(server.options.executable).toBe("/usr/local/bin/codex");
    expect(server.options.cwd).toBe("/w");
    expect(server.options.env).toMatchObject({ PATH: "/usr/bin", HOME: "/home/me", CHAUFFEUR_WORKSPACE_ID: "ws-1" });
    expect(server.options.args).toEqual(["-c", `hooks.PreToolUse=[{matcher="Bash", hooks=[{type="command", command=${JSON.stringify(HOOK)}}]}]`]);
    expect(server.requests.map((r) => r.method)).toEqual(["initialize", "hooks/list", "thread/start", "skills/list", "mcpServerStatus/list"]);
    expect(server.notified).toEqual(["initialized"]);
    const [init] = server.sent("initialize");
    expect(init.clientInfo.name).toBe("chauffeur");
    // The experimental API, for the thread's dynamic tools.
    expect(init.capabilities).toMatchObject({ experimentalApi: true });
    expect(init.capabilities.optOutNotificationMethods).toContain("account/rateLimits/updated");
    // The session follows its MCP servers.
    expect(init.capabilities.optOutNotificationMethods).not.toContain("mcpServer/startupStatus/updated");
    expect(server.sent("thread/start")[0]).toEqual({
      cwd: "/w",
      approvalPolicy: "on-request",
      approvalsReviewer: "user",
      sandbox: "workspace-write",
      model: "gpt-6-luna",
      developerInstructions: `Ticket XYZ-1\n\n${SANDBOX_NOTE}`,
      // Only Chauffeur's own hook is trusted, not the repo's.
      config: {
        "sandbox_workspace_write.writable_roots": ["/extra"],
        "hooks.state": { "/<session-flags>/config.toml:pre_tool_use:0:0": { trusted_hash: "sha256:abc" } },
      },
      // Chauffeur's repo tools, which the thread keeps when resumed; no `delegate` unless asked.
      dynamicTools: codexChauffeurTools(),
    });
    expect(host.events).toEqual([{ type: "session_started", agent_session_id: THREAD_ID, model: "gpt-6-luna" }]);
  });

  test("a local model opens its thread on Ollama, with the context the daemon gives Ollama", async () => {
    const { server, host } = await started({ model: "ollama/qwen3.8" });
    expect(server.sent("thread/start")[0]).toMatchObject({
      model: "qwen3.8",
      modelProvider: "ollama",
      config: { model_context_window: LOCAL_CONTEXT_WINDOW },
    });
    // The chat keeps the model's `ollama/` id.
    expect(host.events).toEqual([{ type: "session_started", agent_session_id: THREAD_ID, model: "ollama/qwen3.8" }]);
  });

  test("a local model's thread takes no repo layer: its Codex trusts none", async () => {
    const repo = mkdtempSync(join(tmpdir(), "codex-local-repo-"));
    mkdirSync(join(repo, ".codex"));
    writeFileSync(join(repo, ".codex", "config.toml"), "[mcp_servers.repo]\ncommand = \"x\"\n");
    const reads = (server: FakeAppServer) => server.sent("config/read").filter((params) => params.cwd === repo);
    try {
      const local = await started({ model: "ollama/qwen3.8", additional_directories: [repo] });
      expect(reads(local.server)).toEqual([]);
      expect(local.host.events.filter((event) => event.type === "notice")).toEqual([]);
      const hosted = await started({ model: "gpt-6-luna", additional_directories: [repo] });
      expect(reads(hosted.server).length).toBeGreaterThan(0);
    } finally {
      rmSync(repo, { recursive: true, force: true });
    }
  });

  test("a hosted model names no provider and keeps Codex's context window", async () => {
    const { server } = await started({ model: "gpt-6-luna" });
    const start = server.sent("thread/start")[0];
    expect(start.modelProvider).toBeUndefined();
    expect(start.config.model_context_window).toBeUndefined();
  });

  test("maps every permission mode onto an approval policy, a sandbox and a reviewer", async () => {
    const expected = {
      // Auto: what leaves the sandbox goes to Codex's auto review rather than to the user.
      auto: ["on-request", "workspace-write", "auto_review"],
      bypass_permissions: ["never", "danger-full-access", "user"],
      default: ["on-request", "workspace-write", "user"],
      accept_edits: ["on-request", "workspace-write", "user"],
      plan: ["on-request", "read-only", "user"],
    } as const;
    for (const [mode, [approvalPolicy, sandbox, approvalsReviewer]] of Object.entries(expected)) {
      const { server } = await started({ permission_mode: mode as SessionStartParams["permission_mode"] });
      expect(server.sent("thread/start")[0]).toMatchObject({ approvalPolicy, sandbox, approvalsReviewer });
    }
  });

  test("tells a writing thread to ask to leave the sandbox rather than give up", async () => {
    const { server } = await started({ permission_mode: "auto", append_system_prompt: undefined });
    expect(server.sent("thread/start")[0].developerInstructions).toBe(SANDBOX_NOTE);
    const explore = await started({ read_only: true, permission_mode: "plan", append_system_prompt: undefined }, { hook: `${HOOK} read-only` });
    expect(explore.server.sent("thread/start")[0].developerInstructions).toBeUndefined();
  });

  test("an exploration runs read-only without ever asking to leave the sandbox", async () => {
    const { session, server } = await started({ read_only: true, permission_mode: "plan", additional_directories: ["/w/alpha"] }, { hook: `${HOOK} read-only` });
    expect(server.sent("thread/start")[0]).toMatchObject({ approvalPolicy: "never", sandbox: "read-only" });
    expect(server.sent("thread/start")[0].config["sandbox_workspace_write.writable_roots"]).toBeUndefined();
    session.send("How are facets computed?");
    await flush();
    // Its instructions say to explain: no plan note.
    expect(server.sent("turn/start")[0].input[0].text).toBe("How are facets computed?");
  });

  test.skipIf(process.platform !== "darwin")("a workspace chat's sandbox reaches the daemon's socket in every mode, the mode's sandbox set by its first turn", async () => {
    const env = { CHAUFFEUR_WORKSPACE_ID: "ws-1", CHAUFFEUR_HOME: "/h" };
    const proxy = { enabled: true, unix_sockets: { "/h/chauffeurd.sock": "allow" } };
    const firstTurn = {
      auto: { approvalPolicy: "on-request", approvalsReviewer: "auto_review", sandboxPolicy: { type: "workspaceWrite", writableRoots: ["/w", "/w/alpha"], networkAccess: false } },
      default: { approvalPolicy: "on-request", approvalsReviewer: "user", sandboxPolicy: { type: "workspaceWrite", writableRoots: ["/w", "/w/alpha"], networkAccess: false } },
      accept_edits: { approvalPolicy: "on-request", approvalsReviewer: "user", sandboxPolicy: { type: "workspaceWrite", networkAccess: false } },
      plan: { approvalPolicy: "on-request", approvalsReviewer: "user", sandboxPolicy: { type: "readOnly", networkAccess: false } },
      bypass_permissions: { approvalPolicy: "never", approvalsReviewer: "user", sandboxPolicy: { type: "dangerFullAccess" } },
    } as const;
    for (const [mode, expected] of Object.entries(firstTurn)) {
      const { session, server } = await started({ env, permission_mode: mode as SessionStartParams["permission_mode"], additional_directories: ["/w/alpha"] });
      const start = server.sent("thread/start")[0];
      // Codex starts its network proxy, which allows the socket and no domain, only for a thread
      // that begins with network in its sandbox.
      expect(start).toMatchObject({ sandbox: "workspace-write", approvalPolicy: expected.approvalPolicy, approvalsReviewer: expected.approvalsReviewer });
      expect(start.config).toMatchObject({ "features.network_proxy": proxy, "sandbox_workspace_write.network_access": true, "sandbox_workspace_write.writable_roots": ["/w/alpha"] });
      expect(server.sent("config/read")[0]).toMatchObject({ cwd: "/w" });
      session.send("go");
      await flush();
      expect(server.sent("turn/start")[0]).toMatchObject(expected);
      server.push(codex.turnStarted("turn-1"), codex.turnCompleted("turn-1"));
      session.send("again");
      await flush();
      expect(server.sent("turn/start")[1].sandboxPolicy).toBeUndefined();
    }
  });

  test("an exploration's sandbox never reaches the daemon's socket", async () => {
    // The daemon gives an exploration's agent no CHAUFFEUR_HOME; were it there, read-only still says no.
    const envs: Record<string, string>[] = [{ CHAUFFEUR_WORKSPACE_ID: "ws-1", CHAUFFEUR_EXPLORE: "1" }, { CHAUFFEUR_WORKSPACE_ID: "ws-1", CHAUFFEUR_HOME: "/h" }];
    for (const env of envs) {
      const { session, server } = await started({ env, read_only: true, permission_mode: "plan" }, { hook: `${HOOK} read-only` });
      const start = server.sent("thread/start")[0];
      expect(start).toMatchObject({ approvalPolicy: "never", sandbox: "read-only" });
      expect(start.config["features.network_proxy"]).toBeUndefined();
      expect(start.config["sandbox_workspace_write.network_access"]).toBeUndefined();
      session.send("look");
      await flush();
      expect(server.sent("turn/start")[0].sandboxPolicy).toBeUndefined();
    }
  });

  test.skipIf(process.platform !== "darwin")("leaves the sandbox as it is when the user's config already gives it the network", async () => {
    const { session, server } = await started(
      { env: { CHAUFFEUR_WORKSPACE_ID: "ws-1", CHAUFFEUR_HOME: "/h" }, permission_mode: "plan" },
      { prepare: (s) => (s.config = { config: { sandbox_workspace_write: { network_access: true } }, origins: {}, layers: [] }) },
    );
    const start = server.sent("thread/start")[0];
    expect(start.sandbox).toBe("read-only");
    expect(start.config["features.network_proxy"]).toBeUndefined();
    session.send("go");
    await flush();
    expect(server.sent("turn/start")[0].sandboxPolicy).toBeUndefined();
  });

  test("resumes the thread given by resume_session_id", async () => {
    const { server, result, host } = await started({ resume_session_id: "thread-old" });
    expect(server.sent("thread/resume")[0]).toMatchObject({ threadId: "thread-old", excludeTurns: true, cwd: "/w" });
    // The thread kept the tools it started with.
    expect(server.sent("thread/resume")[0].dynamicTools).toBeUndefined();
    expect(server.sent("thread/start")).toEqual([]);
    expect(result).toEqual({ agent_session_id: "thread-old" });
    expect(host.events[0]).toMatchObject({ type: "session_started", agent_session_id: "thread-old" });
  });

  test("warns when the guard hook cannot be trusted", async () => {
    const { server, host } = await started({}, { prepare: (s) => (s.handlers["hooks/list"] = () => ({ data: [] })) });
    expect(server.sent("thread/start")[0].config).toEqual({});
    expect(host.events[0]).toMatchObject({ type: "notice", level: "warning", message: expect.stringContaining("guard is not active") });
  });

  test("runs without the hook when none is configured", async () => {
    const { server } = await started({}, { hook: undefined });
    expect(server.options.args).toEqual([]);
    expect(server.requests.map((r) => r.method)).toEqual(["initialize", "thread/start", "skills/list", "mcpServerStatus/list"]);
  });

  test("fails with 503 when the app-server does not initialize, without events", async () => {
    const host = new FakeHost();
    let server: FakeAppServer | undefined;
    const session = new CodexSession(BASE, host, {
      spawn: (options) => {
        server = new FakeAppServer(options);
        server.handlers.initialize = () => Promise.reject(new Error("not logged in"));
        return server;
      },
      resolveExecutable: () => "codex",
      baseEnv: {},
      hookCommand: () => undefined,
    });
    await expect(session.start()).rejects.toMatchObject({ code: ErrorCode.UNAVAILABLE, message: "Codex failed to start: not logged in" });
    expect(server?.closed).toBe(true);
    expect(host.events).toEqual([]);
  });

  test("a design chat writes its folder only and never asks to leave its sandbox, whatever its mode", async () => {
    const dir = "/w/.chauffeur/designs/c1";
    for (const params of [{ permission_mode: "auto" }, { permission_mode: "plan", read_only: true }] as const) {
      const { session, server, guard } = await started({ ...params, cwd: dir, design_dir: dir, additional_directories: ["/w/alpha"], append_system_prompt: "Design" });
      expect(guard).toBe("design");
      const start = server.sent("thread/start")[0];
      // The repos are no writable root, the thread is not told to ask, and it has no Chauffeur tool.
      expect(start).toMatchObject({ cwd: dir, approvalPolicy: "never", sandbox: "workspace-write", developerInstructions: "Design", dynamicTools: [] });
      expect(start.config).toMatchObject(designThreadConfig(dir));
      expect(start.config["mcp_servers.headless-browser"]).toMatchObject({ command: "npx", default_tools_approval_mode: "approve" });
      session.send("A sign-in screen");
      await flush();
      expect(server.sent("turn/start")[0]).toMatchObject({ approvalPolicy: "never", sandboxPolicy: designSandboxPolicy(dir) });
      expect(server.sent("turn/start")[0].input[0].text).toBe("A sign-in screen");
    }
  });

  test("a design chat's sandbox leaves out /tmp and $TMPDIR, the repos and the network", () => {
    expect(designSandboxPolicy("/w/.chauffeur/designs/c1")).toEqual({
      type: "workspaceWrite",
      writableRoots: ["/w/.chauffeur/designs/c1"],
      networkAccess: false,
      excludeTmpdirEnvVar: true,
      excludeSlashTmp: true,
    });
  });

  test("a design chat's sandbox never reaches the daemon's socket, even with CHAUFFEUR_HOME", async () => {
    const dir = "/w/.chauffeur/designs/c1";
    const { session, server } = await started({ cwd: dir, design_dir: dir, permission_mode: "auto", env: { CHAUFFEUR_WORKSPACE_ID: "ws-1", CHAUFFEUR_HOME: "/h" } });
    const start = server.sent("thread/start")[0];
    expect(start.config["features.network_proxy"]).toBeUndefined();
    expect(start.config["sandbox_workspace_write.network_access"]).toBe(false);
    session.send("go");
    await flush();
    expect(server.sent("turn/start")[0].sandboxPolicy).toEqual(designSandboxPolicy(dir));
  });

  test("a mode change in a design chat keeps its sandbox", async () => {
    const dir = "/w/.chauffeur/designs/c1";
    const { session, server } = await started({ cwd: dir, design_dir: dir, permission_mode: "auto" });
    session.send("go");
    await flush();
    server.push(codex.turnStarted("turn-1"), codex.turnCompleted("turn-1"));
    await session.configure({ permissionMode: "bypass_permissions" });
    session.send("again");
    await flush();
    expect(server.sent("turn/start")[1]).toMatchObject({ approvalPolicy: "never", sandboxPolicy: designSandboxPolicy(dir) });
  });

  test("fails with 503 when codex is missing", async () => {
    const session = new CodexSession(BASE, new FakeHost(), {
      spawn: () => {
        throw new Error("not reached");
      },
      resolveExecutable: () => {
        throw new RpcError(ErrorCode.UNAVAILABLE, "codex is not installed");
      },
      baseEnv: {},
      hookCommand: () => undefined,
    });
    await expect(session.start()).rejects.toMatchObject({ code: ErrorCode.UNAVAILABLE, message: "codex is not installed" });
  });
});

describe("each repo's own Codex config", () => {
  /** A workspace with repos alpha (trusted by Codex) and beta (not), each with a `.codex/config.toml`. */
  function repos() {
    const root = mkdtempSync(join(tmpdir(), "codex-session-repos-"));
    const [alpha, beta] = ["alpha", "beta"].map((name) => {
      const dir = join(root, name);
      mkdirSync(join(dir, ".codex"), { recursive: true });
      writeFileSync(join(dir, ".codex", "config.toml"), "# faked\n");
      return dir;
    }) as [string, string];
    const reason = `To load project-local config, hooks, and exec policies, add ${beta} as a trusted project in /home/me/.codex/config.toml.`;
    const prepare = (server: FakeAppServer) => {
      server.configByCwd = {
        [alpha]: { config: {}, origins: {}, layers: [{ name: { type: "project", dotCodexFolder: join(alpha, ".codex") }, version: "1", disabledReason: null, config: { mcp_servers: { notes: { command: "notes-mcp" } }, hooks: { PreToolUse: [] } } }] },
        [beta]: { config: {}, origins: {}, layers: [{ name: { type: "project", dotCodexFolder: join(beta, ".codex") }, version: "1", disabledReason: reason, config: { mcp_servers: { evil: { command: "evil-mcp" } } } }] },
      };
    };
    return { root, alpha, beta, reason, prepare };
  }

  test("a trusted repo's servers and settings are the thread's config; an untrusted repo's are not, and the chat says why", async () => {
    const { root, alpha, beta, reason, prepare } = repos();
    try {
      const { server, host, session } = await started({ additional_directories: [alpha, beta] }, { prepare });
      expect(server.sent("config/read")).toEqual([
        { includeLayers: true, cwd: alpha },
        { includeLayers: true, cwd: beta },
      ]);
      expect(server.sent("thread/start")[0].config).toEqual({
        "sandbox_workspace_write.writable_roots": [alpha, beta],
        // Chauffeur's own hook trust stays; the repo's hooks are not handed over.
        "hooks.state": { "/<session-flags>/config.toml:pre_tool_use:0:0": { trusted_hash: "sha256:abc" } },
        mcp_servers: { notes: { command: "notes-mcp" } },
      });
      expect(host.events.filter((event) => event.type === "notice")).toEqual([
        { type: "notice", level: "warning", message: `beta's .codex/config.toml is not loaded: Codex does not trust this repo. ${reason}` },
      ]);
      await session.stop();
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  test("a design chat's sandbox wins over a trusted repo's that opens the network or adds writable roots, and keeps its servers", async () => {
    const { root, alpha } = repos();
    const dir = "/w/.chauffeur/designs/c1";
    const layer = {
      name: { type: "project", dotCodexFolder: join(alpha, ".codex") },
      version: "1",
      disabledReason: null,
      config: { mcp_servers: { notes: { command: "notes-mcp" } }, sandbox_workspace_write: { network_access: true, writable_roots: [alpha] } },
    };
    try {
      const { server, session } = await started(
        { cwd: dir, design_dir: dir, permission_mode: "auto", additional_directories: [alpha] },
        { prepare: (s) => (s.configByCwd = { [alpha]: { config: {}, origins: {}, layers: [layer] } }) },
      );
      const config = server.sent("thread/start")[0].config;
      expect(config).toMatchObject({ "sandbox_workspace_write.network_access": false, "sandbox_workspace_write.writable_roots": [], "mcp_servers.notes": { command: "notes-mcp" } });
      expect(config["mcp_servers.headless-browser"]).toBeDefined();
      await session.stop();
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  test("a resumed thread gets the repos' config again, without repeating the notice", async () => {
    const { root, alpha, beta, prepare } = repos();
    try {
      const { server, host, session } = await started({ additional_directories: [alpha, beta], resume_session_id: "thread-old" }, { prepare });
      expect(server.sent("thread/resume")[0].config.mcp_servers).toEqual({ notes: { command: "notes-mcp" } });
      expect(host.events.filter((event) => event.type === "notice")).toEqual([]);
      await session.stop();
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

describe("turns and messages", () => {
  test("the first prompt starts a turn with the effort and reasoning summaries", async () => {
    const { server } = await started({ prompt: "Fix the bug", effort: "high" });
    await flush();
    expect(server.sent("turn/start")).toEqual([
      { threadId: THREAD_ID, input: [{ type: "text", text: "Fix the bug", text_elements: [] }], summary: "auto", effort: "high" },
    ]);
  });

  test("the chat is working as soon as a message is handed over, before Codex confirms the turn", async () => {
    const { session, server, host } = await started({}, { prepare: (s) => (s.handlers["turn/start"] = () => new Promise(() => {})) });
    session.send("hi");
    // Synchronously: the daemon reads the status right after `session.send` answers.
    expect(host.events.at(-1)).toEqual({ type: "turn_started" });
    server.push(codex.turnStarted("turn-1"));
    expect(host.events.filter((e) => e.type === "turn_started")).toHaveLength(1);
  });

  test("a prompt's turn is announced by the time start answers", async () => {
    const { host } = await started({ prompt: "Fix the bug" });
    expect(host.types()).toEqual(["session_started", "turn_started"]);
  });

  test("a turn Codex refuses to start ends with its error", async () => {
    const { session, host } = await started({}, { prepare: (s) => (s.handlers["turn/start"] = () => Promise.reject(new Error("model not supported"))) });
    session.send("hi");
    await flush();
    expect(host.events.slice(1)).toEqual([
      { type: "turn_started" },
      { type: "turn_ended", outcome: "error", duration_ms: null, cost_usd: null, context_tokens: null, error: "Codex could not start the turn: model not supported" },
    ]);
  });

  test("a message that waited for the end of a turn starts the next one at once", async () => {
    const { session, server, host } = await started({}, { prepare: (s) => (s.handlers["turn/steer"] = () => Promise.reject(new Error("no active turn"))) });
    session.send("first");
    await flush();
    server.push(codex.turnStarted("turn-1"));
    session.send("late");
    await flush();
    server.push(codex.turnCompleted("turn-1"));
    expect(host.types().slice(-2)).toEqual(["turn_ended", "turn_started"]);
  });

  test("turn notifications become turn_started and turn_ended", async () => {
    const { session, server, host } = await started();
    session.send("hi");
    await flush();
    server.push(codex.turnStarted("turn-1"), codex.agentMessage("msg-1", "hello"), codex.turnCompleted("turn-1"));
    expect(host.events.slice(1)).toEqual([
      { type: "turn_started" },
      { type: "assistant_text", message_id: "msg-1", text: "hello", parent_tool_use_id: null },
      {
        type: "turn_ended",
        outcome: "success",
        duration_ms: 1500,
        cost_usd: null,
        context_tokens: null,
        error: null,
        fork_point: { session_id: THREAD_ID, up_to: "turn-1" },
      },
    ]);
  });

  test("a subagent's thread neither ends the chat's turn nor speaks in it", async () => {
    const { session, server, host } = await started();
    session.send("hi");
    await flush();
    // Codex reports a spawned subagent's thread on the same connection.
    const sub = <T extends { params: object }>(message: T): T => ({ ...message, params: { ...message.params, threadId: "thread-sub" } });
    const delta = (text: string): ServerNotification => ({
      method: "item/agentMessage/delta",
      params: { threadId: THREAD_ID, turnId: "turn-1", itemId: "msg-1", delta: text },
    });
    server.push(
      codex.turnStarted("turn-1"),
      delta("Q13 — un exempl"),
      sub(codex.turnStarted("sub-turn")),
      sub(codex.agentMessage("sub-msg", "No findings", "sub-turn")),
      sub(codex.turnCompleted("sub-turn")),
      delta("aire ?"),
    );
    expect(host.types().slice(1)).toEqual(["turn_started", "text_delta", "text_delta"]);
    // Its requests still come to the chat, and close when Codex withdraws them.
    const asked = server.ask(sub(codex.commandApproval(5, "ls")));
    await flush();
    server.push(sub(codex.serverRequestResolved(5)));
    await asked;
    expect(host.types().slice(-2)).toEqual(["permission_request", "request_resolved"]);
    server.push(codex.agentMessage("msg-1", "Q13 — un exemplaire ?"), codex.turnCompleted("turn-1"));
    expect(host.types().slice(-2)).toEqual(["assistant_text", "turn_ended"]);
    expect(host.types().filter((type) => type === "turn_ended")).toHaveLength(1);
  });

  test("a message sent mid-turn is steered into the running turn", async () => {
    const { session, server } = await started();
    session.send("first");
    await flush();
    server.push(codex.turnStarted("turn-1"));
    session.send("also this");
    await flush();
    expect(server.sent("turn/steer")).toEqual([
      { threadId: THREAD_ID, input: [{ type: "text", text: "also this", text_elements: [] }], expectedTurnId: "turn-1" },
    ]);
    expect(server.sent("turn/start")).toHaveLength(1);
  });

  test("a message sent while the turn starts waits for its id, then is steered", async () => {
    let release!: () => void;
    const { session, server } = await started({}, {
      prepare: (s) => {
        s.handlers["turn/start"] = () => new Promise((resolve) => (release = () => resolve({ turn: { id: "turn-9", status: "inProgress" } })));
      },
    });
    session.send("first");
    session.send("second");
    await flush();
    expect(server.sent("turn/steer")).toEqual([]);
    release();
    await flush();
    expect(server.sent("turn/steer")).toEqual([expect.objectContaining({ expectedTurnId: "turn-9", input: [expect.objectContaining({ text: "second" })] })]);
  });

  test("a steer that loses the race with the end of the turn runs as the next turn", async () => {
    const { session, server } = await started({}, { prepare: (s) => (s.handlers["turn/steer"] = () => Promise.reject(new Error("no active turn"))) });
    session.send("first");
    await flush();
    server.push(codex.turnStarted("turn-1"));
    session.send("late");
    server.push(codex.turnCompleted("turn-1"));
    await flush();
    expect(server.sent("turn/start").map((p) => p.input[0].text)).toEqual(["first", "late"]);
  });

  test("plan mode adds its note to every message", async () => {
    const { session, server } = await started({ permission_mode: "plan" });
    session.send("Refactor the parser");
    await flush();
    expect(server.sent("turn/start")[0].input[0].text).toBe(`${PLAN_NOTE}\n\nRefactor the parser`);
  });

  test("a crashed app-server ends the turn with an error and frees the chat", async () => {
    const { session, server, host } = await started();
    session.send("hi");
    await flush();
    server.push(codex.turnStarted("turn-1"));
    server.crash("exited with code 101");
    await flush();
    expect(host.events.slice(-2)).toEqual([
      { type: "notice", level: "error", message: "Codex session ended: codex app-server exited with code 101" },
      { type: "turn_ended", outcome: "error", duration_ms: null, cost_usd: null, context_tokens: null, error: "codex app-server exited with code 101" },
    ]);
    expect(host.endedCount).toBe(1);
    expect(() => session.send("again")).toThrow(RpcError);
  });
});

describe("tasks", () => {
  test("a task cannot be stopped: Codex reports none", async () => {
    const { session } = await started();
    const error = (await session.stopTask().catch((e: unknown) => e)) as RpcError;
    expect(error).toBeInstanceOf(RpcError);
    expect(error.code).toBe(ErrorCode.BAD_REQUEST);
    expect(error.message).toContain("Codex has no task to stop");
  });
});

describe("skills", () => {
  const skill = (name: string, scope = "user") => ({
    name,
    description: `${name} description`,
    path: `/home/.codex/skills/${name}/SKILL.md`,
    scope,
    enabled: true,
    pluginId: null,
  });

  test("a message starting with /name runs the skill of that name", async () => {
    const { session, server } = await started({ additional_directories: ["/w/alpha"] }, { prepare: (s) => (s.skills = [skill("review-pr")]) });
    expect(server.sent("skills/list")[0]).toEqual({ cwds: ["/w", "/w/alpha"] });
    session.send("/review-pr 1234");
    await flush();
    expect(server.sent("turn/start")[0].input.slice(0, 2)).toEqual([
      { type: "skill", name: "review-pr", path: "/home/.codex/skills/review-pr/SKILL.md" },
      { type: "text", text: "/review-pr 1234", text_elements: [] },
    ]);
  });

  test("each repo's skill folders are handed to the thread before it opens", async () => {
    const repo = mkdtempSync(join(tmpdir(), "chauffeur-repo-"));
    mkdirSync(join(repo, ".agents", "skills"), { recursive: true });
    const { server } = await started({ additional_directories: [repo, "/nowhere"] });
    expect(server.sent("skills/extraRoots/set")).toEqual([{ extraRoots: [join(repo, ".agents", "skills")] }]);
    const methods = server.requests.map((r) => r.method);
    expect(methods.indexOf("skills/extraRoots/set")).toBeLessThan(methods.indexOf("thread/start"));
    rmSync(repo, { recursive: true });
  });

  test("an unknown /name stays plain text", async () => {
    const { session, server } = await started({}, { prepare: (s) => (s.skills = [skill("review-pr")]) });
    session.send("/nope and a/path");
    await flush();
    expect(server.sent("turn/start")[0].input).toEqual([{ type: "text", text: "/nope and a/path", text_elements: [] }]);
  });
});

describe("questions", () => {
  test("request_user_input becomes a question, answered back by question id", async () => {
    const { session, server, host } = await started();
    const answer = server.ask(
      codex.userInput(0, [
        { id: "colour", header: "Colour", question: "Which colour?", options: [{ label: "Red", description: "warm" }] },
        { id: "why", header: "Why", question: "Why?", options: null },
      ]),
    );
    await flush();
    expect(host.events.at(-1)).toEqual({
      type: "question",
      request_id: "req-1",
      questions: [
        { question: "Which colour?", header: "Colour", multi_select: false, options: [{ label: "Red", description: "warm" }] },
        { question: "Why?", header: "Why", multi_select: false, options: [] },
      ],
    });
    session.respond("req-1", { type: "answer", answers: { "Which colour?": "Red", "Why?": "It is warm" } });
    expect(await answer).toEqual({ answers: { colour: { answers: ["Red"] }, why: { answers: ["It is warm"] } } });
  });

  test("a declined question sends no answer and tells the turn why", async () => {
    const { session, server } = await started();
    session.send("go");
    await flush();
    server.push(codex.turnStarted("turn-1"));
    const answer = server.ask(codex.userInput(0, [{ id: "q", header: "", question: "Which?", options: null }]));
    await flush();
    session.respond("req-1", { type: "deny", message: "I will reply in the chat." });
    expect(await answer).toEqual({ answers: {} });
    await flush();
    expect(server.sent("turn/steer").at(-1)?.input[0].text).toBe("I will reply in the chat.");
  });
});

describe("approvals", () => {
  test("a command approval round trip: allow always accepts for the session", async () => {
    const { session, server, host } = await started();
    const answer = server.ask(codex.commandApproval(0, "/bin/zsh -lc 'touch x.txt'", "needs to write"));
    await flush();
    expect(host.events.at(-1)).toEqual({
      type: "permission_request",
      request_id: "req-1",
      tool_name: "Bash",
      input: { command: "touch x.txt", cwd: "/w", reason: "needs to write" },
      title: "needs to write",
    });
    session.respond("req-1", { type: "allow", always: true });
    expect(await answer).toEqual({ decision: "acceptForSession" });
    // What Codex no longer asks for in the session: that command, in that folder.
    expect(host.events.at(-1)).toEqual({ type: "request_resolved", request_id: "req-1", outcome: "allowed", rules: ["Bash(touch x.txt) in /w"] });
  });

  test("always allow on a command allows the open requests Codex's approval covers, and only those", async () => {
    const { session, server, host } = await started();
    const first = server.ask(codex.commandApproval(0, "/bin/zsh -lc 'git commit -m x'", "commit"));
    const same = server.ask(codex.commandApproval(1, "/bin/zsh -lc 'git commit -m x'", "commit again"));
    const elsewhere = server.ask(codex.commandApproval(2, "/bin/zsh -lc 'git commit -m x'", null, "turn-1", { cwd: "/w/alpha" }));
    const other = server.ask(codex.commandApproval(3, "/bin/zsh -lc 'git push'"));
    await flush();
    session.respond("req-1", { type: "allow", always: true });
    expect(await first).toEqual({ decision: "acceptForSession" });
    expect(await same).toEqual({ decision: "acceptForSession" });
    expect(host.events.filter((e) => e.type === "request_resolved")).toEqual([
      { type: "request_resolved", request_id: "req-1", outcome: "allowed", rules: ["Bash(git commit -m x) in /w"] },
      { type: "request_resolved", request_id: "req-2", outcome: "allowed", rules: ["Bash(git commit -m x) in /w"] },
    ]);
    // Another folder or another command: Codex would ask, so does the chat.
    session.respond("req-3", { type: "deny", message: null });
    session.respond("req-4", { type: "allow", always: false });
    expect(await elsewhere).toEqual({ decision: "decline" });
    expect(await other).toEqual({ decision: "accept" });
  });

  test("Codex asking again for a command allowed always is answered without asking", async () => {
    const { session, server, host } = await started();
    const first = server.ask(codex.commandApproval(0, "/bin/zsh -lc 'make'"));
    await flush();
    session.respond("req-1", { type: "allow", always: true });
    await first;
    // A variant the request does not show (another terminal mode), run through another shell.
    expect(await server.ask(codex.commandApproval(1, "/bin/bash -lc make"))).toEqual({ decision: "acceptForSession" });
    expect(host.events.filter((e) => e.type === "permission_request")).toHaveLength(1);
  });

  test("a new session of the chat keeps answering what was allowed always before", async () => {
    const { server, host } = await started({ always_allow: ["Bash(make) in /w", "apply_patch(/w/a.txt)"] });
    expect(await server.ask(codex.commandApproval(0, "/bin/zsh -lc make"))).toEqual({ decision: "acceptForSession" });
    server.push(codex.fileChangeStarted("patch-1"));
    expect(await server.ask(codex.fileApproval(1, "patch-1"))).toEqual({ decision: "acceptForSession" });
    expect(host.events.filter((e) => e.type === "permission_request")).toEqual([]);
  });

  test("always allow on a file change covers the open changes to those files only", async () => {
    const { session, server, host } = await started();
    const change = (path: string, move_path: string | null = null) => ({ path, kind: { type: "update" as const, move_path }, diff: "-a\n+b\n" });
    server.push(codex.fileChangeStarted("p1", "turn-1", [change("/x/a.ts"), change("/x/b.ts", "/x/c.ts")]));
    server.push(codex.fileChangeStarted("p2", "turn-1", [change("/x/c.ts")]));
    server.push(codex.fileChangeStarted("p3", "turn-1", [change("/x/a.ts"), change("/x/d.ts")]));
    server.push(codex.fileChangeStarted("p4", "turn-1", [change("/x/a.ts")]));
    const first = server.ask(codex.fileApproval(0, "p1"));
    const moved = server.ask(codex.fileApproval(1, "p2"));
    const another = server.ask(codex.fileApproval(2, "p3"));
    // Asked again after the sandbox refused it: Codex never remembers it.
    const retried = server.ask(codex.fileApproval(3, "p4", "the sandbox refused the write"));
    await flush();
    session.respond("req-1", { type: "allow", always: true });
    expect(await first).toEqual({ decision: "acceptForSession" });
    expect(await moved).toEqual({ decision: "acceptForSession" });
    expect(host.events.filter((e) => e.type === "request_resolved")).toEqual([
      { type: "request_resolved", request_id: "req-1", outcome: "allowed", rules: ["apply_patch(/x/a.ts)", "apply_patch(/x/b.ts)", "apply_patch(/x/c.ts)"] },
      { type: "request_resolved", request_id: "req-2", outcome: "allowed", rules: ["apply_patch(/x/c.ts)"] },
    ]);
    session.respond("req-3", { type: "allow", always: false });
    session.respond("req-4", { type: "allow", always: true });
    expect(await another).toEqual({ decision: "accept" });
    expect(await retried).toEqual({ decision: "acceptForSession" });
    // Allowed always, but Codex will ask again: the card says nothing is remembered.
    expect(host.events.at(-1)).toEqual({ type: "request_resolved", request_id: "req-4", outcome: "allowed" });
  });

  test("what Codex approves once whatever the answer gets no rule and covers nothing", async () => {
    const { session, server, host } = await started();
    const asks = [
      codex.commandApproval(0, "write_stdin --session-id 1 y", null, "turn-1", { kind: "writeStdin" }),
      codex.commandApproval(1, "/bin/zsh -lc 'rm a'", null, "turn-1", { approvalId: "sub-1" }),
      codex.commandApproval(2, "", "network", "turn-1", { command: null, cwd: null, networkApprovalContext: { host: "example.com", protocol: "https" } }),
    ].map((request) => server.ask(request));
    await flush();
    for (const id of ["req-1", "req-2", "req-3"]) session.respond(id, { type: "allow", always: true });
    await Promise.all(asks);
    expect(host.events.filter((e) => e.type === "request_resolved")).toEqual(
      ["req-1", "req-2", "req-3"].map((id) => ({ type: "request_resolved", request_id: id, outcome: "allowed" })),
    );
    // Nothing remembered: the same subcommand is asked again.
    void server.ask(codex.commandApproval(4, "/bin/zsh -lc 'rm a'", null, "turn-1", { approvalId: "sub-2" }));
    await flush();
    expect(host.events.at(-1)).toMatchObject({ type: "permission_request", request_id: "req-4" });
  });

  test("allow once accepts, deny declines and passes the user's words on", async () => {
    const { session, server } = await started();
    session.send("go");
    await flush();
    server.push(codex.turnStarted("turn-1"));
    const once = server.ask(codex.commandApproval(0, "make"));
    const no = server.ask(codex.commandApproval(1, "rm -rf build"));
    await flush();
    session.respond("req-1", { type: "allow", always: false });
    session.respond("req-2", { type: "deny", message: "Keep the build folder" });
    expect(await once).toEqual({ decision: "accept" });
    expect(await no).toEqual({ decision: "decline" });
    await flush();
    expect(server.sent("turn/steer")).toEqual([expect.objectContaining({ input: [expect.objectContaining({ text: "I declined that: Keep the build folder" })] })]);
  });

  test("a file change approval shows the change it is about", async () => {
    const { session, server, host } = await started();
    server.push(codex.fileChangeStarted("patch-1"));
    const answer = server.ask(codex.fileApproval(4, "patch-1", "outside the workspace"));
    await flush();
    expect(host.events.at(-1)).toEqual({
      type: "permission_request",
      request_id: "req-1",
      tool_name: "apply_patch",
      input: { changes: [{ path: "/w/a.txt", kind: "update", diff: "-a\n+b\n" }], reason: "outside the workspace" },
      title: "outside the workspace",
    });
    session.respond("req-1", { type: "allow", always: false });
    expect(await answer).toEqual({ decision: "accept" });
  });

  test("a request Codex withdraws is resolved as cancelled", async () => {
    const { server, host } = await started();
    const answer = server.ask(codex.commandApproval(7, "make"));
    await flush();
    server.push(codex.serverRequestResolved(7));
    expect(await answer).toEqual({ decision: "cancel" });
    expect(host.events.at(-1)).toEqual({ type: "request_resolved", request_id: "req-1", outcome: "cancelled" });
  });

  test("interrupt cancels pending requests and interrupts the running turn", async () => {
    const { session, server, host } = await started();
    session.send("go");
    await flush();
    server.push(codex.turnStarted("turn-1"));
    const answer = server.ask(codex.commandApproval(0, "make"));
    await flush();
    await session.interrupt();
    expect(await answer).toEqual({ decision: "cancel" });
    expect(server.sent("turn/interrupt")).toEqual([{ threadId: THREAD_ID, turnId: "turn-1" }]);
    server.push(codex.turnCompleted("turn-1", "interrupted"));
    expect(host.events.at(-1)).toMatchObject({ type: "turn_ended", outcome: "interrupted" });
  });

  test("requests left open when a turn ends resolve before turn_ended", async () => {
    const { session, server, host } = await started();
    session.send("go");
    await flush();
    server.push(codex.turnStarted("turn-1"));
    void server.ask(codex.commandApproval(0, "make"));
    await flush();
    server.push(codex.turnCompleted("turn-1"));
    expect(host.events.slice(-2).map((e) => e.type)).toEqual(["request_resolved", "turn_ended"]);
  });

  test("answers a permissions request and refuses unknown requests", async () => {
    const { session, server } = await started();
    const permissions = server.ask({
      method: "item/permissions/requestApproval",
      id: 3,
      params: { threadId: THREAD_ID, turnId: "turn-1", itemId: "i", environmentId: null, startedAtMs: 1, cwd: "/w", reason: "network", permissions: { network: { enabled: true }, fileSystem: null } },
    } as never);
    await flush();
    session.respond("req-1", { type: "allow", always: true });
    expect(await permissions).toEqual({ permissions: { network: { enabled: true } }, scope: "session" });
    await expect(server.ask({ method: "item/unknown/request", id: 6, params: {} } as never)).rejects.toMatchObject({ code: ErrorCode.METHOD_NOT_FOUND });
  });
});

describe("MCP confirmations", () => {
  const request = (mode: "form" | "openai/form" | "openaiForm" = "form") => ({
    method: "mcpServer/elicitation/request" as const, id: 81,
    params: { threadId: THREAD_ID, turnId: "turn-1", serverName: "computer-use", mode,
      _meta: null, message: "Allow Google Chrome?", requestedSchema: { type: "object" as const, properties: {} } },
  });

  test.each(["form", "openai/form", "openaiForm"] as const)("%s confirmations wait for the user's explicit response", async (mode) => {
    const { session, server, host } = await started({ permission_mode: "bypass_permissions" });
    const response = server.ask(request(mode));
    await flush();
    expect(host.events).toContainEqual(expect.objectContaining({ type: "mcp_elicitation", server_name: "computer-use", message: "Allow Google Chrome?" }));
    session.respond("req-1", { type: "elicitation", action: "accept", content: {}, always: false });
    expect(await response).toEqual({ action: "accept", content: {}, _meta: null });
    const again = server.ask(request(mode));
    await flush();
    session.respond("req-2", { type: "elicitation", action: "decline", content: null, always: false });
    expect(await again).toEqual({ action: "decline", content: null, _meta: null });
  });

  test("server withdrawal and turn completion cancel confirmations and expire their request ids", async () => {
    const { session, server } = await started();
    const withdrawn = server.ask(request());
    await flush();
    server.push({ method: "serverRequest/resolved", params: { threadId: THREAD_ID, requestId: 81 } });
    expect(await withdrawn).toEqual({ action: "cancel", content: null, _meta: null });
    expect(() => session.respond("req-1", { type: "elicitation", action: "accept", content: {}, always: false })).toThrow();
    const completed = server.ask(request());
    await flush();
    server.push(codex.turnCompleted("turn-1"));
    expect(await completed).toEqual({ action: "cancel", content: null, _meta: null });
  });
});

describe("configure and stop", () => {
  test("another local model rides the next turn by its name on Ollama", async () => {
    const { session, server } = await started({ model: "ollama/qwen3.8" });
    await session.configure({ model: "ollama/glm-4.7-flash" });
    session.send("go");
    await flush();
    expect(server.sent("turn/start")[0]).toMatchObject({ model: "glm-4.7-flash" });
  });

  test("model, effort and permission changes ride the next turn/start", async () => {
    const { session, server, host } = await started();
    await session.configure({ model: "gpt-6-sol", effort: "xhigh", permissionMode: "bypass_permissions" });
    session.send("go");
    await flush();
    expect(server.sent("turn/start")[0]).toMatchObject({
      model: "gpt-6-sol",
      effort: "xhigh",
      approvalPolicy: "never",
      approvalsReviewer: "user",
      sandboxPolicy: { type: "dangerFullAccess" },
    });
    // Sent once: Codex keeps them for the following turns.
    server.push(codex.turnStarted("turn-1"), codex.turnCompleted("turn-1"));
    session.send("again");
    await flush();
    const second = server.sent("turn/start")[1];
    expect(second.model).toBeUndefined();
    expect(second.approvalPolicy).toBeUndefined();
    expect(second.effort).toBe("xhigh");
    expect(host.events.filter((e) => e.type === "notice")).toEqual([]);
  });

  test("switching to auto hands what leaves the sandbox to auto review", async () => {
    const { session, server } = await started({ permission_mode: "bypass_permissions" });
    await session.configure({ permissionMode: "auto" });
    session.send("go");
    await flush();
    expect(server.sent("turn/start")[0]).toMatchObject({
      approvalPolicy: "on-request",
      approvalsReviewer: "auto_review",
      sandboxPolicy: { type: "workspaceWrite", networkAccess: false },
    });
  });

  test("fast mode is the priority service tier, cleared with null", async () => {
    const { session, server } = await started({ fast: true });
    expect(server.sent("thread/start")[0].serviceTier).toBe("priority");
    await session.configure({ fast: false });
    session.send("go");
    await flush();
    expect(server.sent("turn/start")[0].serviceTier).toBeNull();
    server.push(codex.turnStarted("turn-1"), codex.turnCompleted("turn-1"));
    session.send("again");
    await flush();
    expect("serviceTier" in server.sent("turn/start")[1]).toBe(false);
  });

  test("a change during a turn says it applies from the next turn", async () => {
    const { session, server, host } = await started();
    session.send("go");
    await flush();
    server.push(codex.turnStarted("turn-1"));
    await session.configure({ permissionMode: "default", model: "gpt-6-sol" });
    expect(host.events.at(-1)).toEqual({ type: "notice", level: "info", message: "Codex applies model, effort and permission changes from the next turn" });
    await session.configure({ permissionMode: "accept_edits" });
    server.push(codex.turnCompleted("turn-1"));
    session.send("next");
    await flush();
    expect(server.sent("turn/start")[1]).toMatchObject({
      approvalPolicy: "on-request",
      sandboxPolicy: { type: "workspaceWrite", writableRoots: ["/w"], networkAccess: false },
    });
  });

  test("stop cancels requests, ends the running turn and closes the app-server", async () => {
    const { session, server, host } = await started();
    session.send("go");
    await flush();
    server.push(codex.turnStarted("turn-1"));
    const answer = server.ask(codex.commandApproval(0, "make"));
    await flush();
    await session.stop();
    expect(await answer).toEqual({ decision: "cancel" });
    expect(server.closed).toBe(true);
    expect(host.events.slice(-2).map((e) => e.type)).toEqual(["request_resolved", "turn_ended"]);
    expect(host.events.at(-1)).toMatchObject({ outcome: "interrupted" });
    expect(host.endedCount).toBe(0);
    expect(() => session.send("more")).toThrow(RpcError);
    await session.stop();
  });
});

describe("repo tools", () => {
  const toolCall = (id: number, tool: string, args: unknown) =>
    ({
      method: "item/tool/call",
      id,
      params: { threadId: THREAD_ID, turnId: "turn-1", callId: `call-${id}`, namespace: "chauffeur", tool, arguments: args },
    }) as never;

  test("list_repos lists the organization's repos, those in the workspace with their folder", async () => {
    const { server, host } = await started();
    const answer = (await server.ask(toolCall(1, "list_repos", {}))) as { contentItems: Array<{ text: string }>; success: boolean };
    expect(answer.success).toBe(true);
    expect(answer.contentItems[0]?.text).toContain("- web: The monolith (folder /workspaces/w/web)");
    expect(answer.contentItems[0]?.text).toContain("- analytics: Dashboards");
    expect(host.events.filter((e) => e.type === "permission_request")).toEqual([]);
  });

  test("add_repo puts the repo to the user, and adds it once they said yes", async () => {
    const { session, server, host } = await started();
    const call = server.ask(toolCall(2, "add_repo", { repo: "analytics", reason: "Les dashboards lisent ces events" }));
    await flush();
    expect(host.events.at(-1)).toEqual({
      type: "permission_request",
      request_id: "req-1",
      tool_name: "mcp__chauffeur__add_repo",
      input: { repo: "analytics", reason: "Les dashboards lisent ces events", description: "Dashboards" },
      title: "Add analytics to this workspace?",
    });
    expect(host.added).toEqual([]);
    session.respond("req-1", { type: "allow", always: false });
    const answer = (await call) as { contentItems: Array<{ text: string }>; success: boolean };
    expect(answer.success).toBe(true);
    expect(answer.contentItems[0]?.text).toContain("in /workspaces/w/analytics, on the workspace's branch tester/w");
    expect(host.added).toEqual(["analytics"]);
  });

  test("Not now leaves the workspace as it is, and the agent is told not to insist", async () => {
    const { session, server, host } = await started();
    const call = server.ask(toolCall(3, "add_repo", { repo: "acme/Analytics", reason: "r" }));
    await flush();
    session.respond("req-1", { type: "deny", message: null });
    const answer = (await call) as { contentItems: Array<{ text: string }>; success: boolean };
    expect(answer.contentItems[0]?.text).toContain("chose not to add analytics");
    expect(answer.contentItems[0]?.text).toContain("do not propose it again");
    expect(host.added).toEqual([]);
  });

  test("a call Codex withdraws closes its card; an unknown or present repo is not asked", async () => {
    const { server, host } = await started();
    const call = server.ask(toolCall(4, "add_repo", { repo: "analytics", reason: "r" }));
    await flush();
    server.push({ method: "serverRequest/resolved", params: { threadId: THREAD_ID, requestId: 4 } } as never);
    const answer = (await call) as { contentItems: Array<{ text: string }>; success: boolean };
    expect(answer.success).toBe(false);
    expect(host.events.at(-1)).toMatchObject({ type: "request_resolved", request_id: "req-1", outcome: "cancelled" });

    const unknown = (await server.ask(toolCall(5, "add_repo", { repo: "nope", reason: "r" }))) as { contentItems: Array<{ text: string }>; success: boolean };
    expect(unknown.success).toBe(false);
    expect(unknown.contentItems[0]?.text).toContain("list_repos");
    const present = (await server.ask(toolCall(6, "add_repo", { repo: "web", reason: "r" }))) as { contentItems: Array<{ text: string }>; success: boolean };
    expect(present.contentItems[0]?.text).toContain("already in this workspace");
    expect(host.events.filter((e) => e.type === "permission_request")).toHaveLength(1);
    expect(host.added).toEqual([]);
  });

  test("a call Codex withdraws while the repos are read never asks", async () => {
    const { server, host } = await started();
    let listed!: () => void;
    const reading = new Promise<void>((resolve) => (listed = resolve));
    const list = host.listRepos.bind(host);
    host.listRepos = async () => {
      await reading;
      return list();
    };
    const call = server.ask(toolCall(8, "add_repo", { repo: "analytics", reason: "r" }));
    await flush();
    server.push({ method: "serverRequest/resolved", params: { threadId: THREAD_ID, requestId: 8 } } as never);
    listed();
    const answer = (await call) as { contentItems: Array<{ text: string }>; success: boolean };
    expect(answer.success).toBe(false);
    expect(answer.contentItems[0]?.text).toContain("withdrawn");
    expect(host.events.filter((e) => e.type === "permission_request")).toEqual([]);
  });

  test("delegate is offered when the daemon says so, and waits for the subagent's answer", async () => {
    const plain = await started();
    const tools = (spec: unknown) => (spec as Array<{ tools: Array<{ name: string }> }>)[0]?.tools.map((t) => t.name);
    expect(tools(plain.server.sent("thread/start")[0]?.dynamicTools)).toEqual(["list_repos", "add_repo"]);
    const { server, host } = await started({ delegate: true });
    expect(tools(server.sent("thread/start")[0]?.dynamicTools)).toEqual(["list_repos", "add_repo", "delegate"]);

    const call = server.ask(toolCall(9, "delegate", { agent: "claude", prompt: "Review the diff" }));
    await flush();
    expect(host.delegated).toEqual([{ agent: "claude", prompt: "Review the diff", tool_use_id: "call-9" }]);
    host.answerFor("sub-1", { agent: "claude", text: "Two bugs in main.rs." });
    const answer = (await call) as { contentItems: Array<{ text: string }>; success: boolean };
    expect(answer.success).toBe(true);
    expect(answer.contentItems[0]?.text).toContain("Claude (delegated chat sub-1");
    expect(answer.contentItems[0]?.text).toContain("Two bugs in main.rs.");
  });

  test("a delegate call Codex withdraws at its turn's end leaves the delegated chat working", async () => {
    const { server, host } = await started({ delegate: true });
    const call = server.ask(toolCall(10, "delegate", { agent: "codex", prompt: "Look" }));
    await flush();
    server.push({ method: "serverRequest/resolved", params: { threadId: THREAD_ID, requestId: 10 } } as never);
    const answer = (await call) as { contentItems: Array<{ text: string }>; success: boolean };
    expect(answer.success).toBe(true);
    expect(answer.contentItems[0]?.text).toContain("its answer will come to you as a new message");
    expect(host.detached).toEqual(["sub-1"]);
    expect(host.stopped).toEqual([]);
  });

  test("a delegate call still open when the turn completes leaves the delegated chat working", async () => {
    const { session, server, host } = await started({ delegate: true });
    session.send("go");
    await flush();
    server.push(codex.turnStarted("turn-1"));
    const call = server.ask(toolCall(10, "delegate", { agent: "codex", prompt: "Look" }));
    await flush();
    server.push(codex.turnCompleted("turn-1"));
    await call;
    expect(host.detached).toEqual(["sub-1"]);
    expect(host.stopped).toEqual([]);
  });

  test("the user's interrupt interrupts the delegated chat", async () => {
    const { session, server, host } = await started({ delegate: true });
    session.send("go");
    await flush();
    server.push(codex.turnStarted("turn-1"));
    const call = server.ask(toolCall(10, "delegate", { agent: "codex", prompt: "Look" }));
    await flush();
    await session.interrupt();
    // Codex then withdraws the call: it was already given up.
    server.push({ method: "serverRequest/resolved", params: { threadId: THREAD_ID, requestId: 10 } } as never);
    const answer = (await call) as { success: boolean };
    expect(answer.success).toBe(false);
    expect(host.stopped).toEqual(["sub-1"]);
    expect(host.detached).toEqual([]);
  });

  test("its calls show as Claude's MCP server names them", async () => {
    const { session, server, host } = await started();
    session.send("go");
    await flush();
    server.push(codex.turnStarted("turn-1"));
    server.push({
      method: "item/started",
      params: {
        threadId: THREAD_ID,
        turnId: "turn-1",
        startedAtMs: 1,
        item: { type: "dynamicToolCall", id: "call-7", namespace: "chauffeur", tool: "list_repos", arguments: {}, status: "inProgress", contentItems: null, success: null, durationMs: null },
      },
    } as never);
    expect(host.events.at(-1)).toMatchObject({ type: "tool_call", tool_use_id: "call-7", name: "mcp__chauffeur__list_repos" });
    // Their answer reads as text, not as Codex's content items.
    server.push({
      method: "item/completed",
      params: {
        threadId: THREAD_ID,
        turnId: "turn-1",
        completedAtMs: 2,
        item: {
          type: "dynamicToolCall",
          id: "call-7",
          namespace: "chauffeur",
          tool: "list_repos",
          arguments: {},
          status: "completed",
          contentItems: [{ type: "inputText", text: "In this workspace (w):" }],
          success: true,
          durationMs: 3,
        },
      },
    } as never);
    expect(host.events.at(-1)).toMatchObject({ type: "tool_result", tool_use_id: "call-7", output: "In this workspace (w):", is_error: false });
  });
});
