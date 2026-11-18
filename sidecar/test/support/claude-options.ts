import type { Options } from "@anthropic-ai/claude-agent-sdk";
import type { OptionDeps } from "../../src/agents/claude/options.ts";
import type { SessionStartParams } from "../../src/protocol.ts";

/** What `buildClaudeOptions` needs besides the session's parameters, inert. */
export const optionDeps: OptionDeps = {
  baseEnv: {},
  executable: undefined,
  canUseTool: async () => ({ behavior: "deny" as const, message: "" }),
  hooks: {},
  abortController: new AbortController(),
  stderr: () => {},
  jevPluginDir: () => "/jev",
};

/** A Claude chat's start in `/w`, with `extra` over it. */
export function startParams(extra: Partial<SessionStartParams>): SessionStartParams {
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

/** What the options append to Claude Code's system prompt, empty when nothing. */
export function appended(options: Options): string {
  const prompt = options.systemPrompt;
  return typeof prompt === "object" && prompt !== null && "append" in prompt ? (prompt.append ?? "") : "";
}
