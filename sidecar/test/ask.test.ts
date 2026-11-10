import { describe, expect, test } from "bun:test";
import { homedir } from "node:os";
import type { Options, SDKMessage } from "@anthropic-ai/claude-agent-sdk";
import { claudeAsk } from "../src/agents/claude/ask.ts";
import type { ClaudeSessionDeps } from "../src/agents/claude/session.ts";

/** Deps whose query records its options and prompt and answers with `result`. */
function fakeDeps(result: SDKMessage) {
  const seen: { options?: Options; prompt: string } = { prompt: "" };
  const deps = {
    query: ({ prompt: input, options }: { prompt: AsyncIterable<{ message: { content: string } }>; options: Options }) => {
      seen.options = options;
      return (async function* () {
        for await (const message of input) seen.prompt = message.message.content;
        yield result;
      })();
    },
    resolveExecutable: () => "/opt/claude",
    baseEnv: { HOME: "/home/me" },
  } as unknown as ClaudeSessionDeps;
  return { deps, seen };
}

const success = (text: string) => ({ type: "result", subtype: "success", result: text }) as unknown as SDKMessage;
const params = { agent: "claude" as const, instructions: "Reply with JSON only.", request: "<messages>…</messages>", timeout_ms: 5_000 };

describe("asking one question", () => {
  test("asks Claude Code in one turn, with the instructions as its system prompt, no tools and the model given", async () => {
    const { deps, seen } = fakeDeps(success(' {"suggestions": []}\n'));
    const text = await claudeAsk(deps, { ...params, model: "opus", effort: "high" });
    expect(text).toBe('{"suggestions": []}');
    expect(seen.prompt).toBe("<messages>…</messages>");
    expect(seen.options).toMatchObject({
      systemPrompt: "Reply with JSON only.",
      model: "opus",
      effort: "high",
      tools: [],
      settingSources: [],
      persistSession: false,
      maxTurns: 1,
      cwd: homedir(),
    });
  });

  test("says why it could not answer", async () => {
    const failed = fakeDeps({ type: "result", subtype: "error_during_execution" } as unknown as SDKMessage);
    await expect(claudeAsk(failed.deps, params)).rejects.toThrow("Claude Code could not answer: error_during_execution");
    const empty = fakeDeps(success(" "));
    await expect(claudeAsk(empty.deps, params)).rejects.toThrow("Claude Code could not answer: an empty answer");
  });
});
