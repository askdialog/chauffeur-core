// A chat forked into a new tab: Codex's thread copied into a new one (`thread/fork`) by an
// app-server started just for it, so that resuming the new id carries the conversation on
// without touching the original's. A fork from a turn copies the thread through that turn.

import type { Logger } from "../../log.ts";
import type { AgentForkParams } from "../../protocol.ts";
import { probeCodex } from "./models.ts";
import type { CodexSessionDeps } from "./session.ts";

export function codexFork(deps: CodexSessionDeps, log: Logger, params: AgentForkParams): Promise<string> {
  // The chat's own environment, as its sessions get it: `CODEX_HOME` may be set there.
  const env = { ...deps.baseEnv, ...params.env };
  return probeCodex({ ...deps, baseEnv: env }, log, params.cwd, "fork its thread", async (server) => {
    const response = await server.request("thread/fork", {
      threadId: params.session_id,
      ...(params.up_to ? { lastTurnId: params.up_to } : {}),
      excludeTurns: true,
    });
    return response.thread.id;
  });
}
