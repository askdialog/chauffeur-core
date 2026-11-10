// Ephemeral naming turns through the user's Codex login, used when Claude cannot answer.

import { homedir } from "node:os";
import type { AgentRetitleParams } from "../../protocol.ts";
import type { Logger } from "../../log.ts";
import { INSTRUCTIONS, cleanTitle, request, retitleRequest } from "../title.ts";
import { probeCodex } from "./models.ts";
import type { Model } from "./protocol.ts";
import type { CodexSessionDeps } from "./session.ts";

export function codexTitle(deps: CodexSessionDeps, log: Logger, text: string): Promise<string> {
  return ask(deps, log, request(text));
}

export async function codexRetitle(deps: CodexSessionDeps, log: Logger, params: AgentRetitleParams): Promise<string> {
  const title = await ask(deps, log, retitleRequest(params.conversation, params.current));
  const kept = title.toLowerCase() === "keep" || title.toLowerCase() === params.current?.toLowerCase();
  return params.current && kept ? params.current : title;
}

async function ask(deps: CodexSessionDeps, log: Logger, content: string): Promise<string> {
  let resolve!: (answer: string) => void;
  let reject!: (error: Error) => void;
  const answer = new Promise<string>((ok, fail) => { resolve = ok; reject = fail; });
  // A server can finish the turn before turn/start answers. Keep its completion from the start.
  void answer.catch(() => {});
  let text = "";
  let threadId: string | undefined;
  return probeCodex(deps, log, homedir(), "name the work", async (server) => {
    const models: Model[] = [];
    let cursor: string | null = null;
    do {
      const page: { data: Model[]; nextCursor: string | null } = await server.request("model/list", { cursor, includeHidden: false });
      models.push(...page.data);
      cursor = page.nextCursor;
    } while (cursor);
    const model = models.find((m) => m.model === "gpt-6.1-luna") ?? models.find((m) => m.model === "gpt-6-luna");
    if (!model) throw new Error("Luna is not available in Codex's model catalog");
    const { thread } = await server.request("thread/start", {
      model: model.model,
      modelProvider: "openai",
      ephemeral: true,
      cwd: homedir(),
      approvalPolicy: "never",
      sandbox: "read-only",
      baseInstructions: INSTRUCTIONS + " Never use tools or read files.",
      config: { web_search: "disabled", features: { shell_tool: false }, project_doc_max_bytes: 0 },
    });
    threadId = thread.id;
    await server.request("turn/start", {
      threadId,
      input: [{ type: "text", text: content, text_elements: [] }],
      effort: "low",
    });
    const title = cleanTitle(await answer);
    if (!title) throw new Error("an empty title");
    return title;
  }, {
    timeoutMs: 20_000,
    notification: (message) => {
      if (message.method === "item/completed" && message.params.threadId === threadId && message.params.item.type === "agentMessage") {
        text = message.params.item.text;
      }
      if (message.method !== "turn/completed" || message.params.threadId !== threadId) return;
      const turn = message.params.turn;
      if (turn.status !== "completed" || turn.error) reject(new Error(turn.error?.message ?? `naming turn ${turn.status}`));
      else resolve(text);
    },
  });
}
