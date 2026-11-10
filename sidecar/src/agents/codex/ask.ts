// One question in one turn (`agent.ask`) through the user's Codex login: an ephemeral, read-only
// thread without tools, on the model and effort the user chose (Codex's defaults otherwise).

import { homedir } from "node:os";
import type { AgentAskParams } from "../../protocol.ts";
import type { Logger } from "../../log.ts";
import { probeCodex } from "./models.ts";
import { threadModel } from "./options.ts";
import type { CodexSessionDeps } from "./session.ts";

export async function codexAsk(deps: CodexSessionDeps, log: Logger, params: AgentAskParams): Promise<string> {
  let resolve!: (answer: string) => void;
  let reject!: (error: Error) => void;
  const answer = new Promise<string>((ok, fail) => { resolve = ok; reject = fail; });
  // A server can finish the turn before turn/start answers. Keep its completion from the start.
  void answer.catch(() => {});
  let text = "";
  let threadId: string | undefined;
  return probeCodex(deps, log, homedir(), "answer", async (server) => {
    const { thread } = await server.request("thread/start", {
      ...threadModel(params.model),
      ephemeral: true,
      cwd: homedir(),
      approvalPolicy: "never",
      sandbox: "read-only",
      baseInstructions: params.instructions + " Never use tools or read files.",
      config: { web_search: "disabled", features: { shell_tool: false }, project_doc_max_bytes: 0 },
    });
    threadId = thread.id;
    await server.request("turn/start", {
      threadId,
      input: [{ type: "text", text: params.request, text_elements: [] }],
      ...(params.effort ? { effort: params.effort } : {}),
    });
    const reply = (await answer).trim();
    if (!reply) throw new Error("an empty answer");
    return reply;
  }, {
    timeoutMs: params.timeout_ms,
    notification: (message) => {
      if (message.method === "item/completed" && message.params.threadId === threadId && message.params.item.type === "agentMessage") {
        text = message.params.item.text;
      }
      if (message.method !== "turn/completed" || message.params.threadId !== threadId) return;
      const turn = message.params.turn;
      if (turn.status !== "completed" || turn.error) reject(new Error(turn.error?.message ?? `turn ${turn.status}`));
      else resolve(text);
    },
  });
}
