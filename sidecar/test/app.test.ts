import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import type { Agent, AgentSession, SessionChanges, SessionHost } from "../src/agents/agent.ts";
import { createSidecar } from "../src/app.ts";
import { silentLogger } from "../src/log.ts";
import type { AgentBriefParams, AgentForkParams, AgentModel, AgentSubagentMessagesParams, AgentSubagentMessagesResult, ContextDetail, ContextUsage, RequestResponse, SessionStartParams, SessionStartResult, SlashCommand } from "../src/protocol.ts";
import { AsyncQueue } from "../src/util/async-queue.ts";
import { CLAUDE_AGENT_SDK_VERSION, SIDECAR_VERSION } from "../src/version.ts";
import { flush } from "./support/flush.ts";

class FakeSession implements AgentSession {
  readonly calls: string[] = [];
  constructor(
    readonly params: SessionStartParams,
    readonly host: SessionHost,
  ) {}
  async start(): Promise<SessionStartResult> {
    return { agent_session_id: this.params.resume_session_id ?? null };
  }
  send(text: string): void {
    this.calls.push(`send:${text}`);
  }
  async interrupt(): Promise<void> {
    this.calls.push("interrupt");
  }
  respond(requestId: string, response: RequestResponse): void {
    this.calls.push(`respond:${requestId}:${JSON.stringify(response)}`);
  }
  async configure(changes: SessionChanges): Promise<void> {
    this.calls.push(`configure:${JSON.stringify(changes)}`);
  }
  async stopTask(taskId: string): Promise<void> {
    this.calls.push(`stopTask:${taskId}`);
  }
  /** Codex sessions have none. */
  contextUsage?: (detail: ContextDetail) => Promise<ContextUsage> = async (detail) => {
    this.calls.push(`contextUsage:${detail}`);
    return { model: "m", used_tokens: 1, max_tokens: 10, percent: 10, over_limit: false, categories: [], compaction: null };
  };
  busy = false;
  idle(): boolean {
    return !this.busy;
  }
  async stop(): Promise<void> {
    this.calls.push("stop");
  }
}

class FakeAgent implements Agent {
  readonly kind = "claude" as const;
  readonly versions = { "claude-agent-sdk": CLAUDE_AGENT_SDK_VERSION };
  readonly sessions: FakeSession[] = [];
  createSession(params: SessionStartParams, host: SessionHost): AgentSession {
    const session = new FakeSession(params, host);
    this.sessions.push(session);
    return session;
  }
  async listModels(): Promise<AgentModel[]> {
    return [{ id: "opus", resolved: "claude-opus-5-5", name: "Opus 5.5", description: "", efforts: ["low", "high"], default_effort: null, supports_fast: true, is_default: true }];
  }
  readonly subagentReads: AgentSubagentMessagesParams[] = [];
  async subagentMessages(params: AgentSubagentMessagesParams): Promise<AgentSubagentMessagesResult> {
    this.subagentReads.push(params);
    if (params.agent_id !== "a1") return { agent_id: params.agent_id ?? null, found: false, events: [] };
    return { agent_id: "a1", found: true, events: [{ type: "assistant_text", message_id: "m:0", text: "Hi", parent_tool_use_id: null }] };
  }
  readonly forks: AgentForkParams[] = [];
  async fork(params: AgentForkParams): Promise<string> {
    this.forks.push(params);
    return `${params.session_id}-fork`;
  }
  readonly briefs: AgentBriefParams[] = [];
  async brief(params: AgentBriefParams): Promise<string> {
    this.briefs.push(params);
    return "## Need\nA screen.";
  }
  readonly listed: Array<[string, string[]]> = [];
  async listCommands(cwd: string, directories: string[]): Promise<SlashCommand[]> {
    this.listed.push([cwd, directories]);
    return [{ name: "architect", description: "Design a feature", argument_hint: "<feature>", builtin: false }];
  }
}

const START = {
  chat_id: "c1",
  workspace_id: "w1",
  agent: "claude",
  cwd: "/w",
  additional_directories: [],
  fast: false,
  prompt_attachments: [],
  permission_mode: "bypass_permissions",
  read_only: false,
  prompt_suggestions: false,
  env: {},
};

function sidecar() {
  const agent = new FakeAgent();
  const lines: string[] = [];
  const input = new AsyncQueue<string>();
  const app = createSidecar({ agents: [agent], write: (line) => lines.push(line), log: silentLogger });
  const serving = app.serve(input);
  let id = 0;
  const messages = () => lines.map((line) => JSON.parse(line) as Record<string, unknown>);
  const call = async (method: string, params: unknown) => {
    const requestId = ++id;
    input.push(`${JSON.stringify({ jsonrpc: "2.0", id: requestId, method, params })}\n`);
    await flush();
    return messages().find((m) => m.id === requestId && !("method" in m));
  };
  return { agent, app, input, lines, messages, call, serving };
}

describe("the sidecar over JSON-RPC", () => {
  test("answers ping with its version and the agents' versions", async () => {
    const { call } = sidecar();
    expect(await call("sidecar.ping", {})).toEqual({
      jsonrpc: "2.0",
      id: 1,
      result: { sidecar_version: SIDECAR_VERSION, agents: { "claude-agent-sdk": CLAUDE_AGENT_SDK_VERSION } },
    });
  });

  test("lists an agent's models", async () => {
    const { call } = sidecar();
    const response = await call("agent.models", { agent: "claude" });
    expect((response?.result as { models: AgentModel[] }).models.map((m) => m.name)).toEqual(["Opus 5.5"]);
    expect((await call("agent.models", { agent: "codex" }))?.error).toMatchObject({ code: 503 });
    expect((await call("agent.models", { agent: "gemini" }))?.error).toMatchObject({ code: -32602 });
  });

  test("reads a subagent's conversation without a session", async () => {
    const { agent, call } = sidecar();
    const params = { agent: "claude", session_id: "s1", cwd: "/w", agent_id: "a1", prompt: null, result: null };
    expect((await call("agent.subagent_messages", params))?.result).toEqual({
      agent_id: "a1",
      found: true,
      events: [{ type: "assistant_text", message_id: "m:0", text: "Hi", parent_tool_use_id: null }],
    });
    // Absent and null hints are left out.
    expect(agent.subagentReads[0]).toEqual({ agent: "claude", session_id: "s1", cwd: "/w", agent_id: "a1" });
    expect((await call("agent.subagent_messages", { ...params, agent_id: "gone" }))?.result).toEqual({ agent_id: "gone", found: false, events: [] });
    expect((await call("agent.subagent_messages", { agent: "claude", cwd: "/w" }))?.error).toMatchObject({ code: -32602 });
    expect((await call("agent.subagent_messages", { ...params, agent: "codex" }))?.error).toMatchObject({ code: 503 });
  });

  test("forks an agent session without a session running", async () => {
    const { agent, call } = sidecar();
    const params = { agent: "claude" as const, session_id: "s1", cwd: "/w", env: { CODEX_HOME: "/h" } };
    expect((await call("agent.fork", params))?.result).toEqual({ session_id: "s1-fork" });
    expect(agent.forks).toEqual([params]);
    // The environment is optional; a session id is not.
    expect((await call("agent.fork", { agent: "claude", session_id: "s2", cwd: "/w" }))?.result).toEqual({ session_id: "s2-fork" });
    // Up to a turn, when asked; null is the whole session.
    await call("agent.fork", { ...params, up_to: "u1" });
    await call("agent.fork", { ...params, up_to: null });
    expect(agent.forks.slice(2)).toEqual([{ ...params, up_to: "u1" }, params]);
    expect((await call("agent.fork", { agent: "claude", cwd: "/w" }))?.error).toMatchObject({ code: -32602 });
    expect((await call("agent.fork", { ...params, agent: "codex" }))?.error).toMatchObject({ code: 503 });
  });

  test("lists the commands an agent offers in a folder", async () => {
    const { agent, call } = sidecar();
    const response = await call("agent.commands", { agent: "claude", cwd: "/w", additional_directories: ["/w/alpha"] });
    expect((response?.result as { commands: SlashCommand[] }).commands.map((c) => c.name)).toEqual(["architect"]);
    expect(agent.listed).toEqual([["/w", ["/w/alpha"]]]);
    expect((await call("agent.commands", { agent: "claude", cwd: "/w", additional_directories: "/w" }))?.error).toMatchObject({ code: -32602 });
    expect((await call("agent.commands", { agent: "codex", cwd: "/w" }))?.error).toMatchObject({ code: 503 });
  });

  test("refuses codex with a clear error", async () => {
    const { call } = sidecar();
    const response = await call("session.start", { ...START, agent: "codex" });
    expect(response?.error).toEqual({
      code: 503,
      message: 'agent "codex" is not supported by this sidecar yet (available: claude)',
    });
  });

  test("validates params", async () => {
    const { call } = sidecar();
    expect((await call("session.start", { ...START, cwd: undefined }))?.error).toMatchObject({ code: -32602, message: "cwd must be a non-empty string" });
    expect((await call("session.start", { ...START, permission_mode: "yolo" }))?.error).toMatchObject({ code: -32602 });
    expect((await call("session.start", { ...START, agent: "gpt" }))?.error).toMatchObject({ code: -32602 });
    expect((await call("session.respond", { chat_id: "c1", request_id: "r", response: { type: "maybe" } }))?.error).toMatchObject({ code: -32602 });
    expect((await call("session.send", null))?.error).toMatchObject({ code: -32602 });
    expect((await call("session.nope", {}))?.error).toMatchObject({ code: -32601 });
  });

  test("accepts null optional fields, as serde writes them", async () => {
    const { call, agent } = sidecar();
    const response = await call("session.start", {
      ...START,
      model: null,
      effort: null,
      resume_session_id: "sess-9",
      append_system_prompt: null,
      plans_dir: null,
      notes_dir: null,
      prompt: null,
    });
    expect(response?.result).toEqual({ agent_session_id: "sess-9" });
    expect(agent.sessions[0]?.params).toEqual({ ...START, resume_session_id: "sess-9" } as SessionStartParams);
  });

  test("passes the plans and notes folders on", async () => {
    const { call, agent } = sidecar();
    const plans_dir = "/workspaces/ws-1/.chauffeur/plans";
    const notes_dir = "/workspaces/ws-1/.chauffeur";
    expect((await call("session.start", { ...START, read_only: true, plans_dir, notes_dir }))?.error).toBeUndefined();
    expect(agent.sessions[0]?.params.plans_dir).toBe(plans_dir);
    expect(agent.sessions[0]?.params.notes_dir).toBe(notes_dir);
    expect((await call("session.start", { ...START, chat_id: "c2", plans_dir: 7 }))?.error).toMatchObject({ code: -32602 });
    expect((await call("session.start", { ...START, chat_id: "c3", notes_dir: 7 }))?.error).toMatchObject({ code: -32602 });
  });

  test("passes the chat's always-allowed rules on, and checks them", async () => {
    const { call, agent } = sidecar();
    const always_allow = ["mcp__notes__write_note", "Bash(npm test:*)"];
    expect((await call("session.start", { ...START, always_allow }))?.error).toBeUndefined();
    expect(agent.sessions[0]?.params.always_allow).toEqual(always_allow);
    expect((await call("session.start", { ...START, chat_id: "c2" }))?.error).toBeUndefined();
    expect(agent.sessions[1]?.params.always_allow).toBeUndefined();
    expect((await call("session.start", { ...START, chat_id: "c3", always_allow: [1] }))?.error).toMatchObject({ code: -32602 });
  });

  test("passes on whether the agent gets the delegate tool, and checks it", async () => {
    const { call, agent } = sidecar();
    expect((await call("session.start", { ...START, delegate: true }))?.error).toBeUndefined();
    expect(agent.sessions[0]?.params.delegate).toBe(true);
    expect((await call("session.start", { ...START, chat_id: "c2" }))?.error).toBeUndefined();
    expect(agent.sessions[1]?.params.delegate).toBeUndefined();
    expect((await call("session.start", { ...START, chat_id: "c3", delegate: "yes" }))?.error).toMatchObject({ code: -32602 });
  });

  test("passes the design folder on", async () => {
    const { call, agent } = sidecar();
    const design_dir = "/workspaces/ws-1/.chauffeur/designs/c1";
    expect((await call("session.start", { ...START, cwd: design_dir, design_dir }))?.error).toBeUndefined();
    expect(agent.sessions[0]?.params.design_dir).toBe(design_dir);
    expect((await call("session.start", { ...START, chat_id: "c2", design_dir: null }))?.error).toBeUndefined();
    expect(agent.sessions[1]?.params.design_dir).toBeUndefined();
    expect((await call("session.start", { ...START, chat_id: "c3", design_dir: 7 }))?.error).toMatchObject({ code: -32602 });
  });

  test("writes a design brief, from Claude, with the model and effort given", async () => {
    const { call, agent } = sidecar();
    const response = await call("agent.brief", { conversation: "user: hi", title: "Cart", model: "opus", effort: null });
    expect(response?.result).toEqual({ brief: "## Need\nA screen." });
    expect(agent.briefs).toEqual([{ conversation: "user: hi", title: "Cart", model: "opus" }]);
    expect((await call("agent.brief", { title: "Cart" }))?.error).toMatchObject({ code: -32602 });
    expect((await call("agent.brief", { conversation: "x", model: 3 }))?.error).toMatchObject({ code: -32602 });
  });

  test("passes Jev's key and threshold on, and checks them", async () => {
    const { call, agent } = sidecar();
    const jev = { api_key: "ts_key", threshold_percent: 60 };
    expect((await call("session.start", { ...START, jev }))?.error).toBeUndefined();
    expect(agent.sessions[0]?.params.jev).toEqual(jev);
    expect((await call("session.start", { ...START, chat_id: "c2", jev: { api_key: "k", threshold_percent: 0 } }))?.error).toMatchObject({
      code: -32602,
    });
    expect((await call("session.start", { ...START, chat_id: "c3", jev: { threshold_percent: 50 } }))?.error).toMatchObject({ code: -32602 });
  });

  test("routes session calls to the chat's session", async () => {
    const { call, agent } = sidecar();
    await call("session.start", START);
    expect((await call("session.send", { chat_id: "c1", text: "hi" }))?.result).toEqual({});
    await call("session.respond", { chat_id: "c1", request_id: "r1", response: { type: "allow", always: true } });
    await call("session.respond", { chat_id: "c1", request_id: "r2", response: { type: "deny", message: null } });
    await call("session.respond", { chat_id: "c1", request_id: "r3", response: { type: "elicitation", action: "accept", content: {}, always: true } });
    await call("session.respond", { chat_id: "c1", request_id: "r4", response: { type: "elicitation", action: "cancel" } });
    expect((await call("session.respond", { chat_id: "c1", request_id: "r5", response: { type: "elicitation", action: "maybe" } }))?.error).toMatchObject({ code: -32602 });
    await call("session.configure", { chat_id: "c1", model: "opus", effort: null, permission_mode: "plan" });
    await call("session.interrupt", { chat_id: "c1" });
    expect((await call("session.stop_task", { chat_id: "c1", task_id: "t1" }))?.result).toEqual({});
    expect((await call("session.stop_task", { chat_id: "c1" }))?.error).toMatchObject({ code: -32602 });
    await call("session.stop", { chat_id: "c1" });
    expect(agent.sessions[0]?.calls).toEqual([
      "send:hi",
      'respond:r1:{"type":"allow","always":true}',
      'respond:r2:{"type":"deny","message":null}',
      'respond:r3:{"type":"elicitation","action":"accept","content":{},"always":true}',
      'respond:r4:{"type":"elicitation","action":"cancel","content":null,"always":false}',
      'configure:{"model":"opus","permissionMode":"plan"}',
      "interrupt",
      "stopTask:t1",
      "stop",
    ]);
    // Stopped: the chat is free again, and stopping twice is harmless.
    expect((await call("session.send", { chat_id: "c1", text: "hi" }))?.error).toMatchObject({ code: 404 });
    expect((await call("session.stop", { chat_id: "c1" }))?.result).toEqual({});
  });

  test("stop receipts name only exact tracked launches whose native sessions stopped", async () => {
    const { call, agent } = sidecar();
    const first = "00000000-0000-4000-8000-000000000001";
    const second = "00000000-0000-4000-8000-000000000002";
    await call("session.start", { ...START, launch_id: first });
    agent.sessions[0]!.stop = async () => { throw new Error("still alive"); };
    expect((await call("session.stop", { chat_id: "c1" }))?.error).toBeDefined();
    agent.sessions[0]!.busy = true;
    expect((await call("session.close_idle", { chat_id: "c1" }))?.result).toEqual({ closed: false });
    agent.sessions[0]!.stop = async () => {};
    expect((await call("session.stop", { chat_id: "c1" }))?.result).toEqual({ confirmed_launch_ids: [first] });
    await call("session.start", { ...START, launch_id: second });
    // A repeated ID cannot cause another native session, including after its original stop.
    expect((await call("session.start", { ...START, launch_id: first }))?.error).toMatchObject({ code: 409 });
    agent.sessions[1]!.busy = true;
    expect((await call("session.close_idle", { chat_id: "c1" }))?.result).toEqual({ closed: false });
    agent.sessions[1]!.busy = false;
    expect((await call("session.close_idle", { chat_id: "c1" }))?.result).toEqual({ closed: true, confirmed_launch_ids: [first, second] });
    expect((await call("session.stop", { chat_id: "unknown" }))?.result).toEqual({});
  });

  test("new sidecars and untracked sessions cannot invent old launch receipts", async () => {
    const old = sidecar();
    const fresh = sidecar();
    await old.call("session.start", { ...START, launch_id: "00000000-0000-4000-8000-000000000003" });
    expect((await fresh.call("session.stop", { chat_id: "c1" }))?.result).toEqual({});
    await fresh.call("session.start", START);
    expect((await fresh.call("session.stop", { chat_id: "c1" }))?.result).toEqual({});
    expect((await fresh.call("session.start", { ...START, launch_id: "bad" }))?.error).toMatchObject({ code: -32602 });
  });

  test("failed launch cleanup cannot emit a stop receipt until the retained session is stopped", async () => {
    const { call, agent } = sidecar();
    const create = agent.createSession.bind(agent);
    agent.createSession = (params, host) => {
      const session = create(params, host);
      session.start = async () => { throw new Error("start uncertain"); };
      session.stop = async () => { throw new Error("exit uncertain"); };
      return session;
    };
    const id = "00000000-0000-4000-8000-000000000004";
    await call("session.start", { ...START, launch_id: id });
    expect((await call("session.stop", { chat_id: "c1" }))?.error).toBeDefined();
    agent.sessions[0]!.stop = async () => {};
    expect((await call("session.stop", { chat_id: "c1" }))?.result).toEqual({ confirmed_launch_ids: [id] });
  });

  test("a failed stop retains the session and prevents replacement until a confirmed retry", async () => {
    const { call, agent } = sidecar();
    await call("session.start", START);
    const session = agent.sessions[0]!;
    session.stop = async () => { throw new Error("exit is unconfirmed"); };
    expect((await call("session.stop", { chat_id: "c1" }))?.error).toBeDefined();
    expect((await call("session.start", START))?.error).toMatchObject({ code: 409 });
    session.stop = async () => {};
    expect((await call("session.stop", { chat_id: "c1" }))?.result).toEqual({});
    expect((await call("session.start", START))?.error).toBeUndefined();
  });

  test("a failed start cannot free a chat while its cleanup is unconfirmed", async () => {
    const { call, agent } = sidecar();
    const create = agent.createSession.bind(agent);
    agent.createSession = (params, host) => {
      const session = create(params, host);
      session.start = async () => { throw new Error("initialize failed"); };
      session.stop = async () => { throw new Error("process exit is unknown"); };
      return session;
    };
    expect((await call("session.start", START))?.error).toBeDefined();
    expect((await call("session.start", START))?.error).toMatchObject({ code: 409 });
    agent.sessions[0]!.stop = async () => {};
    await call("session.stop", { chat_id: "c1" });
    agent.createSession = create;
    expect((await call("session.start", START))?.error).toBeUndefined();
  });

  test("concurrent stops wait for the same confirmation, even if the agent reports ended early", async () => {
    const { call, agent, input, messages } = sidecar();
    await call("session.start", START);
    const session = agent.sessions[0]!;
    let finish!: () => void;
    let stops = 0;
    session.stop = () => { stops++; session.host.ended(); return new Promise<void>((resolve) => { finish = resolve; }); };
    for (const id of [100, 101]) input.push(JSON.stringify({ jsonrpc: "2.0", id, method: "session.stop", params: { chat_id: "c1" } }) + "\n");
    await flush();
    expect(stops).toBe(1);
    expect(messages().find((m) => m.id === 100)).toBeUndefined();
    expect((await call("session.start", START))?.error).toMatchObject({ code: 409 });
    finish();
    await flush();
    expect(messages().find((m) => m.id === 100)?.result).toEqual({});
    expect(messages().find((m) => m.id === 101)?.result).toEqual({});
  });

  test("reads a session's context in the detail asked; refused by agents without live readings", async () => {
    const { call, agent } = sidecar();
    expect((await call("session.context_usage", { chat_id: "c1", detail: "full" }))?.error).toMatchObject({ code: 404 });
    await call("session.start", START);
    const read = await call("session.context_usage", { chat_id: "c1", detail: "full" });
    expect(read?.result).toMatchObject({ usage: { percent: 10, categories: [] } });
    expect((await call("session.context_usage", { chat_id: "c1" }))?.error).toBeUndefined();
    expect((await call("session.context_usage", { chat_id: "c1", detail: "all" }))?.error).toMatchObject({ code: -32602 });
    expect(agent.sessions[0]?.calls).toEqual(["contextUsage:full", "contextUsage:summary"]);
    if (agent.sessions[0]) agent.sessions[0].contextUsage = undefined;
    expect((await call("session.context_usage", { chat_id: "c1" }))?.error).toMatchObject({ code: 503 });
  });

  test("closes an idle session only: a busy one is kept, a gone one counts as closed", async () => {
    const { call, agent } = sidecar();
    await call("session.start", START);
    agent.sessions[0]!.busy = true;
    expect((await call("session.close_idle", { chat_id: "c1" }))?.result).toEqual({ closed: false });
    expect((await call("session.send", { chat_id: "c1", text: "still here" }))?.result).toEqual({});
    agent.sessions[0]!.busy = false;
    expect((await call("session.close_idle", { chat_id: "c1" }))?.result).toEqual({ closed: true });
    expect(agent.sessions[0]?.calls.at(-1)).toBe("stop");
    expect((await call("session.close_idle", { chat_id: "c1" }))?.result).toEqual({ closed: true });
    // The next message's session.start resumes the chat.
    expect((await call("session.start", { ...START, resume_session_id: "s" }))?.result).toEqual({ agent_session_id: "s" });
  });

  test("refuses a second session for a running chat, but not after it ended on its own", async () => {
    const { call, agent } = sidecar();
    await call("session.start", START);
    expect((await call("session.start", START))?.error).toMatchObject({ code: 409 });
    agent.sessions[0]?.host.ended();
    expect((await call("session.start", { ...START, resume_session_id: "s" }))?.result).toEqual({ agent_session_id: "s" });
    // The first session's late `ended` must not forget the new one.
    agent.sessions[0]?.host.ended();
    expect((await call("session.send", { chat_id: "c1", text: "still here" }))?.result).toEqual({});
  });

  test("writes session events as session.event notifications", async () => {
    const { call, agent, messages } = sidecar();
    await call("session.start", START);
    agent.sessions[0]?.host.emit({ type: "turn_started" });
    expect(messages().at(-1)).toEqual({
      jsonrpc: "2.0",
      method: "session.event",
      params: { chat_id: "c1", event: { type: "turn_started" } },
    });
  });

  test("streamed text reaches the daemon merged, before the chat's next event, a daemon call or its stop", async () => {
    const { call, agent, messages } = sidecar();
    await call("session.start", START);
    const host = agent.sessions[0]?.host as SessionHost;
    const events = () => messages().filter((m) => m.method === "session.event").map((m) => (m.params as { event: unknown }).event);
    host.emit({ type: "text_delta", message_id: "m:0", text: "Hel" });
    host.emit({ type: "text_delta", message_id: "m:0", text: "lo" });
    expect(events()).toEqual([]);
    host.emit({ type: "turn_started" });
    host.emit({ type: "thinking_delta", message_id: "m:1", text: "hm" });
    void host.listRepos();
    host.emit({ type: "text_delta", message_id: "m:2", text: "Bye" });
    expect(messages().slice(-2).map((m) => m.method)).toEqual(["session.event", "tool.list_repos"]);
    await call("session.stop", { chat_id: "c1" });
    expect(events()).toEqual([
      { type: "text_delta", message_id: "m:0", text: "Hello" },
      { type: "turn_started" },
      { type: "thinking_delta", message_id: "m:1", text: "hm" },
      { type: "text_delta", message_id: "m:2", text: "Bye" },
    ]);
  });

  test("asks the daemon to create and remove worktrees", async () => {
    const { call, agent, messages, input } = sidecar();
    await call("session.start", START);
    const host = agent.sessions[0]?.host as SessionHost;
    const created = host.createWorktree("fix-login");
    const request = messages().at(-1);
    expect(request).toMatchObject({ method: "hook.worktree_create", params: { chat_id: "c1", name: "fix-login" } });
    input.push(`${JSON.stringify({ jsonrpc: "2.0", id: request?.id, result: { path: "/w/.wt/fix-login" } })}\n`);
    expect(await created).toBe("/w/.wt/fix-login");

    const removed = host.removeWorktree("/w/.wt/fix-login");
    const removal = messages().at(-1);
    expect(removal).toMatchObject({ method: "hook.worktree_remove", params: { chat_id: "c1", path: "/w/.wt/fix-login" } });
    input.push(`${JSON.stringify({ jsonrpc: "2.0", id: removal?.id, error: { code: 409, message: "dirty worktree" } })}\n`);
    await expect(removed).rejects.toMatchObject({ code: 409, message: "dirty worktree" });

    const empty = host.createWorktree("x");
    input.push(`${JSON.stringify({ jsonrpc: "2.0", id: messages().at(-1)?.id, result: {} })}\n`);
    await expect(empty).rejects.toThrow("hook.worktree_create returned no path");
  });

  test("stops every session when the input ends", async () => {
    const { call, agent, input, serving } = sidecar();
    await call("session.start", START);
    await call("session.start", { ...START, chat_id: "c2" });
    input.end();
    await serving;
    expect(agent.sessions.map((s) => s.calls)).toEqual([["stop"], ["stop"]]);
  });
});

test("the SDK version reported matches the pinned dependency", () => {
  const pkg = JSON.parse(readFileSync(join(import.meta.dir, "..", "package.json"), "utf8")) as {
    version: string;
    dependencies: Record<string, string>;
  };
  expect(pkg.dependencies["@anthropic-ai/claude-agent-sdk"]).toBe(CLAUDE_AGENT_SDK_VERSION);
  expect(pkg.version).toBe(SIDECAR_VERSION);
  const sdk = JSON.parse(
    readFileSync(join(import.meta.dir, "..", "node_modules", "@anthropic-ai", "claude-agent-sdk", "package.json"), "utf8"),
  ) as { version: string };
  expect(sdk.version).toBe(CLAUDE_AGENT_SDK_VERSION);
});
