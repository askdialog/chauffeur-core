import { expect, test } from "bun:test";
import { VibeSession } from "../src/agents/vibe/session.ts";
import type { SessionStartParams } from "../src/protocol.ts";
import { FakeAcp, SESSION_ID } from "./support/acp-fakes.ts";
import { FakeHost, flush } from "./support/fakes.ts";

const BASE: SessionStartParams = {
  chat_id: "chat-1",
  workspace_id: "ws-1",
  agent: "vibe",
  cwd: "/w",
  additional_directories: ["/w/web"],
  fast: false,
  prompt_attachments: [],
  permission_mode: "default",
  read_only: false,
  prompt_suggestions: false,
  env: { CHAUFFEUR_WORKSPACE_ID: "ws-1" },
};

async function started(params: Partial<SessionStartParams> = {}) {
  const host = new FakeHost();
  let acp: FakeAcp | undefined;
  let ids = 0;
  const session = new VibeSession({ ...BASE, ...params }, host, {
    spawn: (options) => (acp = new FakeAcp(options)),
    resolveExecutable: () => "/home/me/.local/bin/vibe-acp",
    baseEnv: { PATH: "/usr/bin" },
    initTimeoutMs: 500,
    newRequestId: () => `req-${++ids}`,
    answerWithinMs: 50,
  });
  const result = await session.start();
  return { session, host, result, acp: acp as FakeAcp };
}

test("a session opens in Vibe's ask mode, with the repos as directories and Chauffeur's tools over HTTP", async () => {
  const { session, result, acp, host } = await started({ model: "mistral-large-4-0", effort: "low", delegate: true });
  expect(result).toEqual({ agent_session_id: SESSION_ID, native_pid: 4242 });
  const [opened] = acp.sent("session/new") as { cwd: string; additionalDirectories: string[]; mcpServers: { type: string; name: string; url: string }[] }[];
  expect(opened?.cwd).toBe("/w");
  expect(opened?.additionalDirectories).toEqual(["/w/web"]);
  expect(opened?.mcpServers.map((server) => [server.type, server.name])).toEqual([["http", "chauffeur"]]);
  expect(opened?.mcpServers[0]?.url).toMatch(/^http:\/\/127\.0\.0\.1:\d+\/mcp$/);
  expect(acp.sent("session/set_mode")).toEqual([{ sessionId: SESSION_ID, modeId: "ask" }]);
  expect(acp.sent("session/set_config_option")).toEqual([
    { sessionId: SESSION_ID, configId: "model", value: "mistral-large-4-0" },
    { sessionId: SESSION_ID, configId: "thinking", value: "low" },
  ]);
  expect(host.events[0]).toEqual({ type: "session_started", agent_session_id: SESSION_ID, model: "mistral-large-4-0" });
  await session.stop();
});

test("a model the account does not offer is said, and the chat runs on Vibe's default", async () => {
  const { session, acp, host } = await started({ model: "mistral-medium-3.5" });
  expect(acp.sent("session/set_config_option")).toEqual([]);
  expect(host.events.find((event) => event.type === "notice")).toMatchObject({ level: "warning" });
  expect(host.events[0]).toMatchObject({ type: "session_started", model: "mistral-medium-latest" });
  await session.stop();
});

test("Chauffeur's instructions go before the first message of a new session only", async () => {
  const { session, acp } = await started({ append_system_prompt: "Work in Chauffeur.", prompt: "Hello" });
  await flush();
  const [first] = acp.sent("session/prompt") as { prompt: { type: string; text: string }[] }[];
  expect(first?.prompt.map((block) => block.text)).toEqual(["<chauffeur-instructions>\nWork in Chauffeur.\n</chauffeur-instructions>", "Hello"]);
  acp.endPrompt();
  await flush();
  session.send("Again");
  await flush();
  const second = (acp.sent("session/prompt") as { prompt: { text: string }[] }[])[1];
  expect(second?.prompt.map((block) => block.text)).toEqual(["Again"]);
  await session.stop();
});

test("a resumed session loads silently: its replayed updates are not the chat's", async () => {
  const host = new FakeHost();
  let acp: FakeAcp | undefined;
  const session = new VibeSession({ ...BASE, resume_session_id: SESSION_ID, append_system_prompt: "Work in Chauffeur." }, host, {
    spawn: (options) => {
      acp = new FakeAcp(options);
      const request = acp.request.bind(acp);
      acp.request = async <T>(method: string, params: unknown) => {
        if (method === "session/load") acp?.update({ sessionUpdate: "agent_message_chunk", content: { type: "text", text: "old answer" } });
        return request<T>(method, params);
      };
      return acp;
    },
    resolveExecutable: () => "/bin/vibe-acp",
    baseEnv: {},
    initTimeoutMs: 500,
  });
  await session.start();
  expect(acp?.sent("session/load")).toHaveLength(1);
  expect(host.types()).toEqual(["session_started"]);
  session.send("Go on");
  await flush();
  const [prompt] = acp?.sent("session/prompt") as { prompt: { text: string }[] }[];
  expect(prompt?.prompt.map((block) => block.text)).toEqual(["Go on"]);
  await session.stop();
});

test("a turn streams text, ends with Vibe's answer, and messages sent meanwhile go as the next prompt", async () => {
  const { session, acp, host } = await started();
  session.send("First", [], "m1");
  await flush();
  session.send("Second", [], "m2");
  session.send("Third", [], "m3");
  acp.update({ sessionUpdate: "agent_message_chunk", content: { type: "text", text: "Hel" } });
  acp.update({ sessionUpdate: "agent_message_chunk", content: { type: "text", text: "lo" } });
  acp.update({ sessionUpdate: "usage_update", used: 5000, size: 100000 });
  acp.endPrompt({ stopReason: "end_turn", usage: { totalTokens: 7000 } });
  await flush();
  const text = host.events.find((event) => event.type === "assistant_text");
  expect(text).toMatchObject({ text: "Hello" });
  const ended = host.events.find((event) => event.type === "turn_ended");
  expect(ended).toMatchObject({ outcome: "success", tokens: 7000, context_tokens: 5000, error: null });
  expect(acp.running).toBe(1);
  const second = (acp.sent("session/prompt") as { prompt: { text: string }[] }[])[1];
  expect(second?.prompt.map((block) => block.text)).toEqual(["Second", "Third"]);
  expect(host.events.filter((event) => event.type === "user_message_delivered").map((event) => (event as { message_id: string }).message_id)).toEqual(["m1", "m2", "m3"]);
  acp.endPrompt();
  await session.stop();
});

test("interrupt cancels the prompt, and the turn ends interrupted", async () => {
  const { session, acp, host } = await started();
  session.send("Work");
  await flush();
  await session.interrupt();
  expect(acp.notifications).toEqual([{ method: "session/cancel", params: { sessionId: SESSION_ID } }]);
  acp.endPrompt({ stopReason: "cancelled" });
  await flush();
  expect(host.events.at(-1)).toMatchObject({ type: "turn_ended", outcome: "interrupted" });
  await session.stop();
});

test("a shell command is put to the user, and Always allow covers the same command afterwards", async () => {
  const { session, acp, host } = await started();
  session.send("Run the tests");
  await flush();
  acp.update({ sessionUpdate: "tool_call", toolCallId: "t1", title: "bash: bun test", kind: "execute", status: "pending", rawInput: { command: "bun test" } });
  const asking = acp.askPermission("t1");
  await flush();
  const request = host.events.find((event) => event.type === "permission_request");
  expect(request).toMatchObject({ tool_name: "Bash", input: { command: "bun test" } });
  session.respond("req-1", { type: "allow", always: true });
  expect(await asking).toEqual({ outcome: { outcome: "selected", optionId: "allow_once" } });
  expect(host.events.find((event) => event.type === "request_resolved")).toMatchObject({ rules: ["Bash(bun test)"] });
  acp.update({ sessionUpdate: "tool_call", toolCallId: "t2", title: "bash: bun test", kind: "execute", status: "pending", rawInput: { command: "bun test" } });
  expect(await acp.askPermission("t2")).toEqual({ outcome: { outcome: "selected", optionId: "allow_once" } });
  expect(host.events.filter((event) => event.type === "permission_request")).toHaveLength(1);
  acp.endPrompt();
  await session.stop();
});

test("a guard's refusal stops Vibe's step and prompts it again with why, in the same turn", async () => {
  const { session, acp, host } = await started();
  session.send("Make a worktree");
  await flush();
  acp.update({ sessionUpdate: "tool_call", toolCallId: "t1", title: "bash: git worktree add ../x", kind: "execute", status: "pending", rawInput: { command: "git worktree add ../x" } });
  expect(await acp.askPermission("t1")).toEqual({ outcome: { outcome: "selected", optionId: "reject_once" } });
  expect(acp.notifications).toEqual([{ method: "session/cancel", params: { sessionId: SESSION_ID } }]);
  acp.endPrompt({ stopReason: "cancelled" });
  await flush();
  const prompts = acp.sent("session/prompt") as { prompt: { text: string }[] }[];
  expect(prompts).toHaveLength(2);
  expect(prompts[1]?.prompt[0]?.text).toContain("`git worktree add` is disabled");
  expect(host.types().filter((type) => type === "turn_ended")).toHaveLength(0);
  expect(host.events.find((event) => event.type === "tool_result")).toMatchObject({ tool_use_id: "t1", is_error: true });
  acp.endPrompt();
  await flush();
  expect(host.events.filter((event) => event.type === "turn_ended")).toEqual([expect.objectContaining({ outcome: "success" })]);
  await session.stop();
});

test("Chauffeur's own tools run without asking", async () => {
  const { session, acp, host } = await started();
  session.send("List the repos");
  await flush();
  acp.update({ sessionUpdate: "tool_call", toolCallId: "t1", title: "mcp_chauffeur.list_repos", kind: "other", status: "pending" });
  expect(await acp.askPermission("t1")).toEqual({ outcome: { outcome: "selected", optionId: "allow_once" } });
  expect(host.types()).not.toContain("permission_request");
  acp.endPrompt();
  await session.stop();
});

test("a missing key at start marks Vibe signed out", async () => {
  const host = new FakeHost();
  const session = new VibeSession(BASE, host, {
    spawn: (options) => {
      const acp = new FakeAcp(options);
      acp.failures.set("session/new", new Error("Missing API key for mistral provider."));
      return acp;
    },
    resolveExecutable: () => "/bin/vibe-acp",
    baseEnv: {},
    initTimeoutMs: 500,
  });
  await expect(session.start()).rejects.toThrow("Missing API key");
  expect(host.events).toEqual([{ type: "account_lost", message: "Missing API key for mistral provider." }]);
});

test("Vibe dying on its own ends the turn and the session", async () => {
  const { session, acp, host } = await started();
  session.send("Work");
  await flush();
  acp.crash();
  await flush();
  expect(host.endedCount).toBe(1);
  expect(host.events.at(-1)).toMatchObject({ type: "turn_ended", outcome: "error" });
  expect(session.idle()).toBe(false);
});

test("an untrusted workspace folder is trusted, and the session opened again so that Vibe reads its AGENTS.md", async () => {
  const host = new FakeHost();
  let acp: FakeAcp | undefined;
  const session = new VibeSession(BASE, host, {
    spawn: (options) => {
      acp = new FakeAcp(options);
      acp.trusted = false;
      return acp;
    },
    resolveExecutable: () => "/bin/vibe-acp",
    baseEnv: {},
    initTimeoutMs: 500,
  });
  const result = await session.start();
  expect(acp?.sent("_trust/decision")).toEqual([{ sessionId: SESSION_ID, decision: "trust_cwd", cwd: "/w" }]);
  expect(acp?.sent("session/new")).toHaveLength(2);
  expect(result.agent_session_id).toBe(`${SESSION_ID}-2`);
  await session.stop();
});
