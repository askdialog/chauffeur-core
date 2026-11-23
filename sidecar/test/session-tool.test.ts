import { describe, expect, test } from "bun:test";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { CHAUFFEUR_TOOL_RULES } from "../src/agents/claude/options.ts";
import { claudeRepoTools } from "../src/agents/claude/repo-tools.ts";
import { codexChauffeurTools, codexToolCall } from "../src/agents/codex/repo-tools.ts";
import { PendingRequests } from "../src/agents/pending.ts";
import { SESSION_ENSURE_TOOL, sessionEnsure } from "../src/agents/session-tool.ts";
import { FakeHost } from "./support/fakes.ts";

describe("session_ensure", () => {
  test("a session that holds lets the agent go on", async () => {
    const host = new FakeHost();
    expect(await sessionEnsure(host, { name: " aws-sso " })).toEqual({
      text: "AWS SSO (staging) (aws-sso) holds until 2030-01-01T00:00:00+00:00: go on.",
      error: false,
    });
    expect(host.ensured).toEqual(["aws-sso"]);
  });

  test("the daemon's reason (declined, unanswered, unknown) is the agent's error", async () => {
    const host = new FakeHost();
    host.ensureError = new Error("the user declined to log in to AWS SSO: stop what needed it, without working around it");
    expect(await sessionEnsure(host, { name: "aws-sso" })).toEqual({
      text: "Chauffeur could not ensure the session aws-sso: the user declined to log in to AWS SSO: stop what needed it, without working around it",
      error: true,
    });
  });

  test("no name asks nothing of the daemon", async () => {
    const host = new FakeHost();
    expect((await sessionEnsure(host, {})).error).toBe(true);
    expect(host.ensured).toEqual([]);
  });

  test("an interrupt stops the wait", async () => {
    const host = new FakeHost();
    host.sessionEnsure = () => new Promise(() => {});
    const abort = new AbortController();
    const call = sessionEnsure(host, { name: "aws-sso" }, { signal: abort.signal });
    abort.abort();
    expect(await call).toEqual({ text: "The call was given up before the session aws-sso held.", error: true });
  });
});

describe("session_ensure is an exploration's tool", () => {
  async function claudeTools(options: { sessionEnsure?: boolean }) {
    const server = claudeRepoTools(new FakeHost(), new PendingRequests(() => {}, () => "req"), options);
    const [clientSide, serverSide] = InMemoryTransport.createLinkedPair();
    await server.instance.connect(serverSide);
    const client = new Client({ name: "test", version: "1" });
    await client.connect(clientSide);
    return { client, names: (await client.listTools()).tools.map((t) => t.name) };
  }

  test("Claude: served only when asked for, and allowed by the chat", async () => {
    expect((await claudeTools({})).names).not.toContain("session_ensure");
    const { client, names } = await claudeTools({ sessionEnsure: true });
    expect(names).toContain("session_ensure");
    const result = (await client.callTool({ name: "session_ensure", arguments: { name: "aws-sso" } })) as {
      content: Array<{ text: string }>;
      isError?: boolean;
    };
    expect(result.isError).toBeUndefined();
    expect(result.content[0]?.text).toContain("holds");
    expect(CHAUFFEUR_TOOL_RULES).toContain(SESSION_ENSURE_TOOL);
  });

  test("Codex: a dynamic tool of the thread only when asked for, run against the daemon", async () => {
    const names = (options: { sessionEnsure?: boolean }) =>
      (codexChauffeurTools(options) as unknown as Array<{ tools: Array<{ name: string }> }>)[0]?.tools.map((t) => t.name);
    expect(names({})).toEqual(["list_repos", "add_repo"]);
    expect(names({ sessionEnsure: true })).toEqual(["list_repos", "add_repo", "session_ensure"]);
    const host = new FakeHost();
    const answer = await codexToolCall(
      { threadId: "t", turnId: "turn", callId: "call-1", namespace: "chauffeur", tool: "session_ensure", arguments: { name: "aws-sso" } } as never,
      host,
      new PendingRequests(() => {}, () => "req"),
    );
    expect(answer.success).toBe(true);
    expect(host.ensured).toEqual(["aws-sso"]);
  });
});
