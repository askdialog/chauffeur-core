import { expect, test } from "bun:test";
import type { SDKMessage } from "@anthropic-ai/claude-agent-sdk";
import { ClaudeAgent } from "../src/agents/claude/agent.ts";
import type { ClaudeSessionDeps } from "../src/agents/claude/session.ts";
import { CodexAgent } from "../src/agents/codex/agent.ts";
import { createSidecar } from "../src/app.ts";
import { silentLogger } from "../src/log.ts";
import { FakeAppServer, codex } from "./support/codex-fakes.ts";

function setup(claudeResult: string, isError: boolean, codexResult = "Fix checkout", failCodex = false) {
  let server: FakeAppServer | undefined;
  const claude = new ClaudeAgent({
    resolveExecutable: () => undefined,
    baseEnv: {},
    query: (() => (async function* () {
      yield { type: "result", subtype: "success", result: claudeResult, is_error: isError } as SDKMessage;
    })()) as unknown as ClaudeSessionDeps["query"],
  });
  const agent = new CodexAgent({
    resolveExecutable: () => "/fake/codex",
    baseEnv: {},
    spawn: (options) => {
      server = new FakeAppServer(options);
      server.models = [{ model: "gpt-6.1-luna" }];
      const current = server;
      current.handlers["turn/start"] = () => {
        current.push(codex.agentMessage("answer", codexResult));
        current.push(codex.turnCompleted("turn-1", failCodex ? "failed" : "completed"));
        return { turn: { id: "turn-1" } };
      };
      return current;
    },
  });
  const app = createSidecar({ agents: [claude, agent], log: silentLogger, write: () => {} });
  return { sessions: app.sessions, server: () => server };
}

test("a Claude quota error falls back to an ephemeral Luna naming turn", async () => {
  const { sessions, server } = setup("You've hit your session limit · resets 10pm (Europe/Paris)", true);
  expect(await sessions.title("Corrige le checkout")).toBe("Fix checkout");
  expect(server()?.sent("thread/start")[0]).toMatchObject({ model: "gpt-6.1-luna", ephemeral: true, sandbox: "read-only", approvalPolicy: "never" });
  expect(server()?.sent("turn/start")[0].input[0].text).toContain("Corrige le checkout");
  expect(server()?.closed).toBe(true);
  expect(sessions.size).toBe(0);
});

test("a valid Claude title does not start Codex", async () => {
  const { sessions, server } = setup("Fix checkout", false);
  expect(await sessions.title("Corrige le checkout")).toBe("Fix checkout");
  expect(server()).toBeUndefined();
});

test("Luna can keep the current title when Claude is unavailable", async () => {
  const { sessions, server } = setup("unavailable", true, "KEEP");
  expect(await sessions.retitle({ conversation: "Fix checkout", current: "Checkout repair" })).toBe("Checkout repair");
  expect(server()?.sent("turn/start")[0].input[0].text).toContain("Checkout repair");
  expect(server()?.closed).toBe(true);
});

test("a failed Codex turn never becomes a title and closes the naming server", async () => {
  const { sessions, server } = setup("unavailable", true, "Usage limit reached", true);
  await expect(sessions.title("Fix checkout")).rejects.toThrow("naming turn failed");
  expect(server()?.closed).toBe(true);
});

test("an empty Claude answer also falls back to Codex", async () => {
  const { sessions } = setup("", false);
  expect(await sessions.title("Fix checkout")).toBe("Fix checkout");
});
