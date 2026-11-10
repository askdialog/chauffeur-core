// A short title for a piece of work, asked of a small Claude model: names a workspace created
// without ticket or name from its first message, then again from its conversation as the work
// goes on. One turn, no tools, no settings, no transcript.

import { homedir } from "node:os";
import type { SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";
import { errorMessage } from "../../log.ts";
import type { AgentRetitleParams } from "../../protocol.ts";
import { AsyncQueue } from "../../util/async-queue.ts";
import { SIDECAR_VERSION } from "../../version.ts";
import { INSTRUCTIONS, cleanTitle, request, retitleRequest } from "../title.ts";
import type { ClaudeSessionDeps } from "./session.ts";

const TITLE_TIMEOUT_MS = 14_000;

export function claudeTitle(deps: ClaudeSessionDeps, text: string): Promise<string> {
  return ask(deps, request(text));
}

/**
 * The workspace's title again from its conversation: `current` when the model keeps it (KEEP, or
 * the same name in any case), else the new one.
 */
export async function claudeRetitle(deps: ClaudeSessionDeps, params: AgentRetitleParams): Promise<string> {
  const title = await ask(deps, retitleRequest(params.conversation, params.current));
  const kept = title.toLowerCase() === "keep" || title.toLowerCase() === params.current?.toLowerCase();
  return params.current && kept ? params.current : title;
}

async function ask(deps: ClaudeSessionDeps, content: string): Promise<string> {
  const abortController = new AbortController();
  const executable = deps.resolveExecutable();
  const input = new AsyncQueue<SDKUserMessage>();
  input.push({
    type: "user",
    message: { role: "user", content },
    parent_tool_use_id: null,
  });
  input.end();
  const query = deps.query({
    prompt: input,
    options: {
      model: "haiku",
      // A few words: no thinking.
      thinking: { type: "disabled" },
      systemPrompt: INSTRUCTIONS,
      tools: [],
      settingSources: [],
      persistSession: false,
      maxTurns: 1,
      cwd: homedir(),
      env: { CLAUDE_AGENT_SDK_CLIENT_APP: `chauffeur-sidecar/${SIDECAR_VERSION}`, ...deps.baseEnv },
      abortController,
      ...(executable ? { pathToClaudeCodeExecutable: executable } : {}),
    },
  });
  let timer: ReturnType<typeof setTimeout> | undefined;
  const answer = (async () => {
    for await (const message of query) {
      if (message.type === "assistant" && message.error) throw new Error(message.error);
      if (message.type !== "result") continue;
      if (message.subtype !== "success") throw new Error(message.subtype);
      if (message.is_error) throw new Error(message.result || "Claude refused the naming request");
      return cleanTitle(message.result);
    }
    throw new Error("no answer");
  })();
  try {
    const title = await Promise.race([
      answer,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error(`no answer after ${TITLE_TIMEOUT_MS / 1000}s`)), TITLE_TIMEOUT_MS);
      }),
    ]);
    if (!title) throw new Error("an empty title");
    return title;
  } catch (error) {
    throw new Error(`Claude Code could not name the work: ${errorMessage(error)}`);
  } finally {
    clearTimeout(timer);
    abortController.abort();
  }
}
