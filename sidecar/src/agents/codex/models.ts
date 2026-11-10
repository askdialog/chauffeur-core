// Codex's model catalog (`model/list` on the app-server) → AgentModel.

import { homedir } from "node:os";
import type { AgentModel } from "../../protocol.ts";
import { errorMessage, type Logger } from "../../log.ts";
import { RpcError } from "../../rpc/peer.ts";
import { ErrorCode } from "../../protocol.ts";
import { SIDECAR_VERSION } from "../../version.ts";
import type { AppServerHandlers, AppServer } from "./app-server.ts";
import type { CodexSessionDeps } from "./session.ts";
import type { Model } from "./protocol.ts";

/** The service tier Codex's catalog names "Fast" (faster, uses more of the plan). */
export const FAST_SERVICE_TIER = "priority";
const LIST_TIMEOUT_MS = 30_000;

/** `GPT-6.1-Sol` → `GPT-6.1 Sol`, as Codex's own UI writes it. */
export function codexDisplayName(name: string): string {
  return name.replace(/(\d)-(?=[A-Z][a-z])/g, "$1 ");
}

export function codexModels(models: Model[]): AgentModel[] {
  return models
    .filter((model) => !model.hidden)
    .map((model) => ({
      id: model.model,
      resolved: model.model,
      name: codexDisplayName(model.displayName),
      description: model.description,
      efforts: model.supportedReasoningEfforts.map((option) => option.reasoningEffort),
      default_effort: model.defaultReasoningEffort,
      supports_fast: model.serviceTiers.some((tier) => tier.id === FAST_SERVICE_TIER),
      is_default: model.isDefault,
    }));
}

/** Starts an app-server just to page through `model/list`, then ends it. */
export async function listCodexModels(deps: CodexSessionDeps, log: Logger): Promise<AgentModel[]> {
  return probeCodex(deps, log, homedir(), "list its models", async (server) => {
    const models: Model[] = [];
    let cursor: string | null = null;
    do {
      const page: { data: Model[]; nextCursor: string | null } = await server.request("model/list", { cursor, includeHidden: false });
      models.push(...page.data);
      cursor = page.nextCursor;
    } while (cursor);
    return codexModels(models);
  });
}

/** Starts an app-server in `cwd`, initializes it, runs `ask` on it, then ends it. */
export async function probeCodex<T>(
  deps: CodexSessionDeps,
  log: Logger,
  cwd: string,
  what: string,
  ask: (server: AppServer) => Promise<T>,
  options: { notification?: AppServerHandlers["notification"]; timeoutMs?: number } = {},
): Promise<T> {
  const server = deps.spawn({
    executable: deps.resolveExecutable(deps.baseEnv),
    args: [],
    cwd,
    env: deps.baseEnv,
    handlers: {
      notification: options.notification ?? (() => {}),
      request: async () => {
        throw new RpcError(ErrorCode.METHOD_NOT_FOUND, `not supported while asking Codex to ${what}`);
      },
    },
    log,
  });
  let timer: ReturnType<typeof setTimeout> | undefined;
  const asking = (async () => {
    await server.request("initialize", {
      clientInfo: { name: "chauffeur", title: "Chauffeur", version: SIDECAR_VERSION },
      capabilities: { experimentalApi: false, requestAttestation: false, optOutNotificationMethods: [] },
    });
    server.notify("initialized");
    return ask(server);
  })();
  try {
    return await Promise.race([
      asking,
      server.exited.then((reason) => Promise.reject(new Error(`codex app-server ${reason}`))),
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error(`no answer after ${(options.timeoutMs ?? LIST_TIMEOUT_MS) / 1000}s`)), options.timeoutMs ?? LIST_TIMEOUT_MS);
      }),
    ]);
  } catch (error) {
    throw new Error(`Codex could not ${what}: ${errorMessage(error)}`);
  } finally {
    clearTimeout(timer);
    await server.close();
  }
}
