import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import type { CanUseTool, HookCallback, HookInput, PermissionUpdate, SDKMessage, Settings } from "@anthropic-ai/claude-agent-sdk";
import { CHAUFFEUR_TOOL_RULES, plansDirectory } from "../src/agents/claude/options.ts";
import { ClaudeSession } from "../src/agents/claude/session.ts";
import { ErrorCode, type SessionStartParams } from "../src/protocol.ts";
import { RpcError } from "../src/rpc/peer.ts";
import { FakeHost, FakeQuery, flush, sdk } from "./support/fakes.ts";

const PLANS = "/workspaces/ws-1/.chauffeur/plans";
const DESIGN = "/workspaces/ws-1/.chauffeur/designs/chat-1";

const BASE: SessionStartParams = {
  chat_id: "chat-1",
  workspace_id: "ws-1",
  agent: "claude",
  cwd: "/workspaces/ws-1",
  additional_directories: ["/extra"],
  fast: false,
  prompt_attachments: [],
  permission_mode: "default",
  read_only: false,
  prompt_suggestions: false,
  env: { CHAUFFEUR_WORKSPACE_ID: "ws-1" },
};

async function started(params: Partial<SessionStartParams> = {}, options: { initialized?: boolean } = {}) {
  const host = new FakeHost();
  let query: FakeQuery | undefined;
  let ids = 0;
  const session = new ClaudeSession({ ...BASE, ...params }, host, {
    query: ({ prompt, options: queryOptions }) => (query = new FakeQuery(prompt, queryOptions, options.initialized ?? true)),
    resolveExecutable: () => "/opt/claude",
    baseEnv: { PATH: "/usr/bin", HOME: "/home/me" },
    initTimeoutMs: 200,
    stopTimeoutMs: 200,
    newRequestId: () => `req-${++ids}`,
  });
  const result = options.initialized === false ? undefined : await session.start();
  return {
    session,
    host,
    result,
    /** Created by session.start(). */
    get query(): FakeQuery {
      if (!query) throw new Error("the session has not created its query yet");
      return query;
    },
  };
}

function canUseTool(query: FakeQuery): CanUseTool {
  const callback = query.options.canUseTool;
  if (!callback) throw new Error("no canUseTool");
  return callback;
}

const SUGGESTIONS: PermissionUpdate[] = [
  { type: "addRules", rules: [{ toolName: "Bash", ruleContent: "ls:*" }], behavior: "allow", destination: "session" },
];

function toolOptions(signal = new AbortController().signal) {
  return { signal, suggestions: SUGGESTIONS, title: "Claude wants to run ls", toolUseID: "toolu_1", requestId: "ctl_1" };
}

describe("starting", () => {
  test("builds the SDK options from the start params", async () => {
    const { query, result } = await started({
      model: "claude-haiku-4-5-20251001",
      effort: "high",
      append_system_prompt: "You are in Chauffeur.",
    });
    expect(result).toEqual({ agent_session_id: null });
    const o = query.options;
    expect(o.cwd).toBe("/workspaces/ws-1");
    expect(o.additionalDirectories).toEqual(["/extra"]);
    expect(o.env).toMatchObject({ PATH: "/usr/bin", HOME: "/home/me", CHAUFFEUR_WORKSPACE_ID: "ws-1" });
    expect(o.env?.CLAUDE_AGENT_SDK_CLIENT_APP).toStartWith("chauffeur-sidecar/");
    expect(o.settingSources).toEqual(["user", "project", "local"]);
    expect(o.includePartialMessages).toBe(true);
    // Chauffeur stops tasks one by one, so an interrupt spares background tasks.
    expect(o.perTaskStopAffordance).toBe(true);
    expect(o.agentProgressSummaries).toBe(true);
    expect(o.permissionMode).toBe("default");
    expect(o.allowDangerouslySkipPermissions).toBe(true);
    expect(o.model).toBe("claude-haiku-4-5-20251001");
    expect(o.effort).toBe("high");
    expect(o.resume).toBeUndefined();
    expect(o.systemPrompt).toEqual({ type: "preset", preset: "claude_code", append: "You are in Chauffeur." });
    expect(o.pathToClaudeCodeExecutable).toBe("/opt/claude");
    expect(Object.keys(o.hooks ?? {}).sort()).toEqual(["PreToolUse", "WorktreeCreate", "WorktreeRemove"]);
    // Question previews in markdown, never HTML; suggestions as the settings say.
    expect(o.toolConfig).toEqual({ askUserQuestion: { previewFormat: "markdown" } });
    expect(o.promptSuggestions).toBe(false);
  });

  test("prompt suggestions are asked for when the settings keep them on", async () => {
    const { query } = await started({ prompt_suggestions: true });
    expect(query.options.promptSuggestions).toBe(true);
  });

  test("maps every permission mode", async () => {
    const modes = { auto: "auto", default: "default", accept_edits: "acceptEdits", plan: "plan", bypass_permissions: "bypassPermissions" } as const;
    for (const [ours, theirs] of Object.entries(modes)) {
      const { query } = await started({ permission_mode: ours as SessionStartParams["permission_mode"] });
      expect(query.options.permissionMode).toBe(theirs);
    }
  });

  test("auto mode that Claude Code could not take says that the chat asks instead", async () => {
    const make = async (permission_mode: SessionStartParams["permission_mode"], current: string) => {
      const host = new FakeHost();
      const session = new ClaudeSession({ ...BASE, permission_mode, model: "claude-haiku-4-5-20251001" }, host, {
        query: ({ prompt, options }) => {
          const query = new FakeQuery(prompt, options);
          query.initResult.current_permission_mode = current;
          return query;
        },
        resolveExecutable: () => undefined,
        baseEnv: {},
        initTimeoutMs: 200,
      });
      await session.start();
      return host.events;
    };
    expect(await make("auto", "default")).toEqual([
      // The daemon shows the chat asking, rather than in auto mode.
      { type: "permission_mode_effective", mode: "default" },
      {
        type: "notice",
        level: "warning",
        message:
          'Auto mode is unavailable for this chat with claude-haiku-4-5-20251001 (its model, plan or settings turn it off). Claude asks before acting instead, as in "Ask for permissions"',
      },
    ]);
    expect(await make("auto", "auto")).toEqual([]);
    expect(await make("default", "default")).toEqual([]);
  });

  test("an exploration plans, whatever its mode, cannot leave plan mode, and writes its plan to the notes", async () => {
    const { query } = await started({ read_only: true, permission_mode: "bypass_permissions", plans_dir: PLANS });
    expect(query.options.permissionMode).toBe("plan");
    expect(query.options.allowDangerouslySkipPermissions).toBe(false);
    expect(query.options.disallowedTools).toEqual(["NotebookEdit", "ExitPlanMode"]);
    expect(query.options.settings).toEqual({ plansDirectory: ".chauffeur/plans", permissions: { allow: CHAUFFEUR_TOOL_RULES } });
    // The plan guard: on every tool, after the Bash guards.
    expect(query.options.hooks?.PreToolUse?.map((m) => m.matcher)).toEqual(["Bash", undefined]);
  });

  test("an exploration's commands run in the sandbox, which keeps them out of its notes", async () => {
    const { query } = await started({ read_only: true, plans_dir: PLANS, notes_dir: "/workspaces/ws-1/.chauffeur" });
    expect(query.options.sandbox).toMatchObject({
      enabled: true,
      failIfUnavailable: true,
      allowUnsandboxedCommands: false,
      autoAllowBashIfSandboxed: false,
      network: { allowedDomains: ["github.com", "*.github.com", "*.githubusercontent.com"], strictAllowlist: true },
      // Plans come through the Write tool and the plan guard, which the sandbox does not cover.
      filesystem: { denyWrite: ["/workspaces/ws-1/.chauffeur"] },
    });
    // Without the notes folder named (an older daemon), the rest holds.
    const older = (await started({ read_only: true, plans_dir: PLANS })).query;
    expect(older.options.sandbox?.enabled).toBe(true);
    expect(older.options.sandbox?.filesystem).toBeUndefined();
    // A design chat writes its own folder, inside the notes: its sandbox denies the repos instead.
    const design = (
      await started({ read_only: true, design_dir: "/workspaces/ws-1/.chauffeur/designs/c1", notes_dir: "/workspaces/ws-1/.chauffeur" })
    ).query;
    expect(design.options.sandbox?.filesystem?.denyWrite).not.toContain("/workspaces/ws-1/.chauffeur");
    // A workspace's commands are not sandboxed by Chauffeur.
    expect((await started({ notes_dir: "/workspaces/ws-1/.chauffeur" })).query.options.sandbox).toBeUndefined();
  });

  test("an exploration's private temporary folder reaches Claude Code", async () => {
    const { query } = await started({ read_only: true, env: { CLAUDE_CODE_TMPDIR: "/tmp/chauffeur-501/f0bbc4a593" } });
    expect(query.options.env?.CLAUDE_CODE_TMPDIR).toBe("/tmp/chauffeur-501/f0bbc4a593");
  });

  test("the plans folder is given relative to the session's folder, which Claude Code resolves itself", () => {
    expect(plansDirectory("/tmp/ws/.chauffeur/plans", "/tmp/ws")).toBe(".chauffeur/plans");
    expect(plansDirectory("/tmp/ws/.chauffeur/plans/", "/tmp/ws/")).toBe(".chauffeur/plans");
    // Not under it: as given (Claude Code then keeps its own folder).
    expect(plansDirectory("/elsewhere/plans", "/tmp/ws")).toBe("/elsewhere/plans");
    expect(plansDirectory("/tmp/ws", "/tmp/ws")).toBe("/tmp/ws");
  });

  test("an exploration without a plans folder does without every edit tool", async () => {
    const { query } = await started({ read_only: true });
    expect(query.options.disallowedTools).toEqual(["Edit", "Write", "MultiEdit", "NotebookEdit", "ExitPlanMode"]);
    expect(query.options.settings).toEqual({ permissions: { allow: CHAUFFEUR_TOOL_RULES } });
  });

  test("a workspace's plans go to its notes too, with no tool taken away and no plan guard", async () => {
    const { query } = await started({ plans_dir: PLANS });
    expect(query.options.settings).toEqual({ plansDirectory: ".chauffeur/plans", permissions: { allow: CHAUFFEUR_TOOL_RULES } });
    expect(query.options.disallowedTools).toBeUndefined();
    expect(query.options.permissionMode).toBe("default");
    expect(query.options.hooks?.PreToolUse?.map((m) => m.matcher)).toEqual(["Bash"]);
  });

  test("a design chat runs in auto mode, whatever its mode, writing only in its folder, Bash sandboxed off the repos", async () => {
    const { query } = await started({
      cwd: DESIGN,
      additional_directories: ["/workspaces/ws-1/web", "/workspaces/ws-1/api"],
      permission_mode: "bypass_permissions",
      plans_dir: PLANS,
      design_dir: DESIGN,
    });
    expect(query.options.permissionMode).toBe("auto");
    expect(query.options.allowDangerouslySkipPermissions).toBe(false);
    expect(query.options.disallowedTools).toEqual(["NotebookEdit", "ExitPlanMode", "EnterWorktree"]);
    expect(query.options.sandbox).toEqual({
      enabled: true,
      failIfUnavailable: true,
      allowUnsandboxedCommands: false,
      autoAllowBashIfSandboxed: true,
      network: {
        allowedDomains: ["github.com", "*.github.com", "*.githubusercontent.com"],
        strictAllowlist: true,
        allowUnixSockets: [],
        allowAllUnixSockets: false,
        allowLocalBinding: false,
      },
      filesystem: { denyWrite: ["/workspaces/ws-1/web", "/workspaces/ws-1/api"] },
      credentials: { envVars: [{ name: "TYPESAFE_API_KEY", mode: "deny" }] },
    });
    // The Artifact tool asks each time (a rule, and a hook whatever auto mode decides); no plans
    // folder for a chat that never plans.
    expect(query.options.settings).toEqual({ permissions: { ask: ["Artifact"] } });
    expect(query.options.hooks?.PreToolUse?.map((m) => m.matcher)).toEqual(["Bash", undefined, "Artifact"]);
    const [ask] = query.options.hooks?.PreToolUse?.[2]?.hooks ?? [];
    expect(await ask?.({ hook_event_name: "PreToolUse", tool_name: "Artifact", tool_input: {} } as never, "toolu_1", { signal: new AbortController().signal })).toMatchObject({
      hookSpecificOutput: { permissionDecision: "ask" },
    });
  });

  test("a design chat reads the workspace's notes without asking, as given and as resolved", async () => {
    const ws = realpathSync(mkdtempSync(join(tmpdir(), "design-notes-")));
    try {
      const notes = join(ws, ".chauffeur");
      mkdirSync(notes);
      const link = `${ws}-link`;
      symlinkSync(ws, link);
      try {
        const { query } = await started({ cwd: DESIGN, design_dir: DESIGN, notes_dir: join(link, ".chauffeur") });
        expect(query.options.settings).toEqual({
          permissions: { ask: ["Artifact"], allow: [`Read(/${link}/.chauffeur/**)`, `Read(/${notes}/**)`] },
        });
      } finally {
        rmSync(link);
      }
      const plain = await started({ cwd: DESIGN, design_dir: DESIGN, notes_dir: notes });
      expect(plain.query.options.settings).toEqual({ permissions: { ask: ["Artifact"], allow: [`Read(/${notes}/**)`] } });
    } finally {
      rmSync(ws, { recursive: true, force: true });
    }
  });

  test("a design chat in an exploration is a design chat: its mode and its folder, the repos still out of reach", async () => {
    const { query } = await started({ cwd: DESIGN, additional_directories: ["/x/repo"], read_only: true, plans_dir: PLANS, design_dir: DESIGN });
    expect(query.options.permissionMode).toBe("auto");
    expect(query.options.disallowedTools).toEqual(["NotebookEdit", "ExitPlanMode", "EnterWorktree"]);
    expect(query.options.sandbox?.filesystem).toEqual({ denyWrite: ["/x/repo"] });
    expect(query.options.sandbox?.autoAllowBashIfSandboxed).toBe(true);
    expect(query.options.settings).toEqual({ permissions: { ask: ["Artifact"] } });
  });

  test("a design chat's Artifact rule is merged with the other flag settings", async () => {
    const { query } = await started({ cwd: DESIGN, design_dir: DESIGN, fast: true });
    expect(query.options.settings).toEqual({ fastMode: true, permissions: { ask: ["Artifact"] } });
  });

  test("resumes with the given id and returns it", async () => {
    const { query, result } = await started({ resume_session_id: "sess-old" });
    expect(query.options.resume).toBe("sess-old");
    expect(result).toEqual({ agent_session_id: "sess-old" });
  });

  test("rejects an unknown effort", async () => {
    const host = new FakeHost();
    const session = new ClaudeSession({ ...BASE, effort: "extreme" }, host, {
      query: () => {
        throw new Error("not reached");
      },
      resolveExecutable: () => undefined,
      baseEnv: {},
    });
    await expect(session.start()).rejects.toMatchObject({ code: ErrorCode.INVALID_PARAMS });
  });

  test("fails when the CLI does not initialize, without emitting events", async () => {
    const fixture = await started({}, { initialized: false });
    const start = fixture.session.start();
    await flush();
    fixture.query.initialize(new Error("spawn ENOENT"));
    const error = await start.catch((e: unknown) => e);
    expect(error).toBeInstanceOf(RpcError);
    expect(error).toMatchObject({ code: ErrorCode.UNAVAILABLE, message: "Claude Code failed to start: spawn ENOENT" });
    expect(fixture.query.calls).toContain("close");
    expect(fixture.host.events).toEqual([]);
  });

  test("a stop while starting fails the start at once", async () => {
    const fixture = await started({}, { initialized: false });
    const start = fixture.session.start();
    await flush();
    const begun = Date.now();
    await fixture.session.stop();
    await expect(start).rejects.toMatchObject({ code: ErrorCode.UNAVAILABLE, message: "Claude Code failed to start: the session was stopped while starting" });
    expect(Date.now() - begun).toBeLessThan(150);
    expect(fixture.host.events).toEqual([]);
  });

  test("sends the first prompt once initialized, announcing the turn", async () => {
    const fixture = await started({ prompt: "Fix the bug" }, { initialized: false });
    const start = fixture.session.start();
    fixture.session.send("and add a test"); // sent while starting: queued after the prompt
    await flush();
    const query = fixture.query;
    expect(query.sent).toEqual([]);
    query.initialize();
    await start;
    await query.waitForSent(2);
    expect(query.sent.map((m) => m.message.content)).toEqual(["Fix the bug", "and add a test"]);
    expect(query.sent[0]).toMatchObject({ type: "user", parent_tool_use_id: null });
    expect(fixture.host.types()).toEqual(["turn_started"]);
  });
});

describe("turns", () => {
  test("maps a turn: turn_started on send, then the agent's events, then turn_ended", async () => {
    const { session, query, host } = await started();
    session.send("hi");
    await query.waitForSent(1);
    query.push(sdk.init("sess-1"), sdk.text("m1", "hello"), sdk.result());
    await flush();
    expect(host.types()).toEqual(["turn_started", "session_started", "assistant_text", "context_usage", "turn_ended"]);
    // The context is read once per turn, as a summary, before the turn ends.
    expect(query.contextReads).toEqual(["summary"]);
    expect(host.events[3]).toMatchObject({ type: "context_usage", usage: { model: "claude-opus-5-5", max_tokens: 200_000 } });
  });

  test("a message sent mid-turn does not start a new turn until the CLI runs it", async () => {
    const { session, query, host } = await started();
    session.send("first");
    await query.waitForSent(1);
    query.push(sdk.text("m1", "working"));
    session.send("second");
    await query.waitForSent(2);
    expect(host.types()).toEqual(["turn_started", "assistant_text"]);
    query.push(sdk.result({ queued_turn_count: 1 }));
    await flush();
    expect(host.types()).toEqual(["turn_started", "assistant_text", "context_usage", "turn_ended", "turn_started"]);
    query.push(sdk.text("m2", "done"), sdk.result({ queued_turn_count: 0 }));
    await flush();
    expect(host.types().slice(-3)).toEqual(["assistant_text", "context_usage", "turn_ended"]);
  });

  test("a suggestion after the turn is passed on; one arriving once another turn runs is dropped", async () => {
    const { session, query, host } = await started({ prompt_suggestions: true });
    session.send("first");
    await query.waitForSent(1);
    query.push(sdk.text("m1", "done"), sdk.result(), sdk.suggestion("Now run the tests"));
    await flush();
    expect(host.events.at(-1)).toEqual({ type: "prompt_suggestion", text: "Now run the tests" });
    session.send("second");
    await query.waitForSent(2);
    query.push(sdk.result({ queued_turn_count: 1 }), sdk.suggestion("stale"));
    await flush();
    expect(host.types().slice(-2)).toEqual(["turn_ended", "turn_started"]);
    expect(host.events.filter((e) => e.type === "prompt_suggestion")).toHaveLength(1);
  });

  test("a turn the CLI starts on its own still gets turn_started first", async () => {
    const { query, host } = await started();
    query.push(sdk.text("m1", "background task finished"), sdk.result());
    await flush();
    expect(host.types()).toEqual(["turn_started", "assistant_text", "context_usage", "turn_ended"]);
  });

  test("a background subagent working after the turn ended opens no turn; the agent's own turn when it hands back does", async () => {
    const { session, query, host } = await started();
    session.send("start a background subagent");
    await query.waitForSent(1);
    query.push(sdk.toolUse("m1", "toolu_agent", "Agent", { run_in_background: true }), sdk.text("m2", "started"), sdk.result());
    await flush();
    expect(host.types().at(-1)).toBe("turn_ended");
    const ended = host.events.length;
    // The subagent streams, calls tools and gets their results, after the turn ended.
    query.push(
      {
        type: "stream_event",
        parent_tool_use_id: "toolu_agent",
        event: { type: "message_start", message: { id: "s1", usage: { input_tokens: 1, output_tokens: 1 } } },
      } as never,
      { ...sdk.toolUse("s1", "toolu_sub", "Bash", { command: "ping" }), parent_tool_use_id: "toolu_agent" } as never,
      { ...sdk.toolResult("toolu_sub", "done"), parent_tool_use_id: "toolu_agent" } as never,
      sdk.text("s2", "pinged", "toolu_agent"),
    );
    await flush();
    const types = host.events.slice(ended).map((event) => event.type);
    expect(types).not.toContain("turn_started");
    expect(types).toContain("tool_call");
    // A message sent meanwhile starts a turn at once.
    expect(session.idle()).toBe(true);
    // The hand-back: Claude Code runs a turn of the agent's own, which is the agent working.
    query.push(sdk.text("m3", "the subagent finished: pinged"), sdk.result());
    await flush();
    expect(host.types().slice(ended)).toEqual(expect.arrayContaining(["turn_started", "assistant_text", "turn_ended"]));
    expect(host.types().slice(ended).filter((type) => type === "turn_started")).toHaveLength(1);
    expect(host.types().slice(ended).indexOf("turn_started")).toBeGreaterThan(types.length - 1);
  });

  test("a crash mid-turn reports an error, ends the turn and frees the chat", async () => {
    const { session, query, host } = await started();
    session.send("hi");
    await query.waitForSent(1);
    query.crash(new Error("Claude Code process exited with code 1"));
    await flush();
    expect(host.events.slice(1)).toEqual([
      { type: "notice", level: "error", message: "Claude session ended: Claude Code process exited with code 1" },
      { type: "turn_ended", outcome: "error", duration_ms: null, cost_usd: null, context_tokens: null, error: "Claude Code process exited with code 1" },
    ]);
    expect(host.endedCount).toBe(1);
    expect(() => session.send("again")).toThrow(RpcError);
  });

  test("a CLI that exits while idle is reported without a turn", async () => {
    const { query, host } = await started();
    query.exit();
    await flush();
    expect(host.events).toEqual([{ type: "notice", level: "error", message: "Claude session ended: Claude Code exited unexpectedly" }]);
    expect(host.endedCount).toBe(1);
  });
});

describe("permission requests and questions", () => {
  test("a permission request is emitted, answered with allow always, and resolved", async () => {
    const { session, query, host } = await started();
    const decision = canUseTool(query)("Bash", { command: "ls" }, toolOptions());
    await flush();
    expect(host.events).toEqual([
      { type: "permission_request", request_id: "req-1", tool_name: "Bash", input: { command: "ls" }, title: "Claude wants to run ls" },
    ]);
    session.respond("req-1", { type: "allow", always: true });
    expect(await decision).toEqual({ behavior: "allow", updatedInput: { command: "ls" }, updatedPermissions: SUGGESTIONS });
    expect(host.events[1]).toEqual({ type: "request_resolved", request_id: "req-1", outcome: "allowed", rules: ["Bash(ls:*)"] });
  });

  test("always allow without suggestions (plan mode, an MCP tool) remembers the tool for the session", async () => {
    const { session, query, host } = await started();
    const ask = canUseTool(query);
    const plain = { ...toolOptions(), suggestions: undefined, title: undefined };
    const first = ask("mcp__notes__write_note", { text: "one" }, plain);
    await flush();
    session.respond("req-1", { type: "allow", always: true });
    expect(await first).toEqual({
      behavior: "allow",
      updatedInput: { text: "one" },
      updatedPermissions: [
        { type: "addRules", rules: [{ toolName: "mcp__notes__write_note" }], behavior: "allow", destination: "session" },
      ],
    });
    // Claude Code asks again (plan mode does whatever the rules say): the chat answers, unasked.
    expect(await ask("mcp__notes__write_note", { text: "two" }, plain)).toEqual({ behavior: "allow", updatedInput: { text: "two" } });
    // Another tool is still asked.
    void ask("mcp__notes__delete_note", { id: "1" }, plain);
    await flush();
    expect(host.events.filter((e) => e.type === "permission_request").map((e) => e.request_id)).toEqual(["req-1", "req-2"]);
    expect(host.events.filter((e) => e.type === "request_resolved")).toEqual([
      { type: "request_resolved", request_id: "req-1", outcome: "allowed", rules: ["mcp__notes__write_note"] },
    ]);
  });

  test("always allow also allows the open requests its rules cover, and only those", async () => {
    const { session, query, host } = await started();
    const ask = canUseTool(query);
    const plain = { ...toolOptions(), suggestions: undefined, title: undefined };
    const first = ask("mcp__notes__write_note", { text: "one" }, plain);
    const second = ask("mcp__notes__write_note", { text: "two" }, { ...plain, agentID: "zz9" });
    const other = ask("Bash", { command: "make" }, plain);
    await flush();
    session.respond("req-1", { type: "allow", always: true });
    await first;
    expect(await second).toEqual({ behavior: "allow", updatedInput: { text: "two" } });
    expect(host.events.filter((e) => e.type === "request_resolved")).toEqual([
      { type: "request_resolved", request_id: "req-1", outcome: "allowed", rules: ["mcp__notes__write_note"] },
      { type: "request_resolved", request_id: "req-2", outcome: "allowed", rules: ["mcp__notes__write_note"] },
    ]);
    session.respond("req-3", { type: "deny", message: null });
    expect(await other).toEqual({ behavior: "deny", message: "Denied by the user" });
  });

  test("always allow on a read outside the working directories allows the open reads of that folder", async () => {
    // A design chat reading four images of the notes at once: one answer, not four.
    const { session, query, host } = await started();
    const ask = canUseTool(query);
    const notes = {
      ...toolOptions(),
      suggestions: [
        { type: "addRules", rules: [{ toolName: "Read", ruleContent: "//ws/.chauffeur/**" }], behavior: "allow", destination: "session" },
      ] satisfies PermissionUpdate[],
    };
    const first = ask("Read", { file_path: "/ws/.chauffeur/acme-debug.png" }, notes);
    const second = ask("Read", { file_path: "/ws/.chauffeur/attachments/pasted.png" }, notes);
    const outside = ask("Read", { file_path: "/ws/.env" }, notes);
    await flush();
    session.respond("req-1", { type: "allow", always: true });
    await first;
    expect(await second).toEqual({ behavior: "allow", updatedInput: { file_path: "/ws/.chauffeur/attachments/pasted.png" } });
    expect(host.events.filter((e) => e.type === "request_resolved")).toEqual([
      { type: "request_resolved", request_id: "req-1", outcome: "allowed", rules: ["Read(//ws/.chauffeur/**)"] },
      { type: "request_resolved", request_id: "req-2", outcome: "allowed", rules: ["Read(//ws/.chauffeur/**)"] },
    ]);
    // Later reads there are not asked; elsewhere they still are.
    expect(await ask("Read", { file_path: "/ws/.chauffeur/b.png" }, notes)).toEqual({ behavior: "allow", updatedInput: { file_path: "/ws/.chauffeur/b.png" } });
    session.respond("req-3", { type: "deny", message: null });
    expect(await outside).toEqual({ behavior: "deny", message: "Denied by the user" });
  });

  test("always allow keeps a Bash command, not Bash, when Claude suggests nothing", async () => {
    const { session, query, host } = await started();
    const ask = canUseTool(query);
    const plain = { ...toolOptions(), suggestions: undefined };
    const first = ask("Bash", { command: "make test" }, plain);
    await flush();
    session.respond("req-1", { type: "allow", always: true });
    expect(await first).toMatchObject({
      updatedPermissions: [{ type: "addRules", rules: [{ toolName: "Bash", ruleContent: "make test" }], destination: "session" }],
    });
    expect(await ask("Bash", { command: "make test" }, plain)).toEqual({ behavior: "allow", updatedInput: { command: "make test" } });
    void ask("Bash", { command: "make deploy" }, plain);
    await flush();
    expect(host.events.at(-1)).toMatchObject({ type: "permission_request", request_id: "req-2" });
  });

  test("always allow on a command writing outside the folder keeps the chat's mode and folders", async () => {
    const { session, query, host } = await started({ permission_mode: "default" });
    const ask = canUseTool(query);
    // Claude Code's suggestions in default mode for a command writing outside the working directories.
    const outside = {
      ...toolOptions(),
      suggestions: [
        { type: "addDirectories", directories: ["/tmp/out"], destination: "session" },
        { type: "setMode", mode: "acceptEdits", destination: "session" },
      ] satisfies PermissionUpdate[],
    };
    const first = ask("Bash", { command: "mkdir -p /tmp/out/x" }, outside);
    await flush();
    session.respond("req-1", { type: "allow", always: true });
    expect(await first).toEqual({
      behavior: "allow",
      updatedInput: { command: "mkdir -p /tmp/out/x" },
      updatedPermissions: [
        { type: "addRules", rules: [{ toolName: "Bash", ruleContent: "mkdir -p /tmp/out/x" }], behavior: "allow", destination: "session" },
      ],
    });
    expect(host.events.at(-1)).toEqual({ type: "request_resolved", request_id: "req-1", outcome: "allowed", rules: ["Bash(mkdir -p /tmp/out/x)"] });
    // Claude Code asks again for that folder whatever its rules: the chat answers, unasked.
    expect(await ask("Bash", { command: "mkdir -p /tmp/out/x" }, outside)).toEqual({ behavior: "allow", updatedInput: { command: "mkdir -p /tmp/out/x" } });
    expect(host.events.filter((e) => e.type === "permission_request")).toHaveLength(1);
  });

  test("a new session of the chat keeps not asking for what was allowed always before", async () => {
    const { query, host } = await started({ always_allow: ["mcp__notes__write_note"] });
    const plain = { ...toolOptions(), suggestions: undefined };
    expect(await canUseTool(query)("mcp__notes__write_note", { text: "a" }, plain)).toEqual({ behavior: "allow", updatedInput: { text: "a" } });
    expect(host.events.filter((e) => e.type === "permission_request")).toEqual([]);
  });

  test("always allow is a single allow when Claude Code says a rule would grant more", async () => {
    const { session, query, host } = await started();
    const decision = canUseTool(query)("Bash", { command: "ls" }, { ...toolOptions(), suppressAlwaysAllowRule: true });
    await flush();
    session.respond("req-1", { type: "allow", always: true });
    expect(await decision).toEqual({ behavior: "allow", updatedInput: { command: "ls" } });
    expect(host.events.at(-1)).toEqual({ type: "request_resolved", request_id: "req-1", outcome: "allowed" });
  });

  test("a call put to the user is no longer being checked by auto mode", async () => {
    const { query, host } = await started();
    query.push({ type: "system", subtype: "permission_check_status", tool_use_id: "toolu_1", status: "checking", uuid: "u", session_id: "s" } as unknown as SDKMessage);
    await flush();
    void canUseTool(query)("Bash", { command: "ls" }, toolOptions());
    await flush();
    expect(host.types()).toEqual(["permission_check", "permission_check", "permission_request"]);
    expect(host.events[1]).toEqual({ type: "permission_check", tool_use_id: "toolu_1", checking: false });
  });

  test("a subagent's request names it, with what its task said", async () => {
    const { query, host } = await started();
    query.push({
      type: "system",
      subtype: "task_started",
      task_id: "a1b2",
      description: "Find the flaky tests",
      subagent_type: "general-purpose",
      task_type: "local_agent",
      is_backgrounded: true,
      tool_use_id: "toolu_agent",
      uuid: "u",
      session_id: "s",
    } as never);
    await flush();
    void canUseTool(query)("Bash", { command: "ls" }, { ...toolOptions(), agentID: "a1b2" });
    // A subagent whose task Claude Code did not report: its id only.
    void canUseTool(query)("Bash", { command: "pwd" }, { ...toolOptions(), agentID: "zz9" });
    await flush();
    expect(host.events.filter((e) => e.type === "permission_request")).toEqual([
      {
        type: "permission_request",
        request_id: "req-1",
        tool_name: "Bash",
        input: { command: "ls" },
        title: "Claude wants to run ls",
        agent: { id: "a1b2", subagent_type: "general-purpose", description: "Find the flaky tests", background: true },
      },
      {
        type: "permission_request",
        request_id: "req-2",
        tool_name: "Bash",
        input: { command: "pwd" },
        title: "Claude wants to run ls",
        agent: { id: "zz9", subagent_type: null, description: null, background: false },
      },
    ]);
  });

  test("allow once does not persist the suggestions", async () => {
    const { session, query } = await started();
    const decision = canUseTool(query)("Write", { file_path: "/a" }, toolOptions());
    await flush();
    session.respond("req-1", { type: "allow", always: false });
    expect(await decision).toEqual({ behavior: "allow", updatedInput: { file_path: "/a" } });
  });

  test("deny carries the user's message, or a default one", async () => {
    const { session, query, host } = await started();
    const first = canUseTool(query)("Bash", { command: "rm -rf /" }, toolOptions());
    const second = canUseTool(query)("Bash", { command: "rm -rf ~" }, toolOptions());
    await flush();
    session.respond("req-1", { type: "deny", message: "Not that" });
    session.respond("req-2", { type: "deny", message: null });
    expect(await first).toEqual({ behavior: "deny", message: "Not that" });
    expect(await second).toEqual({ behavior: "deny", message: "Denied by the user" });
    expect(host.events.filter((e) => e.type === "request_resolved")).toEqual([
      { type: "request_resolved", request_id: "req-1", outcome: "denied" },
      { type: "request_resolved", request_id: "req-2", outcome: "denied" },
    ]);
  });

  test("AskUserQuestion becomes a question whose answers are fed back as the tool input", async () => {
    const { session, query, host } = await started();
    const input = {
      questions: [
        {
          question: "Which database?",
          header: "Database",
          options: [
            { label: "Postgres", description: "Relational", preview: "```sql\nselect 1;\n```" },
            { label: "Redis", description: "In memory", preview: "  " },
          ],
          multiSelect: false,
        },
        { question: "Which features?", header: "Features", options: [{ label: "A", description: "a" }], multiSelect: true },
      ],
    };
    const decision = canUseTool(query)("AskUserQuestion", input, toolOptions());
    await flush();
    expect(host.events).toEqual([
      {
        type: "question",
        request_id: "req-1",
        questions: [
          {
            question: "Which database?",
            header: "Database",
            options: [
              // The preview is kept; a blank one is left out.
              { label: "Postgres", description: "Relational", preview: "```sql\nselect 1;\n```" },
              { label: "Redis", description: "In memory" },
            ],
            multi_select: false,
          },
          { question: "Which features?", header: "Features", options: [{ label: "A", description: "a" }], multi_select: true },
        ],
      },
    ]);
    const answers = { "Which database?": "Postgres", "Which features?": "A" };
    session.respond("req-1", { type: "answer", answers });
    expect(await decision).toEqual({ behavior: "allow", updatedInput: { ...input, answers } });
    expect(host.events[1]).toEqual({ type: "request_resolved", request_id: "req-1", outcome: "answered" });
  });

  test("unknown ids and answers to permission requests are refused", async () => {
    const { session, query } = await started();
    void canUseTool(query)("Bash", { command: "ls" }, toolOptions());
    await flush();
    expect(() => session.respond("nope", { type: "allow", always: false })).toThrow(
      expect.objectContaining({ code: ErrorCode.NOT_FOUND }),
    );
    expect(() => session.respond("req-1", { type: "answer", answers: {} })).toThrow(
      expect.objectContaining({ code: ErrorCode.BAD_REQUEST }),
    );
    session.respond("req-1", { type: "allow", always: false });
    expect(() => session.respond("req-1", { type: "allow", always: false })).toThrow(
      expect.objectContaining({ code: ErrorCode.NOT_FOUND }),
    );
  });

  test("a request the SDK aborts is resolved as cancelled", async () => {
    const { query, host } = await started();
    const abort = new AbortController();
    const decision = canUseTool(query)("Bash", { command: "ls" }, toolOptions(abort.signal));
    await flush();
    abort.abort();
    expect(await decision).toMatchObject({ behavior: "deny" });
    expect(host.events[1]).toEqual({ type: "request_resolved", request_id: "req-1", outcome: "cancelled" });
  });

  test("a request still open when the CLI dies is reported cancelled, even with no turn running", async () => {
    // A background subagent asks while the agent itself rests.
    const { query, host } = await started();
    const decision = canUseTool(query)("Bash", { command: "ls" }, toolOptions());
    await flush();
    query.crash(new Error("Claude Code process exited with code 1"));
    await flush();
    expect(await decision).toMatchObject({ behavior: "deny" });
    expect(host.events).toContainEqual({ type: "request_resolved", request_id: "req-1", outcome: "cancelled" });
    expect(host.types()).not.toContain("turn_ended");
  });

  test("interrupt denies pending requests, interrupts the query and ends the turn as interrupted", async () => {
    const { session, query, host } = await started();
    session.send("do it");
    await query.waitForSent(1);
    const decision = canUseTool(query)("Bash", { command: "make" }, toolOptions());
    await flush();
    await session.interrupt();
    expect(await decision).toMatchObject({ behavior: "deny" });
    expect(query.calls).toContain("interrupt");
    query.push(sdk.result({ subtype: "error_during_execution", is_error: true, errors: ["Request was aborted"] }));
    await flush();
    expect(host.types()).toEqual(["turn_started", "permission_request", "request_resolved", "context_usage", "turn_ended"]);
    expect(host.events[2]).toMatchObject({ outcome: "cancelled" });
    expect(host.events[4]).toMatchObject({ outcome: "interrupted", error: null });
  });

  test("interrupt spares a background subagent's request, which stays answerable", async () => {
    const { session, query, host } = await started();
    const subagent = (taskId: string, background: boolean) =>
      ({
        type: "system",
        subtype: "task_started",
        task_id: taskId,
        description: `Agent ${taskId}`,
        subagent_type: "general-purpose",
        task_type: "local_agent",
        is_backgrounded: background,
        tool_use_id: `toolu_${taskId}`,
        uuid: `u-${taskId}`,
        session_id: "s",
      }) as never;
    query.push(subagent("bg", true));
    query.push(subagent("fg", false));
    query.push(subagent("moved", false));
    await flush();
    session.send("do it");
    await query.waitForSent(1);
    const background = canUseTool(query)("Bash", { command: "echo hello > greeting.txt" }, { ...toolOptions(), agentID: "bg" });
    const foreground = canUseTool(query)("Bash", { command: "make" }, { ...toolOptions(), agentID: "fg" });
    const moved = canUseTool(query)("Bash", { command: "make test" }, { ...toolOptions(), agentID: "moved" });
    const own = canUseTool(query)("Bash", { command: "make lint" }, toolOptions());
    await flush();
    // Moved to the background after it asked (Ctrl+B in Claude Code's terminal).
    query.push({ type: "system", subtype: "task_updated", task_id: "moved", patch: { is_backgrounded: true }, uuid: "u", session_id: "s" } as never);
    await flush();
    await session.interrupt();
    expect(query.calls).toContain("interrupt");
    expect(await foreground).toMatchObject({ behavior: "deny" });
    expect(await own).toMatchObject({ behavior: "deny" });
    const resolved = () => host.events.filter((e) => e.type === "request_resolved");
    expect(resolved()).toEqual([
      { type: "request_resolved", request_id: "req-2", outcome: "cancelled" },
      { type: "request_resolved", request_id: "req-4", outcome: "cancelled" },
    ]);
    query.push(sdk.result({ subtype: "error_during_execution", is_error: true, errors: ["Request was aborted"] }));
    await flush();
    // After the turn, the background subagents' requests are still answered.
    session.respond("req-1", { type: "allow", always: false });
    session.respond("req-3", { type: "deny", message: null });
    expect(await background).toEqual({ behavior: "allow", updatedInput: { command: "echo hello > greeting.txt" } });
    expect(await moved).toEqual({ behavior: "deny", message: "Denied by the user" });
    expect(session.idle()).toBe(true);
  });

  test("interrupt while idle does not touch the query", async () => {
    const { session, query } = await started();
    await session.interrupt();
    expect(query.calls).not.toContain("interrupt");
  });
});

describe("idle", () => {
  const tasks = (list: Array<{ task_id: string; ambient?: boolean }>) =>
    ({
      type: "system",
      subtype: "background_tasks_changed",
      tasks: list.map((task) => ({ task_type: "local_bash", description: "watch CI", ...task })),
      uuid: "u",
      session_id: "sess-1",
    }) as never;

  test("idle between turns, not during one", async () => {
    const { session, query } = await started();
    expect(session.idle()).toBe(true);
    session.send("hi");
    await query.waitForSent(1);
    expect(session.idle()).toBe(false);
    query.push(sdk.init("sess-1"), sdk.text("m1", "hello"), sdk.result());
    await flush();
    expect(session.idle()).toBe(true);
  });

  test("not idle while a background task would wake the agent; ambient watchers do not count", async () => {
    const { session, query } = await started();
    query.push(tasks([{ task_id: "b1" }]));
    await flush();
    expect(session.idle()).toBe(false);
    query.push(tasks([{ task_id: "w1", ambient: true }]));
    await flush();
    expect(session.idle()).toBe(true);
  });
});

describe("configure and stop", () => {
  test("changes model, permission mode and effort live", async () => {
    const { session, query } = await started();
    await session.configure({ model: "claude-opus-4-8", permissionMode: "accept_edits", effort: "low" });
    expect(query.calls).toEqual(["setModel:claude-opus-4-8", "setPermissionMode:acceptEdits", 'applyFlagSettings:{"effortLevel":"low"}']);
  });

  test("switching to auto where it is unavailable falls back to asking, and says so", async () => {
    const { session, query, host } = await started();
    query.permissionModeErrors.auto = new Error("Cannot set permission mode to auto: auto mode unavailable for this model");
    await session.configure({ permissionMode: "auto" });
    expect(query.calls).toEqual(["setPermissionMode:default"]);
    expect(host.events).toEqual([
      { type: "permission_mode_effective", mode: "default" },
      {
        type: "notice",
        level: "warning",
        message:
          'Auto mode is unavailable for this chat (Cannot set permission mode to auto: auto mode unavailable for this model). Claude asks before acting instead, as in "Ask for permissions"',
      },
    ]);
    // Other modes' failures still fail the change.
    query.permissionModeErrors.plan = new Error("no");
    await expect(session.configure({ permissionMode: "plan" })).rejects.toMatchObject({ code: ErrorCode.INTERNAL });
  });

  test("the mode Claude Code runs in is reported when it changes, and after each mode asked", async () => {
    const { session, query, host } = await started({ permission_mode: "auto", model: "claude-haiku-4-5-20251001" });
    // As asked at start: nothing to say.
    expect(host.events).toEqual([]);
    const modes = () => host.events.filter((event) => event.type === "permission_mode_effective");
    // Each turn's init and the status messages carry the mode.
    query.push({ type: "system", subtype: "status", status: null, permissionMode: "default", uuid: "u1", session_id: "sess-1" } as never);
    await flush();
    expect(modes()).toEqual([{ type: "permission_mode_effective", mode: "default" }]);
    query.push({ type: "system", subtype: "init", session_id: "sess-1", model: "claude-haiku-4-5-20251001", permissionMode: "default" } as never);
    await flush();
    expect(modes()).toHaveLength(1);
    // A model change asks for auto again: refused for this model, the chat still asks.
    query.permissionModeErrors.auto = new Error("auto mode unavailable for this model");
    await session.configure({ model: "claude-haiku-4-5" });
    expect(query.calls).toEqual(["setModel:claude-haiku-4-5", "setPermissionMode:default"]);
    expect(modes().at(-1)).toEqual({ type: "permission_mode_effective", mode: "default" });
    // Choosing auto again on such a model keeps the fallback.
    await session.configure({ permissionMode: "auto" });
    expect(modes().at(-1)).toEqual({ type: "permission_mode_effective", mode: "default" });
    // A model with auto mode takes it back.
    delete query.permissionModeErrors.auto;
    await session.configure({ model: "claude-opus-4-8" });
    expect(query.calls.slice(-2)).toEqual(["setModel:claude-opus-4-8", "setPermissionMode:auto"]);
    expect(modes().at(-1)).toEqual({ type: "permission_mode_effective", mode: "auto" });
    // A chat that asked for another mode does not touch it on a model change.
    await session.configure({ permissionMode: "accept_edits" });
    await session.configure({ model: "claude-haiku-4-5" });
    expect(query.calls.slice(-2)).toEqual(["setPermissionMode:acceptEdits", "setModel:claude-haiku-4-5"]);
    expect(modes().at(-1)).toEqual({ type: "permission_mode_effective", mode: "accept_edits" });
  });

  test("an exploration keeps plan mode when asked to change it", async () => {
    const { session, query } = await started({ read_only: true });
    await session.configure({ permissionMode: "bypass_permissions" });
    expect(query.calls).toEqual([]);
  });

  test("a design chat refuses another permission mode, before changing anything", async () => {
    const { session, query, host } = await started({ cwd: DESIGN, design_dir: DESIGN, permission_mode: "accept_edits" });
    await expect(session.configure({ model: "claude-haiku-4-5", permissionMode: "bypass_permissions" })).rejects.toMatchObject({
      code: ErrorCode.CONFLICT,
      message: expect.stringContaining("design chat"),
    });
    expect(query.calls).toEqual([]);
    // Its own mode (auto, or accept_edits for an older design chat) is left alone; a model change
    // asks for auto again, as in any chat in auto mode.
    await session.configure({ permissionMode: "accept_edits", effort: "low" });
    await session.configure({ permissionMode: "auto" });
    expect(query.calls).toEqual(['applyFlagSettings:{"effortLevel":"low"}']);
    await session.configure({ model: "claude-haiku-4-5" });
    expect(query.calls.slice(-2)).toEqual(["setModel:claude-haiku-4-5", "setPermissionMode:auto"]);
    expect(host.events.filter((e) => e.type === "notice")).toEqual([]);
  });

  test("a design chat without auto mode accepts edits instead, and says so", async () => {
    // At start: Claude Code took default; the session switches to acceptEdits before any turn.
    const host = new FakeHost();
    let made: FakeQuery | undefined;
    const session = new ClaudeSession({ ...BASE, cwd: DESIGN, design_dir: DESIGN, model: "claude-haiku-4-5-20251001" }, host, {
      query: ({ prompt, options }) => {
        made = new FakeQuery(prompt, options);
        made.initResult.current_permission_mode = "default";
        return made;
      },
      resolveExecutable: () => undefined,
      baseEnv: {},
      initTimeoutMs: 200,
    });
    await session.start();
    expect(made?.calls).toEqual(["setPermissionMode:acceptEdits"]);
    expect(host.events).toEqual([
      { type: "permission_mode_effective", mode: "default" },
      { type: "permission_mode_effective", mode: "accept_edits" },
      {
        type: "notice",
        level: "warning",
        message:
          "Auto mode is unavailable for this chat with claude-haiku-4-5-20251001 (its model, plan or settings turn it off). Claude writes the screens without asking, and asks before other tools (MCP servers, reading outside the repos and the notes)",
      },
    ]);
    // A model change that auto mode refuses: acceptEdits again, never default.
    made!.permissionModeErrors.auto = new Error("auto mode unavailable for this model");
    await session.configure({ model: "claude-haiku-4-5" });
    expect(made?.calls.slice(-2)).toEqual(["setModel:claude-haiku-4-5", "setPermissionMode:acceptEdits"]);
    await session.stop();
  });

  test("fast mode is a flag setting, at start and live", async () => {
    const { session, query } = await started({ fast: true });
    expect(query.options.settings).toEqual({ fastMode: true, permissions: { allow: CHAUFFEUR_TOOL_RULES } });
    await session.configure({ fast: false });
    expect(query.calls).toEqual(['applyFlagSettings:{"fastMode":false}']);
    const plain = await started();
    expect((plain.query.options.settings as Settings | undefined)?.fastMode).toBeUndefined();
  });

  test("tells the user when effort cannot change live", async () => {
    const { session, query, host } = await started();
    query.applyFlagSettingsError = new Error("unsupported");
    await session.configure({ effort: "max" });
    expect(host.events).toEqual([
      { type: "notice", level: "warning", message: 'Effort "max" will apply when the chat restarts (unsupported)' },
    ]);
    await expect(session.configure({ effort: "huge" })).rejects.toMatchObject({ code: ErrorCode.INVALID_PARAMS });
  });

  test("an SDK iterator which does not close refuses stop confirmation until it actually ends", async () => {
    const { session, query } = await started();
    const close = query.close.bind(query);
    query.close = () => {};
    await expect(session.stop()).rejects.toMatchObject({ code: ErrorCode.UNAVAILABLE });
    expect(() => session.send("no new work")).toThrow();
    close();
    await expect(session.stop()).resolves.toBeUndefined();
  });

  test("stop cancels pending requests, ends the running turn and closes the query", async () => {
    const { session, query, host } = await started();
    session.send("go");
    await query.waitForSent(1);
    const decision = canUseTool(query)("Bash", { command: "ls" }, toolOptions());
    await flush();
    await session.stop();
    expect(await decision).toMatchObject({ behavior: "deny" });
    expect(query.calls).toContain("close");
    expect(query.options.abortController?.signal.aborted).toBe(true);
    expect(host.types()).toEqual(["turn_started", "permission_request", "request_resolved", "turn_ended"]);
    expect(host.events[3]).toMatchObject({ outcome: "interrupted" });
    expect(host.endedCount).toBe(0);
    expect(() => session.send("more")).toThrow(RpcError);
    await session.stop(); // idempotent
  });
});

describe("hooks", () => {
  const base = { session_id: "s", transcript_path: "/t", cwd: "/w" };
  const signal = new AbortController().signal;

  function hook(query: FakeQuery, event: "PreToolUse" | "WorktreeCreate" | "WorktreeRemove"): HookCallback {
    const callback = query.options.hooks?.[event]?.[0]?.hooks[0];
    if (!callback) throw new Error(`no ${event} hook`);
    return callback;
  }

  test("PreToolUse on Bash denies git worktree add and lets other commands through", async () => {
    const { query } = await started({ permission_mode: "bypass_permissions" });
    expect(query.options.hooks?.PreToolUse?.[0]?.matcher).toBe("Bash");
    const guard = hook(query, "PreToolUse");
    const input = (command: string) =>
      ({ ...base, hook_event_name: "PreToolUse", tool_name: "Bash", tool_input: { command }, tool_use_id: "t" }) as HookInput;
    const denied = await guard(input("cd repo && git worktree add ../x"), "t", { signal });
    expect(denied).toMatchObject({
      hookSpecificOutput: { hookEventName: "PreToolUse", permissionDecision: "deny" },
    });
    expect(JSON.stringify(denied)).toContain("chauffeur worktree add --repo <repo> [--name <name>]");
    expect(await guard(input("git worktree list"), "t", { signal })).toEqual({});
  });

  test("in an exploration, PreToolUse on Bash also denies git and GitHub writes", async () => {
    const input = (command: string) =>
      ({ ...base, hook_event_name: "PreToolUse", tool_name: "Bash", tool_input: { command }, tool_use_id: "t" }) as HookInput;
    const run = async (query: FakeQuery, command: string) => {
      const outputs = await Promise.all((query.options.hooks?.PreToolUse?.[0]?.hooks ?? []).map((h) => h(input(command), "t", { signal })));
      return outputs.find((o) => JSON.stringify(o).includes('"deny"'));
    };
    const exploration = (await started({ read_only: true })).query;
    expect(JSON.stringify(await run(exploration, "git commit -am wip"))).toContain("exploration, read-only");
    expect(await run(exploration, "gh pr create --fill")).toBeDefined();
    expect(await run(exploration, "git log --oneline")).toBeUndefined();
    const workspace = (await started()).query;
    expect(await run(workspace, "git commit -am wip")).toBeUndefined();
  });

  test("in an exploration, Write and Edit go into the plans folder only, NotebookEdit nowhere", async () => {
    const ws = mkdtempSync(join(tmpdir(), "plan-hook-"));
    try {
      const plans = join(ws, ".chauffeur", "plans");
      mkdirSync(plans, { recursive: true });
      mkdirSync(join(ws, "repo-a"));
      writeFileSync(join(ws, "repo-a", "README.md"), "readme\n");
      const { query } = await started({ read_only: true, cwd: ws, plans_dir: plans });
      const guard = query.options.hooks?.PreToolUse?.[1]?.hooks[0];
      if (!guard) throw new Error("no plan guard");
      const call = (tool_name: string, tool_input: unknown, cwd = ws) =>
        guard({ ...base, cwd, hook_event_name: "PreToolUse", tool_name, tool_input, tool_use_id: "t" } as HookInput, "t", { signal });
      expect(await call("Write", { file_path: join(plans, "glowing-otter.md"), content: "# Plan" })).toEqual({});
      expect(await call("Edit", { file_path: ".chauffeur/plans/glowing-otter.md", old_string: "a", new_string: "b" })).toEqual({});
      expect(await call("Write", { file_path: "glowing-otter.md", content: "x" }, plans)).toEqual({});
      for (const [tool, input] of [
        ["Write", { file_path: join(ws, "repo-a", "README.md"), content: "x" }],
        ["Edit", { file_path: "repo-a/README.md", old_string: "readme", new_string: "x" }],
        ["MultiEdit", { file_path: join(ws, "CLAUDE.md"), edits: [] }],
        ["Write", { file_path: join(homedir(), ".claude", "plans", "glowing-otter.md"), content: "x" }],
        ["Write", { file_path: `${plans}/../../repo-a/README.md`, content: "x" }],
        ["NotebookEdit", { notebook_path: join(plans, "a.ipynb"), new_source: "x" }],
      ] as const) {
        const denied = await call(tool, input);
        expect(denied).toMatchObject({ hookSpecificOutput: { hookEventName: "PreToolUse", permissionDecision: "deny" } });
        expect(JSON.stringify(denied)).toContain(`the only files you may write are your plans, in ${plans}/`);
      }
      expect(await call("Read", { file_path: join(ws, "repo-a", "README.md") })).toEqual({});
      expect(await call("Bash", { command: "ls" })).toEqual({});

      // Again in canUseTool, without asking the user; a plan write still asks as plan mode does.
      const { query: again, host } = await started({ read_only: true, cwd: ws, plans_dir: plans });
      const denied = await canUseTool(again)("Write", { file_path: join(ws, "repo-a", "README.md"), content: "x" }, toolOptions());
      expect(denied).toMatchObject({ behavior: "deny" });
      expect(host.types()).toEqual([]);
      void canUseTool(again)("Write", { file_path: join(plans, "other.md"), content: "x" }, toolOptions());
      await flush();
      expect(host.types()).toEqual(["permission_request"]);
    } finally {
      rmSync(ws, { recursive: true, force: true });
    }
  });

  test("an exploration without a plans folder refuses every file write", async () => {
    const { query } = await started({ read_only: true });
    const guard = query.options.hooks?.PreToolUse?.[1]?.hooks[0];
    if (!guard) throw new Error("no plan guard");
    const denied = await guard(
      { ...base, hook_event_name: "PreToolUse", tool_name: "Write", tool_input: { file_path: "/w/.chauffeur/plans/a.md" }, tool_use_id: "t" } as HookInput,
      "t",
      { signal },
    );
    expect(JSON.stringify(denied)).toContain("no plans folder");
  });

  test("in a design chat, PreToolUse on Bash denies git and GitHub writes as a design chat", async () => {
    const input = (command: string) =>
      ({ ...base, hook_event_name: "PreToolUse", tool_name: "Bash", tool_input: { command }, tool_use_id: "t" }) as HookInput;
    const { query } = await started({ cwd: DESIGN, design_dir: DESIGN });
    const run = async (command: string) => {
      const outputs = await Promise.all((query.options.hooks?.PreToolUse?.[0]?.hooks ?? []).map((h) => h(input(command), "t", { signal })));
      return outputs.find((o) => JSON.stringify(o).includes('"deny"'));
    };
    const denied = JSON.stringify(await run("git commit -am wip"));
    expect(denied).toContain("This is a Chauffeur design chat");
    expect(denied).not.toContain("exploration");
    expect(await run("gh pr create --fill")).toBeDefined();
    expect(JSON.stringify(await run("git worktree add ../x"))).toContain("chauffeur worktree add");
    expect(await run("git log --oneline")).toBeUndefined();
  });

  test("in a design chat, Write and Edit go into the design folder only, with the design chat's denial", async () => {
    const ws = mkdtempSync(join(tmpdir(), "design-hook-"));
    try {
      const notes = join(ws, ".chauffeur");
      const design = join(notes, "designs", "chat-1");
      mkdirSync(design, { recursive: true });
      mkdirSync(join(notes, "plans"));
      mkdirSync(join(ws, "repo-a"));
      writeFileSync(join(ws, "repo-a", "README.md"), "readme\n");
      const params = { cwd: design, additional_directories: [join(ws, "repo-a")], plans_dir: join(notes, "plans"), design_dir: design };
      const { query } = await started(params);
      const guard = query.options.hooks?.PreToolUse?.[1]?.hooks[0];
      if (!guard) throw new Error("no write guard");
      const call = (tool_name: string, tool_input: unknown, cwd = design) =>
        guard({ ...base, cwd, hook_event_name: "PreToolUse", tool_name, tool_input, tool_use_id: "t" } as HookInput, "t", { signal });
      expect(await call("Write", { file_path: join(design, "checkout.html"), content: "<html>" })).toEqual({});
      expect(await call("Write", { file_path: "canvas.json", content: "{}" })).toEqual({});
      expect(await call("Edit", { file_path: "checkout.html", old_string: "a", new_string: "b" })).toEqual({});
      for (const [tool, input] of [
        ["Write", { file_path: join(ws, "repo-a", "README.md"), content: "x" }],
        ["Edit", { file_path: "../../../repo-a/README.md", old_string: "readme", new_string: "x" }],
        ["Write", { file_path: join(notes, "tickets.md"), content: "x" }],
        ["Write", { file_path: join(notes, "plans", "a.md"), content: "x" }],
        ["MultiEdit", { file_path: join(ws, "CLAUDE.md"), edits: [] }],
        ["NotebookEdit", { notebook_path: join(design, "a.ipynb"), new_source: "x" }],
      ] as const) {
        const denied = await call(tool, input);
        expect(denied).toMatchObject({ hookSpecificOutput: { hookEventName: "PreToolUse", permissionDecision: "deny" } });
        const reason = JSON.stringify(denied);
        expect(reason).toContain("This is a Chauffeur design chat: you design screens, you never change code.");
        expect(reason).toContain(`are your screens and canvas.json, in ${design}/. Refused: `);
        expect(reason).toContain("the user sends it to a coding chat that implements it");
      }
      expect(await call("Read", { file_path: join(ws, "repo-a", "README.md") })).toEqual({});

      // Again in canUseTool, without asking the user; a screen write that reaches it is asked.
      const { query: again, host } = await started(params);
      const denied = await canUseTool(again)("Write", { file_path: join(ws, "repo-a", "README.md"), content: "x" }, toolOptions());
      expect(denied).toMatchObject({ behavior: "deny", message: expect.stringContaining("design chat") });
      expect(host.types()).toEqual([]);
      void canUseTool(again)("Write", { file_path: join(design, "a.html"), content: "x" }, toolOptions());
      await flush();
      expect(host.types()).toEqual(["permission_request"]);
    } finally {
      rmSync(ws, { recursive: true, force: true });
    }
  });

  test("WorktreeCreate and WorktreeRemove go to the daemon", async () => {
    const { query, host } = await started();
    const created = await hook(query, "WorktreeCreate")({ ...base, hook_event_name: "WorktreeCreate", name: "fix-login" }, undefined, { signal });
    expect(created).toEqual({
      hookSpecificOutput: { hookEventName: "WorktreeCreate", worktreePath: "/workspaces/w/.worktrees/fix-login" },
    });
    expect(host.worktrees).toEqual(["fix-login"]);
    await hook(query, "WorktreeRemove")({ ...base, hook_event_name: "WorktreeRemove", worktree_path: "/wt/a" }, undefined, { signal });
    expect(host.removed).toEqual(["/wt/a"]);
    expect(query.options.hooks?.WorktreeCreate?.[0]?.timeout).toBe(600);
  });

  test("a failed daemon request fails the hook", async () => {
    const { query, host } = await started();
    host.worktreeError = new RpcError(ErrorCode.CONFLICT, "workspace is archiving");
    const run = hook(query, "WorktreeCreate")({ ...base, hook_event_name: "WorktreeCreate", name: "x" }, undefined, { signal });
    await expect(run).rejects.toThrow("Chauffeur could not create the worktree: workspace is archiving");
  });
});

describe("repo tools", () => {
  test("a workspace's chat and an exploration's get Chauffeur's repo tools, allowed by the chat itself", async () => {
    for (const params of [{}, { read_only: true, plans_dir: PLANS }]) {
      const { query, host } = await started(params);
      expect(query.options.mcpServers?.chauffeur).toMatchObject({ type: "sdk", name: "chauffeur" });
      expect((query.options.settings as Settings | undefined)?.permissions?.allow).toEqual(CHAUFFEUR_TOOL_RULES);
      // Whatever Claude Code would ask, the chat allows them: add_repo asks the user itself.
      const input = { repo: "analytics", reason: "r" };
      expect(await canUseTool(query)("mcp__chauffeur__add_repo", input, toolOptions())).toEqual({ behavior: "allow", updatedInput: input });
      expect(host.events.filter((e) => e.type === "permission_request")).toEqual([]);
    }
  });

  test("a design chat has none, but a browser without a window, for its screens' files", async () => {
    const { query } = await started({ cwd: DESIGN, plans_dir: PLANS, design_dir: DESIGN });
    expect(Object.keys(query.options.mcpServers ?? {})).toEqual(["headless-browser"]);
    expect(query.options.mcpServers?.["headless-browser"]).toEqual({
      type: "stdio",
      command: "npx",
      args: [
        "-y",
        "@playwright/mcp@0.0.83",
        "--headless",
        "--isolated",
        "--allow-unrestricted-file-access",
        "--output-dir",
        join(tmpdir(), "chauffeur-design-browser", "chat-1"),
      ],
    });
    expect((query.options.settings as Settings | undefined)?.permissions?.allow).toBeUndefined();
  });

  test("PreToolUse on Bash denies the agent's own chauffeur add-repo", async () => {
    const { query } = await started({ permission_mode: "bypass_permissions" });
    const input = (command: string) =>
      ({ session_id: "s", transcript_path: "/t", cwd: "/w", hook_event_name: "PreToolUse", tool_name: "Bash", tool_input: { command }, tool_use_id: "t" }) as HookInput;
    const run = async (command: string) => {
      const signal = new AbortController().signal;
      const outputs = await Promise.all((query.options.hooks?.PreToolUse?.[0]?.hooks ?? []).map((h) => h(input(command), "t", { signal })));
      return outputs.find((o) => JSON.stringify(o).includes('"deny"'));
    };
    expect(JSON.stringify(await run("chauffeur add-repo w analytics"))).toContain("add_repo tool");
    expect(await run("chauffeur repos")).toBeUndefined();
  });
});
