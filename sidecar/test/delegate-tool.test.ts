import { describe, expect, test } from "bun:test";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { claudeRepoTools } from "../src/agents/claude/repo-tools.ts";
import { answer, delegate } from "../src/agents/delegate-tool.ts";
import { PendingRequests } from "../src/agents/pending.ts";
import { FakeHost, flush } from "./support/fakes.ts";

describe("delegate", () => {
  test("gives a new delegated chat its task and returns its answer, with how to give it more", async () => {
    const host = new FakeHost();
    const call = delegate(host, { agent: "codex", prompt: "  Review the diff of web  " }, { toolUseId: "toolu_1" });
    await flush();
    expect(host.delegated).toEqual([{ agent: "codex", prompt: "Review the diff of web", tool_use_id: "toolu_1" }]);
    host.answerFor("sub-1", { text: "One bug: the cart total skips discounts." });
    const result = await call;
    expect(result.error).toBe(false);
    expect(result.text).toBe(
      'Codex (delegated chat sub-1, "Review the diff") answered:\n\nOne bug: the cart total skips discounts.\n\n' +
        'Give it more work with delegate and chat "sub-1".',
    );
  });

  test("gives the same delegated chat more work, whatever else the call names", async () => {
    const host = new FakeHost();
    const call = delegate(host, { agent: "nobody", tier: "huge", review: true, prompt: "Fix it", chat: "sub-7" });
    await flush();
    expect(host.delegated).toEqual([{ prompt: "Fix it", chat: "sub-7" }]);
    host.answerFor("sub-7");
    expect((await call).error).toBe(false);
  });

  test("leaves the agent and the model to the daemon unless the call names them", async () => {
    const host = new FakeHost();
    void delegate(host, { prompt: "Implement the parser" });
    void delegate(host, { prompt: "Review the parser", review: true, tier: "strongest", effort: " HIGH " });
    void delegate(host, { prompt: "Try Fable", agent: "claude", model: "claude-fable-5-1[1m]" });
    await flush();
    expect(host.delegated).toEqual([
      { prompt: "Implement the parser" },
      { prompt: "Review the parser", review: true, tier: "strongest", effort: "high" },
      { prompt: "Try Fable", agent: "claude", model: "claude-fable-5-1[1m]" },
    ]);
  });

  test("an input without task, or with an unknown agent or tier, is refused before anything starts", async () => {
    const host = new FakeHost();
    expect(await delegate(host, { agent: "claude", prompt: "  " })).toEqual({ text: "Say the task in `prompt`.", error: true });
    expect(await delegate(host, { agent: "gemini", prompt: "Review" })).toEqual({
      text: "`agent` is codex or claude; leave it out for yours.",
      error: true,
    });
    expect(await delegate(host, { tier: "huge", prompt: "Review" })).toEqual({ text: "`tier` is fast, default or strongest.", error: true });
    expect(host.delegated).toEqual([]);
  });

  test("tells the agent when Chauffeur gave the task to another agent than asked", async () => {
    const host = new FakeHost();
    host.delegateNotice = "Claude has used 97% of its 5h limit: the task went to Codex instead.";
    const call = delegate(host, { prompt: "Review", review: true });
    await flush();
    host.answerFor("sub-1", { text: "LGTM" });
    expect((await call).text).toStartWith(
      "Claude has used 97% of its 5h limit: the task went to Codex instead.\n\nCodex (delegated chat sub-1",
    );
  });

  test("a refusal of the daemon is an error the agent reads", async () => {
    const host = new FakeHost();
    host.delegateError = new Error("the delegated chat sub-1 is still working");
    const result = await delegate(host, { agent: "codex", prompt: "More", chat: "sub-1" });
    expect(result).toEqual({ text: "Chauffeur could not give the task to a delegated chat: the delegated chat sub-1 is still working", error: true });
  });

  test("a call the agent gives up interrupts the delegated chat", async () => {
    const host = new FakeHost();
    const abort = new AbortController();
    const call = delegate(host, { agent: "codex", prompt: "Look" }, { signal: abort.signal });
    await flush();
    abort.abort();
    const result = await call;
    expect(result.error).toBe(true);
    expect(result.text).toContain("sub-1 was interrupted before it answered");
    expect(host.stopped).toEqual(["sub-1"]);
  });

  test("a call the agent's turn left without an interrupt leaves the delegated chat working", async () => {
    const host = new FakeHost();
    const abort = new AbortController();
    const detach = new AbortController();
    const call = delegate(host, { agent: "codex", prompt: "Look" }, { signal: abort.signal, detach: detach.signal });
    await flush();
    detach.abort();
    // An interrupt after that changes nothing: the delegated chat is no longer this call's.
    abort.abort();
    expect(await call).toEqual({ text: "The delegated chat sub-1 works on: its answer will come to you as a new message.", error: false });
    expect(host.detached).toEqual(["sub-1"]);
    expect(host.stopped).toEqual([]);
  });

  test("a delegated chat that did not finish says how it ended", () => {
    const base = { chat: "sub-1", agent: "claude", title: null, status: "idle", error: null, text: null } as const;
    expect(answer({ ...base, outcome: "interrupted", text: "Half done" })).toEqual({
      text: 'Claude (delegated chat sub-1) was interrupted before it finished; its last message:\n\nHalf done\n\nGive it more work with delegate and chat "sub-1".',
      error: true,
    });
    expect(answer({ ...base, outcome: "error", error: "rate limited" }).text).toStartWith("Claude (delegated chat sub-1) stopped on an error: rate limited");
    expect(answer({ ...base, status: "error", outcome: null, error: "the agent stopped before answering (its chat says why)" })).toEqual({
      text: "Claude (delegated chat sub-1) could not work on the task: the agent stopped before answering (its chat says why).",
      error: true,
    });
  });
});

describe("Claude's delegate tool", () => {
  async function connect(options: { delegate?: boolean }) {
    const host = new FakeHost();
    const requests = new PendingRequests(() => {}, () => "req");
    const server = claudeRepoTools(host, requests, options);
    const [clientSide, serverSide] = InMemoryTransport.createLinkedPair();
    await server.instance.connect(serverSide);
    const client = new Client({ name: "test", version: "1" });
    await client.connect(clientSide);
    return { host, client };
  }

  test("is served only when the daemon says so", async () => {
    const without = await connect({});
    expect((await without.client.listTools()).tools.map((t) => t.name)).toEqual(["list_repos", "add_repo", "update_plan"]);
    const { client } = await connect({ delegate: true });
    const tools = (await client.listTools()).tools;
    expect(tools.map((t) => t.name)).toEqual(["list_repos", "add_repo", "delegate", "update_plan"]);
    expect(tools[2]?.inputSchema.required).toEqual(["prompt"]);
  });

  test("keeps Claude Code's id for the call on the delegated chat", async () => {
    const { host, client } = await connect({ delegate: true });
    const call = client.callTool({
      name: "delegate",
      arguments: { agent: "codex", prompt: "Review" },
      _meta: { "claudecode/toolUseId": "toolu_42" },
    });
    for (let i = 0; i < 20 && host.delegated.length === 0; i++) await flush();
    expect(host.delegated).toEqual([{ agent: "codex", prompt: "Review", tool_use_id: "toolu_42" }]);
    host.answerFor("sub-1");
    const result = (await call) as { content: Array<{ text: string }>; isError?: boolean };
    expect(result.isError).toBeUndefined();
    expect(result.content[0]?.text).toContain("answered:\n\nLGTM");
  });
});

describe("Claude's update_plan tool", () => {
  test("records the plan and says how far it is", async () => {
    const { recordPlan } = await import("../src/agents/plan-tool.ts");
    expect(recordPlan({ steps: [{ text: "a", status: "completed" }, { text: "b", status: "pending" }] })).toEqual({
      text: "Plan recorded: 1/2 done.",
      error: false,
    });
    expect(recordPlan({ steps: [] }).error).toBe(true);
  });
});
