// The sidecar proper: JSON-RPC 2.0, one JSON object per line, on stdin (daemon → sidecar) and
// stdout (sidecar → daemon). Logs go to stderr.

import { ClaudeAgent } from "./agents/claude/agent.ts";
import { CodexAgent } from "./agents/codex/agent.ts";
import { createSidecar } from "./app.ts";
import { createLogger } from "./log.ts";
import { SIDECAR_VERSION } from "./version.ts";

const STOP_TIMEOUT_MS = 10_000;

// Nothing but protocol lines may reach stdout, whatever a dependency prints.
console.log = console.error;
console.info = console.error;
console.debug = console.error;

const log = createLogger();
const sidecar = createSidecar({
  agents: [new ClaudeAgent(), new CodexAgent({}, log.child("codex"))],
  write: (line) => {
    process.stdout.write(line);
  },
  log,
});

process.on("unhandledRejection", (reason) => log.error("unhandled rejection", reason));
process.on("uncaughtException", (error) => log.error("uncaught exception", error));

let exiting = false;
async function exit(code: number, reason: string): Promise<void> {
  if (exiting) return;
  exiting = true;
  if (sidecar.sessions.size > 0) log.info(`${reason}: stopping ${sidecar.sessions.size} session(s)`);
  const deadline = new Promise((resolve) => setTimeout(resolve, STOP_TIMEOUT_MS));
  await Promise.race([sidecar.sessions.stopAll(), deadline]);
  sidecar.peer.close(reason);
  // Let buffered protocol lines reach the daemon before exiting.
  await new Promise<void>((resolve) => process.stdout.write("", () => resolve()));
  log.info(`exiting (${reason})`);
  process.exit(code);
}

process.on("SIGTERM", () => void exit(0, "SIGTERM"));
process.on("SIGINT", () => void exit(0, "SIGINT"));

log.info(`chauffeur-sidecar ${SIDECAR_VERSION} ready (pid ${process.pid})`);
await sidecar.serve(process.stdin);
await exit(0, "stdin closed");
