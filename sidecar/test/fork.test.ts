import { describe, expect, test } from "bun:test";
import { ClaudeAgent } from "../src/agents/claude/agent.ts";
import { codexFork } from "../src/agents/codex/fork.ts";
import { silentLogger } from "../src/log.ts";
import { FakeAppServer, HOOK } from "./support/codex-fakes.ts";

const SESSION = "0b8e6a52-4c1f-4d7e-9a43-1f2e3d4c5b6a";

describe("forking an agent session", () => {
  test("Claude copies the session in its project folder and gives the new id", async () => {
    const asked: Array<[string, string]> = [];
    const agent = new ClaudeAgent({}, undefined, async (sessionId, dir) => {
      asked.push([sessionId, dir]);
      return "5f0c1d2e-3b4a-4c5d-8e6f-7a8b9c0d1e2f";
    });
    expect(await agent.fork({ agent: "claude", session_id: SESSION, cwd: "/w", env: {} })).toBe("5f0c1d2e-3b4a-4c5d-8e6f-7a8b9c0d1e2f");
    expect(asked).toEqual([[SESSION, "/w"]]);
  });

  test("Claude copies the session up to a turn's last message", async () => {
    const asked: Array<[string, string, string | undefined]> = [];
    const agent = new ClaudeAgent({}, undefined, async (sessionId, dir, upTo) => {
      asked.push([sessionId, dir, upTo]);
      return "5f0c1d2e-3b4a-4c5d-8e6f-7a8b9c0d1e2f";
    });
    const upTo = "155d76c3-402f-4005-a112-aebd129537ad";
    await agent.fork({ agent: "claude", session_id: SESSION, cwd: "/w", env: {}, up_to: upTo });
    expect(asked).toEqual([[SESSION, "/w", upTo]]);
    await expect(agent.fork({ agent: "claude", session_id: SESSION, cwd: "/w", env: {}, up_to: "../x" })).rejects.toMatchObject({ code: 400 });
  });

  test("Claude refuses what is no session id, and names a failed copy", async () => {
    const agent = new ClaudeAgent({}, undefined, async () => {
      throw new Error("Session not found");
    });
    await expect(agent.fork({ agent: "claude", session_id: "../etc", cwd: "/w", env: {} })).rejects.toMatchObject({ code: 400 });
    await expect(agent.fork({ agent: "claude", session_id: SESSION, cwd: "/w", env: {} })).rejects.toMatchObject({
      code: 503,
      message: "Claude Code could not fork its session: Session not found",
    });
  });

  test("Codex forks the thread from a short-lived app-server, in the chat's folder and environment", async () => {
    let server: FakeAppServer | undefined;
    const id = await codexFork(
      {
        spawn: (options) => {
          server = new FakeAppServer(options);
          return server;
        },
        resolveExecutable: (env) => `${env.CODEX_BIN ?? "/usr/local/bin"}/codex`,
        baseEnv: { PATH: "/usr/bin", CODEX_BIN: "/opt" },
        hookCommand: () => HOOK,
      },
      silentLogger,
      { agent: "codex", session_id: "thread-1", cwd: "/w", env: { CODEX_HOME: "/h/.codex" } },
    );
    expect(id).toBe("thread-1-fork");
    expect(server?.options.cwd).toBe("/w");
    expect(server?.options.executable).toBe("/opt/codex");
    expect(server?.options.env).toMatchObject({ PATH: "/usr/bin", CODEX_HOME: "/h/.codex" });
    expect(server?.options.args).toEqual([]);
    expect(server?.requests.map((r) => r.method)).toEqual(["initialize", "thread/fork"]);
    expect(server?.sent("thread/fork")).toEqual([{ threadId: "thread-1", excludeTurns: true }]);
    expect(server?.closed).toBe(true);
  });

  test("Codex forks the thread through a given turn", async () => {
    let server: FakeAppServer | undefined;
    await codexFork(
      {
        spawn: (options) => {
          server = new FakeAppServer(options);
          return server;
        },
        resolveExecutable: () => "/usr/local/bin/codex",
        baseEnv: {},
        hookCommand: () => HOOK,
      },
      silentLogger,
      { agent: "codex", session_id: "thread-1", cwd: "/w", env: {}, up_to: "turn-2" },
    );
    expect(server?.sent("thread/fork")).toEqual([{ threadId: "thread-1", lastTurnId: "turn-2", excludeTurns: true }]);
  });
});
