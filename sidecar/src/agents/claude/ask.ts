// One question in one turn (`agent.ask`): Find routines' analysis. No tools, no settings, no
// transcript, as titles and design briefs are asked; the model and effort the user chose.

import { homedir } from "node:os";
import type { Options, SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";
import { errorMessage } from "../../log.ts";
import type { AgentAskParams } from "../../protocol.ts";
import { AsyncQueue } from "../../util/async-queue.ts";
import { SIDECAR_VERSION } from "../../version.ts";
import { effortLevel } from "./options.ts";
import type { ClaudeSessionDeps } from "./session.ts";

export async function claudeAsk(deps: ClaudeSessionDeps, params: AgentAskParams): Promise<string> {
  // Checked before any process starts: a bad effort is the caller's error.
  const effort = params.effort ? effortLevel(params.effort) : undefined;
  const abortController = new AbortController();
  const executable = deps.resolveExecutable();
  const input = new AsyncQueue<SDKUserMessage>();
  input.push({ type: "user", message: { role: "user", content: params.request }, parent_tool_use_id: null });
  input.end();
  const options: Options = {
    systemPrompt: params.instructions,
    tools: [],
    settingSources: [],
    persistSession: false,
    maxTurns: 1,
    cwd: homedir(),
    env: { CLAUDE_AGENT_SDK_CLIENT_APP: `chauffeur-sidecar/${SIDECAR_VERSION}`, ...deps.baseEnv },
    abortController,
    ...(params.model ? { model: params.model } : {}),
    ...(effort ? { effort } : {}),
    ...(executable ? { pathToClaudeCodeExecutable: executable } : {}),
  };
  const query = deps.query({ prompt: input, options });
  let timer: ReturnType<typeof setTimeout> | undefined;
  const answer = (async () => {
    for await (const message of query) {
      if (message.type !== "result") continue;
      if (message.subtype !== "success") throw new Error(message.subtype);
      return message.result.trim();
    }
    throw new Error("no answer");
  })();
  try {
    const text = await Promise.race([
      answer,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error(`no answer after ${Math.round(params.timeout_ms / 1000)}s`)), params.timeout_ms);
      }),
    ]);
    if (!text) throw new Error("an empty answer");
    return text;
  } catch (error) {
    throw new Error(`Claude Code could not answer: ${errorMessage(error)}`);
  } finally {
    clearTimeout(timer);
    abortController.abort();
  }
}
