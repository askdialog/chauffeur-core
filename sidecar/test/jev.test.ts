import { CHAUFFEUR_TOOL_RULES } from "../src/agents/claude/options.ts";
import { describe, expect, test } from "bun:test";
import { chmodSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";
import type { SDKMessage } from "@anthropic-ai/claude-agent-sdk";
import { jevPluginDir, jevPluginRoot, USER_JEV_PLUGIN } from "../src/agents/claude/jev.ts";
import { JEV_PLUGIN_FILES } from "../src/agents/claude/jev-plugin.generated.ts";
import { ClaudeSession } from "../src/agents/claude/session.ts";
import type { SessionStartParams } from "../src/protocol.ts";
import { generated, pluginFiles } from "../scripts/gen-jev-plugin.ts";
import { FakeHost, FakeQuery, flush, sdk } from "./support/fakes.ts";

const BASE: SessionStartParams = {
  chat_id: "chat-1",
  workspace_id: "ws-1",
  agent: "claude",
  cwd: "/workspaces/ws-1",
  additional_directories: [],
  fast: false,
  prompt_attachments: [],
  permission_mode: "default",
  read_only: false,
  prompt_suggestions: false,
  env: {},
};

const JEV = { api_key: "ts_key", threshold_percent: 10 };

async function started(params: Partial<SessionStartParams> = {}, contextPercentage = 15) {
  const host = new FakeHost();
  let query: FakeQuery | undefined;
  const session = new ClaudeSession({ ...BASE, ...params }, host, {
    query: ({ prompt, options }) => (query = new FakeQuery(prompt, options)),
    resolveExecutable: () => "/opt/claude",
    baseEnv: { PATH: "/usr/bin" },
    initTimeoutMs: 200,
    stopTimeoutMs: 200,
    jevPluginDir: () => "/tmp/jev-plugin",
  });
  await session.start();
  if (!query) throw new Error("no query");
  query.contextPercentage = contextPercentage;
  return { session, host, query };
}

function resultOf(fields: Record<string, unknown> = {}): SDKMessage {
  return sdk.result(fields);
}

function compactBoundary(postTokens: number): SDKMessage {
  return {
    type: "system",
    subtype: "compact_boundary",
    session_id: "sess-1",
    compact_metadata: { trigger: "manual", pre_tokens: 15, post_tokens: postTokens },
  } as unknown as SDKMessage;
}

function sentTexts(query: FakeQuery): unknown[] {
  return query.sent.map((message) => message.message.content);
}

describe("session options", () => {
  test("load Chauffeur's Jev plugin with the key, and leave the user's own install out", async () => {
    const { query } = await started({ jev: JEV, fast: true });
    expect(query.options.plugins).toEqual([{ type: "local", path: "/tmp/jev-plugin" }]);
    expect(query.options.env).toMatchObject({ CLAUDE_CODE_ENABLE_FUNCTION_HOOKS: "1", TYPESAFE_API_KEY: "ts_key", PATH: "/usr/bin" });
    expect(query.options.settings).toEqual({ fastMode: true, enabledPlugins: { [USER_JEV_PLUGIN]: false }, idleCompaction: false, permissions: { allow: CHAUFFEUR_TOOL_RULES } });
  });

  test("without Jev, neither plugin nor key", async () => {
    const { query } = await started();
    expect(query.options.plugins).toBeUndefined();
    expect(query.options.env?.TYPESAFE_API_KEY).toBeUndefined();
    // Only the chat's own allow rule for Chauffeur's repo tools.
    expect(query.options.settings).toEqual({ permissions: { allow: CHAUFFEUR_TOOL_RULES } });
  });
});

describe("compaction past the threshold", () => {
  test("sends /compact after the turn and ends the turn once compacted", async () => {
    const { session, host, query } = await started({ jev: JEV });
    session.send("hi");
    await query.waitForSent(1);
    query.push(sdk.text("m1", "hello"), resultOf());
    await query.waitForSent(2);
    expect(sentTexts(query)[1]).toBe("/compact");
    await flush();
    expect(host.types()).toEqual(["turn_started", "assistant_text", "context_usage", "notice"]);
    // The reading Jev measured is the one the app shows, with Jev's threshold marked.
    expect(host.events[2]).toMatchObject({ type: "context_usage", usage: { percent: 15, compaction: { by: "jev", threshold_percent: 10 } } });
    expect(query.contextReads).toEqual(["summary"]);
    expect(host.events[3]).toEqual({
      type: "notice",
      level: "info",
      message: "Compacting the conversation with Jev: 15% of the context is used",
    });

    query.push(compactBoundary(4), sdk.result({ num_turns: 0, duration_ms: 3000 }));
    await flush();
    expect(host.types()).toEqual(["turn_started", "assistant_text", "context_usage", "notice", "notice", "context_usage", "turn_ended"]);
    expect(host.events.at(-1)).toMatchObject({ type: "turn_ended", outcome: "success", duration_ms: 1200, context_tokens: 4 });

    // The next turn starts from the compacted context.
    session.send("next");
    await query.waitForSent(3);
    query.push(sdk.text("m2", "ok"), sdk.result());
    await flush();
    expect(host.types().slice(-4)).toEqual(["turn_started", "assistant_text", "context_usage", "turn_ended"]);
  });

  test("not below the threshold, nor without Jev, nor after a failed turn", async () => {
    for (const [params, result] of [
      [{ jev: { ...JEV, threshold_percent: 20 } }, resultOf()],
      [{}, resultOf()],
      [{ jev: JEV }, resultOf({ subtype: "error_during_execution", is_error: true })],
    ] as const) {
      const { session, host, query } = await started(params);
      session.send("hi");
      await query.waitForSent(1);
      query.push(sdk.text("m1", "hello"), result);
      await flush();
      expect(host.types().at(-1)).toBe("turn_ended");
      expect(query.sent).toHaveLength(1);
    }
  });

  test("not while messages wait to run as the next turn", async () => {
    const { session, host, query } = await started({ jev: JEV });
    session.send("first");
    await query.waitForSent(1);
    query.push(sdk.text("m1", "working"));
    session.send("second");
    await query.waitForSent(2);
    query.push(resultOf({ queued_turn_count: 1 }));
    await flush();
    expect(host.types()).toEqual(["turn_started", "assistant_text", "context_usage", "turn_ended", "turn_started"]);
    expect(sentTexts(query)).toEqual([expect.anything(), expect.anything()]);
  });

  test("not right after Claude Code compacted on its own, and not again before the context grows", async () => {
    const { session, host, query } = await started({ jev: JEV }, 40);
    // Claude Code compacts mid-turn (its own threshold): no second compaction at the turn's end.
    session.send("big task");
    await query.waitForSent(1);
    query.push(sdk.text("m1", "reading"), compactBoundary(5000), sdk.text("m2", "done"), resultOf());
    await flush();
    expect(host.types().at(-1)).toBe("turn_ended");
    expect(query.sent).toHaveLength(1);

    // Still at 40% (Jev kept much): not compacted again until it gains 10 points.
    session.send("more");
    await query.waitForSent(2);
    query.push(sdk.text("m3", "ok"), resultOf());
    await flush();
    expect(query.sent).toHaveLength(2);

    query.contextPercentage = 50;
    session.send("even more");
    await query.waitForSent(3);
    query.push(sdk.text("m4", "ok"), resultOf());
    await query.waitForSent(4);
    expect(sentTexts(query)[3]).toBe("/compact");
  });

  test("an interrupted compaction still ends the turn, with a warning", async () => {
    const { session, host, query } = await started({ jev: JEV });
    session.send("hi");
    await query.waitForSent(1);
    query.push(sdk.text("m1", "hello"), resultOf());
    await query.waitForSent(2);
    await session.interrupt();
    expect(query.calls).toContain("interrupt");
    query.push(sdk.result({ num_turns: 0, terminal_reason: "aborted_streaming" }));
    await flush();
    expect(host.events.slice(-2)).toEqual([
      { type: "notice", level: "warning", message: "The compaction did not finish: interrupted" },
      expect.objectContaining({ type: "turn_ended", outcome: "success", context_tokens: 15 }),
    ]);
  });

  test("a message sent while compacting waits, then runs as the next turn", async () => {
    const { session, host, query } = await started({ jev: JEV });
    session.send("hi");
    await query.waitForSent(1);
    query.push(sdk.text("m1", "hello"), resultOf());
    await query.waitForSent(2);
    session.send("meanwhile", [], "msg-2");
    await query.waitForSent(3);
    expect(host.types()).not.toContain("user_message_delivered");
    query.push(compactBoundary(4), sdk.result({ num_turns: 0, queued_turn_count: 1 }));
    await flush();
    expect(host.types().slice(-2)).toEqual(["turn_ended", "turn_started"]);
  });
});

describe("the plugin", () => {
  test("is written once per version to a folder Claude Code loads", () => {
    const root = mkdtempSync(join(tmpdir(), "jev-test-"));
    const files = { ".claude-plugin/plugin.json": '{"name":"jev"}', "hooks/hooks.json": "{}" };
    const dir = jevPluginDir(files, root);
    expect(readFileSync(join(dir, ".claude-plugin/plugin.json"), "utf8")).toBe('{"name":"jev"}');
    expect(jevPluginDir(files, root)).toBe(dir);
    const changed = jevPluginDir({ ...files, "hooks/hooks.json": '{"modules":[]}' }, root);
    expect(changed).not.toBe(dir);
    expect(readdirSync(root).sort()).toEqual([dir, changed].map((d) => d.slice(root.length + 1)).sort());
    expect(lstatSync(dir).mode & 0o077).toBe(0);
  });

  const FILES = { ".claude-plugin/plugin.json": '{"name":"jev"}', "hooks/hooks.json": "{}" };

  /** The name the plugin gets under any root, learnt from a clean one. */
  function expectedName(): string {
    return basename(jevPluginDir(FILES, mkdtempSync(join(tmpdir(), "jev-test-"))));
  }

  test("does not reuse a folder at its path that others can write into", () => {
    const root = mkdtempSync(join(tmpdir(), "jev-test-"));
    const planted = join(root, expectedName());
    mkdirSync(planted);
    chmodSync(planted, 0o777);
    writeFileSync(join(planted, ".complete"), "");
    writeFileSync(join(planted, "planted"), "evil");
    const dir = jevPluginDir(FILES, root);
    expect(dir).not.toBe(planted);
    expect(dir.startsWith(`${planted}-`)).toBe(true);
    expect(readFileSync(join(dir, "hooks/hooks.json"), "utf8")).toBe("{}");
    expect(lstatSync(dir).mode & 0o077).toBe(0);
    // The planted folder is neither loaded nor touched.
    expect(readFileSync(join(planted, "planted"), "utf8")).toBe("evil");
    expect(existsSync(join(planted, "hooks"))).toBe(false);
  });

  test("does not follow a symlink at its path", () => {
    const root = mkdtempSync(join(tmpdir(), "jev-test-"));
    const elsewhere = mkdtempSync(join(tmpdir(), "jev-elsewhere-"));
    writeFileSync(join(elsewhere, ".complete"), "");
    const link = join(root, expectedName());
    symlinkSync(elsewhere, link);
    const dir = jevPluginDir(FILES, root);
    expect(dir).not.toBe(link);
    expect(lstatSync(dir).isSymbolicLink()).toBe(false);
    expect(readFileSync(join(dir, "hooks/hooks.json"), "utf8")).toBe("{}");
    expect(lstatSync(link).isSymbolicLink()).toBe(true);
    expect(readdirSync(elsewhere)).toEqual([".complete"]);
  });

  test("goes under XDG_RUNTIME_DIR only when that is our private folder", () => {
    const saved = process.env.XDG_RUNTIME_DIR;
    try {
      const runtime = mkdtempSync(join(tmpdir(), "jev-runtime-"));
      process.env.XDG_RUNTIME_DIR = runtime;
      expect(jevPluginRoot()).toBe(runtime);
      chmodSync(runtime, 0o755 | 0o020);
      expect(jevPluginRoot()).toBe(tmpdir());
      delete process.env.XDG_RUNTIME_DIR;
      expect(jevPluginRoot()).toBe(tmpdir());
    } finally {
      if (saved === undefined) delete process.env.XDG_RUNTIME_DIR;
      else process.env.XDG_RUNTIME_DIR = saved;
    }
  });

  test("the sidecar carries plugins/jev as it is", () => {
    expect(generated(JEV_PLUGIN_FILES)).toBe(generated(pluginFiles()));
    expect(Object.keys(JEV_PLUGIN_FILES)).toContain("hooks/fast-jev.ts");
    expect(JSON.parse(JEV_PLUGIN_FILES[".claude-plugin/plugin.json"] ?? "{}").name).toBe("jev");
    expect(existsSync(join(jevPluginDir(), "hooks/hooks.json"))).toBe(true);
  });
});
