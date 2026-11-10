import { afterAll, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { readThreadHistory, type ResponseItem } from "../src/agents/codex/history.ts";
import { compactCodexHistory, isThreadContext, type JevAsker } from "../src/agents/codex/jev.ts";
import { CodexSession } from "../src/agents/codex/session.ts";
import { listCodexCommands } from "../src/agents/codex/skills.ts";
import { silentLogger } from "../src/log.ts";
import type { ChatEvent, SessionStartParams } from "../src/protocol.ts";
import { codex, FakeAppServer, THREAD_ID } from "./support/codex-fakes.ts";
import { FakeHost, flush } from "./support/fakes.ts";

const root = mkdtempSync(join(tmpdir(), "codex-jev-"));
afterAll(() => rmSync(root, { recursive: true, force: true }));

const user = (text: string): ResponseItem => ({ type: "message", role: "user", content: [{ type: "input_text", text }] });
const assistant = (text: string): ResponseItem => ({ type: "message", role: "assistant", content: [{ type: "output_text", text }] });
const developer = (text: string): ResponseItem => ({ type: "message", role: "developer", content: [{ type: "input_text", text }] });
const reasoning = (id: string): ResponseItem => ({ type: "reasoning", id, summary: [], encrypted_content: "gAAAA" });
const exec = (id: string, script: string): ResponseItem => ({ type: "custom_tool_call", call_id: id, name: "exec", input: script, status: "completed" });
const execOutput = (id: string, text: string): ResponseItem => ({ type: "custom_tool_call_output", call_id: id, output: [{ type: "input_text", text }] });
const call = (id: string, name: string, args: object): ResponseItem => ({ type: "function_call", call_id: id, name, arguments: JSON.stringify(args) });
const callOutput = (id: string, text: string): ResponseItem => ({ type: "function_call_output", call_id: id, output: text });

/** A rollout file in a session folder of a fake CODEX_HOME. */
let rollouts = 0;
function rollout(lines: object[], name = `rollout-2026-10-21T10-00-00-${String(++rollouts).padStart(8, "0")}-aaaa.jsonl`): string {
  const dir = join(root, "codex-home", "sessions", "2026", "10", "21");
  mkdirSync(dir, { recursive: true });
  const path = join(dir, name);
  writeFileSync(path, lines.map((line) => `${JSON.stringify(line)}\n`).join(""));
  return path;
}
const meta = (extra: object = {}) => ({ type: "session_meta", payload: { id: "t", ...extra } });
const item = (payload: ResponseItem) => ({ type: "response_item", payload });

/** Answers Jev's questions from a table: `call_<id>` / `result_<id>` → probability, 1 otherwise. */
function asker(answers: Record<string, number>, seen: string[] = []): JevAsker {
  return {
    async ask(_state, questions) {
      seen.push(...Object.keys(questions));
      return { answers: Object.fromEntries(Object.keys(questions).map((name) => [name, { noul: answers[name] ?? 1 }])) };
    },
  };
}

describe("reading a thread's history", () => {
  test("a rollout's history is its response items, the last compaction replacing what came before", async () => {
    const path = rollout([
      meta(),
      item(user("first")),
      { type: "event_msg", payload: { type: "task_started" } },
      item(assistant("one")),
      { type: "compacted", payload: { message: "", replacement_history: [user("first"), { type: "compaction", encrypted_content: "x" }] } },
      { type: "turn_context", payload: {} },
      item(user("second")),
      item(assistant("two")),
    ]);
    expect(await readThreadHistory(path)).toEqual([user("first"), { type: "compaction", encrypted_content: "x" }, user("second"), assistant("two")]);
  });

  test("a rollout resting on another one starts from that one's history, up to where it says", async () => {
    const baseLines = [meta(), item(user("kept")), item(assistant("kept too"))];
    const kept = baseLines.map((line) => `${JSON.stringify(line)}\n`).join("");
    const base = rollout([...baseLines, item(user("reverted"))], "rollout-2026-10-21T09-00-00-01a15db8-545f-76e1-bf0c-0ec566835539.jsonl");
    const path = rollout([
      meta({ history_base: { thread_id: "01a15db8-545f-76e1-bf0c-0ec566835539", end_ordinal_exclusive: 3, end_byte_offset: Buffer.byteLength(kept) } }),
      item(user("after")),
    ]);
    expect(base).toContain("sessions");
    expect(await readThreadHistory(path)).toEqual([user("kept"), assistant("kept too"), user("after")]);
  });

  test("a base rollout is looked for first in the day folder its thread id was created on", async () => {
    const created = new Date(2026, 9, 22, 12).getTime();
    const id = `${created.toString(16).padStart(12, "0").replace(/^(.{8})(.{4})$/, "$1-$2")}-7000-8000-000000000001`;
    const write = (day: string, text: string) => {
      const dir = join(root, "codex-home", "sessions", ...day.split("/"));
      mkdirSync(dir, { recursive: true });
      writeFileSync(join(dir, `rollout-${id}.jsonl`), [meta(), item(user(text))].map((line) => `${JSON.stringify(line)}\n`).join(""));
    };
    // Walked first in a full walk, the older folder would win.
    write("2000/01/01", "a stale copy");
    write("2026/10/22", "the base");
    const path = rollout([meta({ history_base: { thread_id: id, end_byte_offset: 10_000 } }), item(user("after"))]);
    expect(await readThreadHistory(path)).toEqual([user("the base"), user("after")]);
  });

  test("what Chauffeur cannot replay is refused rather than guessed", async () => {
    await expect(readThreadHistory(rollout([meta(), item(user("a")), { type: "event_msg", payload: { type: "thread_rolled_back", num_turns: 1 } }]))).rejects.toThrow(/rollback/);
    await expect(readThreadHistory(rollout([meta(), { type: "compacted", payload: { message: "a summary" } }]))).rejects.toThrow(/compaction/);
    await expect(readThreadHistory(rollout([meta({ history_base: { thread_id: "01a15dff-0000-7000-8000-000000000000", end_byte_offset: 10 } })]))).rejects.toThrow(/missing/);
  });
});

describe("compacting a Codex history with Jev", () => {
  const big = "x".repeat(20_000);
  /** Two calls Jev judges, then the newest messages it never touches. */
  const history: ResponseItem[] = [
    developer("<permissions instructions>sandboxed</permissions instructions>"),
    user("# AGENTS.md instructions for /w\n\n<INSTRUCTIONS>be nice</INSTRUCTIONS>"),
    user("Fix the build"),
    reasoning("rs_1"),
    exec("c1", "tools.exec_command({cmd: 'cat build.log'})"),
    execOutput("c1", big),
    call("c2", "shell", { command: ["ls"] }),
    callOutput("c2", big),
    assistant("The log says the import is wrong."),
    user("go on"),
    exec("c3", "tools.exec_command({cmd: 'git diff'})"),
    execOutput("c3", "diff"),
    assistant("Done."),
    user("thanks"),
  ];

  test("an output Jev no longer needs is cut, a call it no longer needs goes with its output, Codex's context is left out", async () => {
    const seen: string[] = [];
    const { items, result } = await compactCodexHistory(history, asker({ result_t1: 0.1, call_t2: 0.1, result_t2: 0.1 }, seen));
    // The newest messages (c3) are never asked about.
    expect(seen.sort()).toEqual(["call_t1", "call_t2", "result_t1", "result_t2"]);
    expect(items.map((i) => i.call_id ?? i.type)).toEqual(["message", "reasoning", "c1", "c1", "message", "message", "c3", "c3", "message", "message"]);
    expect(items[0]).toEqual(user("Fix the build"));
    expect(items[2]).toBe(history[4] as ResponseItem);
    expect(items[3]).toMatchObject({ type: "custom_tool_call_output", call_id: "c1" });
    expect(String(items[3]?.output)).toContain("truncated");
    expect(String(items[3]?.output).length).toBeLessThan(1_000);
    expect(items[7]).toBe(history[11] as ResponseItem);
    expect(result.stats).toMatchObject({ resultsDropped: 1, callsDropped: 1 });
  });

  test("a compaction that would remove too little is refused, and so is Jev failing", async () => {
    await expect(compactCodexHistory(history, asker({}))).rejects.toThrow(/only/);
    const failing: JevAsker = { ask: async () => Promise.reject(new Error("Jev request failed (401)")) };
    await expect(compactCodexHistory(history, failing)).rejects.toThrow(/401/);
  });

  test("Codex's own context is told apart from what the user wrote", () => {
    expect(isThreadContext(developer("anything"))).toBe(true);
    expect(isThreadContext(user("<environment_context>\n  <cwd>/w</cwd>\n</environment_context>"))).toBe(true);
    const agents = "# AGENTS.md instructions for /w\n\n<INSTRUCTIONS>\nbe nice\n</INSTRUCTIONS>";
    expect(isThreadContext({ type: "message", role: "user", content: [{ type: "input_text", text: agents }, { type: "input_text", text: "<environment_context></environment_context>" }] })).toBe(true);
    // Text the user pasted around such a block is theirs.
    expect(isThreadContext(user(`${agents}\n\nNow update these rules to forbid force pushes.`))).toBe(false);
    expect(isThreadContext(user("<environment_context> is what Codex adds; explain it"))).toBe(false);
    expect(isThreadContext(user("Read <environment_context> in the docs"))).toBe(false);
    expect(isThreadContext(user("Fix the build"))).toBe(false);
  });
});

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
  env: {},
  model: "gpt-6.1-sol",
};

const JEV = { api_key: "ts_key", threshold_percent: 60 };
const WINDOW = 100_000;

/** A Codex session whose thread's rollout holds `history`, Jev answering from `answers`. */
async function started(params: Partial<SessionStartParams>, answers: Record<string, number> | Error = {}) {
  const host = new FakeHost();
  let server!: FakeAppServer;
  const recent = ["a", "b", "c", "d", "e", "f"].map((text, i) => (i % 2 ? assistant(text) : user(text)));
  const path = rollout([meta(), ...[user("Fix the build"), exec("c1", "cat build.log"), execOutput("c1", "x".repeat(20_000)), assistant("Fixed."), ...recent].map(item)]);
  const session = new CodexSession({ ...BASE, ...params }, host, {
    spawn: (options) => {
      server = new FakeAppServer(options);
      server.rolloutPath = path;
      let threads = 0;
      server.handlers["thread/start"] = (p) => ({ thread: { id: ++threads === 1 ? THREAD_ID : `thread-${threads}` }, model: (p as { model?: string }).model ?? "gpt-6.1-sol" });
      return server;
    },
    resolveExecutable: () => "/usr/local/bin/codex",
    baseEnv: {},
    hookCommand: () => undefined,
    initTimeoutMs: 500,
    jevAsker: () => (answers instanceof Error ? { ask: async () => Promise.reject(answers) } : asker(answers)),
  });
  await session.start();
  return { session, host, server };
}

/** The thread the chat is on: the last one Jev moved it to. */
function current(server: FakeAppServer): string {
  return server.sent("thread/inject_items").at(-1)?.threadId ?? THREAD_ID;
}

/** `message`, from the thread the chat is on. */
function onCurrent<T extends { params: object }>(server: FakeAppServer, message: T): T {
  return { ...message, params: { ...message.params, threadId: current(server) } };
}

/** A turn whose last response used `percent` of the window. */
function turn(server: FakeAppServer, id: string, percent: number): void {
  const usage = { totalTokens: percent * 1_000, inputTokens: percent * 1_000, cachedInputTokens: 0, cacheWriteInputTokens: 0, outputTokens: 0, reasoningOutputTokens: 0 };
  server.push(
    onCurrent(server, codex.turnStarted(id)),
    { method: "thread/tokenUsage/updated", params: { threadId: current(server), turnId: id, tokenUsage: { total: usage, last: usage, modelContextWindow: WINDOW } } },
    onCurrent(server, codex.turnCompleted(id)),
  );
}

const kinds = (events: ChatEvent[]) => events.map((e) => (e.type === "notice" ? `notice:${e.message.split(":")[0]}` : e.type));

describe("a Codex chat compacting through Jev", () => {
  test("a turn that fills the context past the threshold moves the chat to a thread holding what Jev keeps, then ends", async () => {
    const { session, host, server } = await started({ jev: JEV }, { result_t1: 0.1 });
    session.send("Fix the build");
    await flush();
    turn(server, "turn-1", 65);
    await flush(20);
    expect(kinds(host.events)).toEqual([
      "session_started",
      "turn_started",
      "context_usage",
      "notice:Compacting the conversation with Jev",
      "session_started",
      "notice:Jev compacted the conversation, 98% smaller",
      "context_usage",
      "turn_ended",
    ]);
    const [reading, estimate] = host.events.flatMap((e) => (e.type === "context_usage" ? [e.usage] : []));
    expect(reading?.compaction).toEqual({ by: "jev", threshold_percent: 60 });
    expect(reading?.estimated).toBeUndefined();
    expect(host.events.filter((e) => e.type === "session_started").at(-1)).toEqual({ type: "session_started", agent_session_id: "thread-2", model: "gpt-6.1-sol" });
    // The new thread is opened as the first one was, and gets what Jev kept.
    const [first, second] = server.sent("thread/start");
    expect(second).toMatchObject({ ...first, serviceTier: null });
    const [injected] = server.sent("thread/inject_items");
    expect(injected.threadId).toBe("thread-2");
    expect(injected.items.map((i: ResponseItem) => i.type)).toEqual(["message", "custom_tool_call", "custom_tool_call_output", ...Array(7).fill("message")]);
    expect(String(injected.items[2].output)).toContain("truncated");
    // The context left is estimated: the last reading less the ~20k characters cut (~5k tokens).
    expect(estimate).toMatchObject({ estimated: true, max_tokens: WINDOW, percent: 60, compaction: { by: "jev", threshold_percent: 60 } });
    expect(estimate?.used_tokens).toBeGreaterThan(59_000);
    expect(estimate?.used_tokens).toBeLessThan(61_000);
    // The chat goes on in the new thread, the mode's settings sent again.
    session.send("next");
    await flush();
    expect(server.sent("turn/start").at(-1)).toMatchObject({ threadId: "thread-2", approvalPolicy: "on-request" });
  });

  test("the estimate after a compaction gives way to Codex's first count in the new thread, before the turn ends", async () => {
    const { session, host, server } = await started({ jev: JEV }, { result_t1: 0.1 });
    session.send("Fix the build");
    await flush();
    turn(server, "turn-1", 65);
    await flush(20);
    session.send("next");
    await flush();
    const usage = { totalTokens: 18_000, inputTokens: 17_000, cachedInputTokens: 0, cacheWriteInputTokens: 0, outputTokens: 1_000, reasoningOutputTokens: 0 };
    const count = { method: "thread/tokenUsage/updated" as const, params: { threadId: "thread-2", turnId: "turn-2", tokenUsage: { total: usage, last: usage, modelContextWindow: WINDOW } } };
    server.push(onCurrent(server, codex.turnStarted("turn-2")), count);
    await flush();
    const measured = host.events.at(-1);
    expect(measured).toMatchObject({ type: "context_usage", usage: { used_tokens: 18_000, percent: 18 } });
    expect(measured?.type === "context_usage" && measured.usage.estimated).toBeUndefined();
    // Later counts wait for the turn's end, as before.
    server.push(count, onCurrent(server, codex.turnCompleted("turn-2")));
    await flush(20);
    expect(kinds(host.events.slice(host.events.indexOf(measured as ChatEvent)))).toEqual(["context_usage", "context_usage", "turn_ended"]);
    expect(host.events.at(-1)).toMatchObject({ type: "turn_ended", outcome: "success" });
  });

  test("messages sent while it compacts wait for the compaction, then run as the next turn", async () => {
    let release!: () => void;
    const { session, host, server } = await started({ jev: JEV }, { result_t1: 0.1 });
    server.handlers["thread/inject_items"] = () => new Promise((resolve) => (release = () => resolve({})));
    session.send("Fix the build");
    await flush();
    turn(server, "turn-1", 70);
    await flush(20);
    session.send("and now?");
    await flush();
    expect(session.idle()).toBe(false);
    expect(server.sent("turn/start")).toHaveLength(1);
    expect(host.events.filter((e) => e.type === "turn_ended")).toHaveLength(0);
    release();
    await flush(20);
    expect(server.sent("turn/start")).toHaveLength(2);
    expect(server.sent("turn/start")[1]).toMatchObject({ threadId: "thread-2", input: [{ type: "text", text: "and now?" }] });
  });

  test("below the threshold, after a failed turn, or with Jev off, nothing is compacted", async () => {
    for (const [params, percent, status] of [
      [{ jev: JEV }, 59, "completed"],
      [{ jev: JEV }, 80, "failed"],
      [{}, 90, "completed"],
    ] as const) {
      const { session, server } = await started(params, { result_t1: 0.1 });
      session.send("hi");
      await flush();
      const usage = { totalTokens: percent * 1_000, inputTokens: 0, cachedInputTokens: 0, cacheWriteInputTokens: 0, outputTokens: 0, reasoningOutputTokens: 0 };
      server.push(
        codex.turnStarted("turn-1"),
        { method: "thread/tokenUsage/updated", params: { threadId: THREAD_ID, turnId: "turn-1", tokenUsage: { total: usage, last: usage, modelContextWindow: WINDOW } } },
        codex.turnCompleted("turn-1", status),
      );
      await flush(20);
      expect(server.sent("thread/read")).toHaveLength(0);
      expect(session.idle()).toBe(true);
    }
  });

  test("never twice in a row: not after Codex compacted on its own, nor until the context grew 10 points past the last compaction", async () => {
    const { session, server } = await started({ jev: JEV }, { result_t1: 0.1 });
    const run = async (id: string, percent: number, compacted = false) => {
      session.send("go");
      await flush();
      if (compacted) server.push({ method: "item/started", params: { threadId: current(server), turnId: id, startedAtMs: 1, item: { type: "contextCompaction", id: `cmp-${id}` } } });
      turn(server, id, percent);
      await flush(20);
      return server.sent("thread/read").length;
    };
    expect(await run("turn-1", 70, true)).toBe(0); // Codex compacted in this turn: left at 70
    expect(await run("turn-2", 75)).toBe(0);
    expect(await run("turn-3", 80)).toBe(1); // 10 points past
    expect(await run("turn-4", 64)).toBe(1); // the reading after Jev's compaction: left at 64
    expect(await run("turn-5", 73)).toBe(1);
    expect(await run("turn-6", 74)).toBe(2);
  });

  test("when Jev fails, Codex compacts the conversation its own way, within the same turn", async () => {
    const { session, host, server } = await started({ jev: JEV }, new Error("Jev request failed (401)"));
    server.handlers["thread/compact/start"] = () => {
      queueMicrotask(() => {
        server.push(
          codex.turnStarted("compact-1"),
          { method: "item/started", params: { threadId: THREAD_ID, turnId: "compact-1", startedAtMs: 1, item: { type: "contextCompaction", id: "cmp-1" } } },
          codex.turnCompleted("compact-1"),
        );
      });
      return {};
    };
    session.send("Fix the build");
    await flush();
    turn(server, "turn-1", 65);
    await flush(20);
    expect(kinds(host.events)).toEqual([
      "session_started",
      "turn_started",
      "context_usage",
      "notice:Compacting the conversation with Jev",
      "notice:Jev did not compact the conversation (Jev request failed (401))",
      "notice:Conversation compacted",
      "turn_ended",
    ]);
    expect(server.sent("thread/compact/start")).toEqual([{ threadId: THREAD_ID }]);
    expect(server.sent("thread/inject_items")).toHaveLength(0);
    expect(session.idle()).toBe(true);
  });

  test("interrupting a compaction ends the turn, the conversation left as it was", async () => {
    const { session, host, server } = await started({ jev: JEV }, { result_t1: 0.1 });
    server.handlers["thread/inject_items"] = () => new Promise(() => {});
    session.send("Fix the build");
    await flush();
    turn(server, "turn-1", 65);
    await flush(20);
    await session.interrupt();
    await flush(20);
    expect(host.events.at(-2)).toMatchObject({ type: "notice", message: "The compaction did not finish: interrupted" });
    expect(host.events.at(-1)).toMatchObject({ type: "turn_ended", outcome: "success" });
    expect(host.events.filter((e) => e.type === "session_started")).toHaveLength(1);
    expect(session.idle()).toBe(true);
  });
});

describe("/compact in a Codex chat", () => {
  test("compacts through Jev when it is on, as a turn of its own", async () => {
    const { session, host, server } = await started({ jev: JEV }, { result_t1: 0.1 });
    session.send("/compact", [], "m-1");
    await flush(20);
    expect(server.sent("turn/start")).toHaveLength(0);
    expect(server.sent("thread/inject_items")).toHaveLength(1);
    expect(kinds(host.events)).toEqual(["session_started", "turn_started", "user_message_delivered", "session_started", "notice:Jev compacted the conversation, 98% smaller", "turn_ended"]);
    expect(host.events.at(-1)).toMatchObject({ type: "turn_ended", outcome: "success" });
  });

  test("compacts Codex's own way when Jev is off, and waits for a running turn", async () => {
    const { session, host, server } = await started({});
    session.send("work");
    await flush();
    server.push(codex.turnStarted("turn-1"));
    session.send("/compact");
    await flush();
    expect(server.sent("turn/steer")).toHaveLength(0);
    expect(server.sent("thread/compact/start")).toHaveLength(0);
    server.handlers["thread/compact/start"] = () => {
      queueMicrotask(() => server.push(codex.turnStarted("compact-1"), codex.turnCompleted("compact-1")));
      return {};
    };
    server.push(codex.turnCompleted("turn-1"));
    await flush(20);
    expect(server.sent("thread/compact/start")).toEqual([{ threadId: THREAD_ID }]);
    expect(host.events.filter((e) => e.type === "turn_ended")).toHaveLength(2);
    expect(session.idle()).toBe(true);
  });

  test("is listed first among Codex's commands", async () => {
    let server!: FakeAppServer;
    const commands = await listCodexCommands(
      {
        spawn: (options) => {
          server = new FakeAppServer(options);
          server.skills = [{ name: "review-pr", description: "Review", shortDescription: null, path: "/s", scope: "user", enabled: true }];
          return server;
        },
        resolveExecutable: () => "/usr/local/bin/codex",
        baseEnv: {},
        hookCommand: () => undefined,
      },
      silentLogger,
      "/w",
      [],
    );
    expect(commands.map((c) => c.name)).toEqual(["compact", "review-pr"]);
  });
});

test("interrupting Codex's own compaction interrupts its turn, which ends the compaction once", async () => {
  const { session, host, server } = await started({});
  server.handlers["thread/compact/start"] = () => {
    queueMicrotask(() => server.push(codex.turnStarted("compact-1")));
    return {};
  };
  session.send("/compact");
  await flush(20);
  await session.interrupt();
  expect(server.sent("turn/interrupt")).toEqual([{ threadId: THREAD_ID, turnId: "compact-1" }]);
  server.push(codex.turnCompleted("compact-1", "interrupted"));
  await flush(20);
  expect(kinds(host.events)).toEqual(["session_started", "turn_started", "notice:The compaction did not finish", "turn_ended"]);
  expect(host.events.at(-1)).toMatchObject({ type: "turn_ended", outcome: "interrupted" });
  expect(session.idle()).toBe(true);
});

test("an interrupt before Codex's own compaction started its turn interrupts that turn once it starts", async () => {
  const { session, host, server } = await started({});
  session.send("/compact");
  await flush(20);
  await session.interrupt();
  expect(server.sent("turn/interrupt")).toHaveLength(0);
  server.push(codex.turnStarted("compact-1"));
  await flush(20);
  expect(server.sent("turn/interrupt")).toEqual([{ threadId: THREAD_ID, turnId: "compact-1" }]);
  server.push(codex.turnCompleted("compact-1", "interrupted"));
  await flush(20);
  expect(host.events.at(-1)).toMatchObject({ type: "turn_ended", outcome: "interrupted" });
  expect(session.idle()).toBe(true);
});

test("a turn/start answered after its turn ended does not steer what waits for the compaction into it", async () => {
  const { session, server } = await started({ jev: JEV }, new Error("Jev request failed (401)"));
  let answer!: () => void;
  server.handlers["turn/start"] = () => new Promise((resolve) => (answer = () => resolve({ turn: { id: "turn-1", status: "inProgress" } })));
  server.handlers["thread/compact/start"] = () => {
    queueMicrotask(() => server.push(codex.turnStarted("compact-1")));
    return {};
  };
  session.send("Fix the build");
  await flush();
  turn(server, "turn-1", 70);
  await flush(20);
  session.send("and now?");
  answer();
  await flush(20);
  expect(server.sent("turn/steer")).toHaveLength(0);
  server.push(codex.turnCompleted("compact-1"));
  await flush(20);
  expect(server.sent("turn/start").at(-1)).toMatchObject({ input: [{ type: "text", text: "and now?" }] });
});

test("an interrupt meant for a compaction that failed to start leaves the next turn alone", async () => {
  const { session, server } = await started({});
  let refuse!: () => void;
  server.handlers["thread/compact/start"] = () => new Promise((_, reject) => (refuse = () => reject(new Error("thread busy"))));
  session.send("/compact");
  await flush(20);
  await session.interrupt();
  refuse();
  await flush(20);
  session.send("hello");
  await flush();
  server.push(codex.turnStarted("turn-1"));
  await flush(20);
  expect(server.sent("turn/interrupt")).toHaveLength(0);
});
