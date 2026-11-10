import { describe, expect, test } from "bun:test";
import { ClaudeSession } from "../src/agents/claude/session.ts";
import { CodexSession } from "../src/agents/codex/session.ts";
import type { SessionStartParams } from "../src/protocol.ts";
import { codex, FakeAppServer, HOOK } from "./support/codex-fakes.ts";
import { FakeHost, FakeQuery, flush, sdk } from "./support/fakes.ts";

const BASE: SessionStartParams = {
  chat_id: "c",
  workspace_id: "w",
  agent: "claude",
  cwd: "/w",
  additional_directories: [],
  fast: false,
  prompt_attachments: [],
  permission_mode: "default",
  read_only: false,
  prompt_suggestions: false,
  env: {},
};

const delivered = (host: FakeHost) =>
  host.events.filter((e) => e.type === "user_message_delivered").map((e) => (e as { message_id: string }).message_id);

async function claude() {
  const host = new FakeHost();
  let query: FakeQuery | undefined;
  const session = new ClaudeSession(BASE, host, {
    query: ({ prompt, options }) => (query = new FakeQuery(prompt, options)),
    resolveExecutable: () => undefined,
    baseEnv: {},
  });
  await session.start();
  return { session, host, query: query as FakeQuery };
}

describe("Claude: when a message sent mid-turn is read", () => {
  test("a message sent while idle starts a turn: read at once", async () => {
    const { session, host } = await claude();
    session.send("go", [], "m1");
    expect(delivered(host)).toEqual(["m1"]);
  });

  test("mid-turn: read at the first reply after the next tool result", async () => {
    const { session, host, query } = await claude();
    session.send("go");
    await query.waitForSent(1);
    query.push(sdk.init(), sdk.toolUse("a1", "t1", "Bash", { command: "sleep 5" }));
    await flush();
    session.send("also this", [], "m2");
    query.push(sdk.text("a1", "still thinking"));
    await flush();
    expect(delivered(host)).toEqual([]);
    query.push(sdk.toolResult("t1", "ok"));
    await flush();
    expect(delivered(host)).toEqual([]);
    query.push(sdk.text("a2", "Done, and here is the other thing"));
    await flush();
    expect(delivered(host)).toEqual(["m2"]);
    await session.stop();
  });

  test("a message left for the next turn is read when that turn replies; one folded in, at the result", async () => {
    const { session, host, query } = await claude();
    session.send("go");
    await query.waitForSent(1);
    query.push(sdk.init(), sdk.text("a1", "working"));
    await flush();
    session.send("next", [], "m3");
    query.push(sdk.result({ queued_turn_count: 1 }));
    await flush();
    expect(delivered(host)).toEqual([]);
    query.push(sdk.text("a2", "on it"));
    await flush();
    expect(delivered(host)).toEqual(["m3"]);

    session.send("more", [], "m4");
    query.push(sdk.result({ queued_turn_count: 0 }));
    await flush();
    expect(delivered(host)).toEqual(["m3", "m4"]);
    await session.stop();
  });

  test("interrupted mid-tool, a message steering the turn runs as the next one", async () => {
    const { session, host, query } = await claude();
    session.send("go");
    await query.waitForSent(1);
    query.push(sdk.init(), sdk.toolUse("a1", "t1", "Bash", { command: "sleep 600" }));
    await flush();
    session.send("where are we?", [], "m5");
    await session.interrupt();
    expect(query.calls).toContain("interrupt");
    query.push(sdk.result({ queued_turn_count: 1 }));
    await flush();
    const turns = host.events.filter((e) => e.type === "turn_started" || e.type === "turn_ended").map((e) => e.type);
    expect(turns.slice(-2)).toEqual(["turn_ended", "turn_started"]);
    query.push(sdk.text("a2", "We are at 30 of 65"));
    await flush();
    expect(delivered(host)).toEqual(["m5"]);
    await session.stop();
  });
});

describe("Codex: when a message sent mid-turn is read", () => {
  test("once the steer is accepted, or the turn it starts", async () => {
    const host = new FakeHost();
    let server: FakeAppServer | undefined;
    const session = new CodexSession({ ...BASE, agent: "codex" }, host, {
      spawn: (options) => (server = new FakeAppServer(options)),
      resolveExecutable: () => "/usr/local/bin/codex",
      baseEnv: {},
      hookCommand: () => HOOK,
      initTimeoutMs: 500,
    });
    await session.start();
    session.send("go", [], "m1");
    await flush();
    expect(delivered(host)).toEqual(["m1"]);
    server?.push(codex.turnStarted("turn-1"));
    await flush();
    session.send("steer", [], "m2");
    await flush();
    expect(delivered(host)).toEqual(["m1", "m2"]);
    await session.stop();
  });
});
