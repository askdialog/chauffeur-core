import { describe, expect, test } from "bun:test";
import type { Settings } from "@anthropic-ai/claude-agent-sdk";
import { buildClaudeOptions } from "../src/agents/claude/options.ts";
import type { SessionStartParams } from "../src/protocol.ts";

const deps = {
  baseEnv: {},
  executable: undefined,
  canUseTool: async () => ({ behavior: "deny" as const, message: "" }),
  hooks: {},
  abortController: new AbortController(),
  stderr: () => {},
  jevPluginDir: () => "/jev",
};

function start(extra: Partial<SessionStartParams>): SessionStartParams {
  return {
    chat_id: "c",
    workspace_id: "w",
    agent: "claude",
    cwd: "/w",
    additional_directories: [],
    fast: false,
    permission_mode: "default",
    read_only: false,
    env: {},
    prompt_attachments: [],
    prompt_suggestions: false,
    ...extra,
  };
}

describe("a chat's brands", () => {
  test("are read without asking, and a chat without one gets no rule", () => {
    const branded = buildClaudeOptions(start({ brands_dir: "/home/me/.chauffeur/brands" }), deps);
    expect((branded.settings as Settings | undefined)?.permissions?.allow).toContain("Read(//home/me/.chauffeur/brands/**)");
    const plain = buildClaudeOptions(start({}), deps);
    expect((plain.settings as Settings | undefined)?.permissions?.allow ?? []).not.toContainEqual(expect.stringContaining("brands"));
  });
});
