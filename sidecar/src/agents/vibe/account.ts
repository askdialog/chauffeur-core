// What Chauffeur asks a short-lived `vibe-acp` outside any chat: the model catalog, the plan's
// usage, the login, one-turn questions (titles, briefs), forks. Each starts Vibe in an empty folder
// of its own, initializes it, asks, and ends it. A session opened only to read something is not
// kept by Vibe until it gets a prompt; one that got a prompt is deleted afterwards
// (`_session/delete`), so that none shows in the user's Vibe history.

import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { AgentForkParams, AgentLoginResult, AgentModel, AgentUsageResult, NativeAccountInfo, UsageWindow } from "../../protocol.ts";
import { ErrorCode } from "../../protocol.ts";
import { errorMessage, type Logger } from "../../log.ts";
import { RpcError } from "../../rpc/peer.ts";
import { SIDECAR_VERSION } from "../../version.ts";
import type { AcpConnection } from "../acp/connection.ts";
import { blockText } from "../acp/events.ts";
import { ACP_PROTOCOL_VERSION, type ConfigOption, type NewSessionResponse, type PromptResponse, type SessionNotification } from "../acp/protocol.ts";
import { isSignedOut, VIBE_ASK_MODE, type VibeSessionDeps } from "./session.ts";

const PROBE_TIMEOUT_MS = 30_000;

interface Probe {
  connection: AcpConnection;
  folder: string;
}

/** Starts Vibe in a folder of its own, initializes it, runs `ask`, then ends it. */
export async function probeVibe<T>(
  deps: VibeSessionDeps,
  log: Logger,
  what: string,
  ask: (probe: Probe) => Promise<T>,
  options: { timeoutMs?: number; meta?: Record<string, unknown>; onUpdate?: (notification: SessionNotification) => void } = {},
): Promise<T> {
  const folder = await mkdtemp(join(tmpdir(), "chauffeur-vibe-"));
  const connection = deps.spawn({
    executable: deps.resolveExecutable(deps.baseEnv),
    args: [],
    cwd: folder,
    env: deps.baseEnv,
    log,
    name: "vibe-acp",
    handlers: {
      notification: (method, params) => {
        if (method === "session/update") options.onUpdate?.(params as SessionNotification);
      },
      // Nothing runs a tool here: every call is refused.
      request: async (method, params) => {
        if (method === "session/request_permission") {
          const offered = (params as { options?: { optionId: string; kind: string }[] }).options ?? [];
          const reject = offered.find((option) => option.kind === "reject_once");
          return { outcome: reject ? { outcome: "selected", optionId: reject.optionId } : { outcome: "cancelled" } };
        }
        throw new RpcError(ErrorCode.METHOD_NOT_FOUND, `not supported while asking Mistral Vibe to ${what}`);
      },
    },
  });
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeoutMs = options.timeoutMs ?? PROBE_TIMEOUT_MS;
  try {
    return await Promise.race([
      (async () => {
        await initialize(connection, options.meta);
        return ask({ connection, folder });
      })(),
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error(`Mistral Vibe did not ${what} within ${timeoutMs / 1000}s`)), timeoutMs);
      }),
      connection.exited.then((reason) => {
        throw new Error(`vibe-acp ${reason} before it could ${what}`);
      }),
    ]);
  } finally {
    clearTimeout(timer);
    await connection.close().catch((error: unknown) => log.debug(`closing vibe-acp: ${errorMessage(error)}`));
    await rm(folder, { recursive: true, force: true }).catch(() => {});
  }
}

export async function initialize(connection: AcpConnection, meta?: Record<string, unknown>): Promise<void> {
  await connection.request("initialize", {
    protocolVersion: ACP_PROTOCOL_VERSION,
    clientCapabilities: { fs: { readTextFile: false, writeTextFile: false }, terminal: false, ...(meta ? { _meta: meta } : {}) },
    clientInfo: { name: "chauffeur", title: "Chauffeur", version: SIDECAR_VERSION },
  });
}

/** Vibe's models, from a session's `model` option, with the efforts of its `thinking` option. */
export function vibeModels(options: ConfigOption[]): AgentModel[] {
  const model = options.find((option) => option.id === "model");
  const thinking = options.find((option) => option.id === "thinking");
  const efforts = thinking?.options.map((option) => option.value) ?? [];
  return (model?.options ?? []).map((option) => ({
    id: option.value,
    resolved: option.value,
    name: option.name,
    description: option.description && option.description !== option.value ? option.description : "",
    efforts,
    default_effort: thinking?.currentValue ?? null,
    supports_fast: false,
    is_default: option.value === model?.currentValue,
  }));
}

export function listVibeModels(deps: VibeSessionDeps, log: Logger): Promise<AgentModel[]> {
  return probeVibe(deps, log, "list its models", async ({ connection, folder }) => {
    const session = await connection.request<NewSessionResponse>("session/new", { cwd: folder, mcpServers: [] });
    return vibeModels(session.configOptions ?? []);
  });
}

interface AuthStatus {
  authenticated?: boolean;
}

interface AccountView {
  plan?: { kind?: string; name?: string; title?: string | null } | null;
  budget?: { usagePercentage?: number; resetAt?: string | null; paygEnabled?: boolean } | null;
}

/**
 * The plan and its monthly budget, as Vibe reads them (`_account/read`, which needs a session).
 * Vibe has one window, the month: past 100% its usage is billed when pay-as-you-go is on, paused
 * otherwise. With pay-as-you-go the window is labelled so (a label with a space), which the daemon
 * never counts as spent for `delegate`.
 */
export function readVibeUsage(deps: VibeSessionDeps, log: Logger): Promise<AgentUsageResult> {
  try {
    deps.resolveExecutable(deps.baseEnv);
  } catch (error) {
    if (!(error instanceof RpcError) || error.code !== ErrorCode.UNAVAILABLE) throw error;
    return Promise.resolve({ plan: null, windows: [], extra_usage: null, signed_out: null, not_installed: error.message });
  }
  return probeVibe(deps, log, "read its plan's usage", async ({ connection, folder }) => {
    const status = await connection.request<AuthStatus>("_auth/status", {});
    if (!status.authenticated) return signedOut("Mistral Vibe is not signed in");
    let account: AccountView;
    try {
      const session = await connection.request<NewSessionResponse>("session/new", { cwd: folder, mcpServers: [] });
      account = await connection.request<AccountView>("_account/read", { sessionId: session.sessionId });
    } catch (error) {
      if (isSignedOut(error)) return signedOut(errorMessage(error));
      throw error;
    }
    return vibeUsage(account);
  });
}

export function vibeUsage(account: AccountView): AgentUsageResult {
  const budget = account.budget;
  const windows: UsageWindow[] = budget
    ? [
        {
          label: budget.paygEnabled ? "month (pay-as-you-go)" : "month",
          used_percent: Math.max(0, budget.usagePercentage ?? 0),
          resets_at: budget.resetAt ?? null,
          window_secs: 30 * 24 * 60 * 60,
        },
      ]
    : [];
  const plan = account.plan?.title ?? account.plan?.name ?? null;
  return { plan: plan ? plan.toLowerCase() : null, windows, extra_usage: null, signed_out: null };
}

function signedOut(why: string): AgentUsageResult {
  return { plan: null, windows: [], extra_usage: null, signed_out: why };
}

export async function readVibeAccount(deps: VibeSessionDeps, log: Logger): Promise<NativeAccountInfo> {
  try {
    return await probeVibe(deps, log, "read its account", async ({ connection }) => {
      const status = await connection.request<AuthStatus & { authState?: string }>("_auth/status", {});
      if (!status.authenticated) return { mode: "signed_out", email: null, organization: null };
      return { mode: "subscription", email: null, organization: null };
    });
  } catch {
    return { mode: "unknown", email: null, organization: null };
  }
}

/** One answer to one request, in a session refused every tool, deleted afterwards. */
export function vibeAsk(
  deps: VibeSessionDeps,
  log: Logger,
  content: string,
  options: { model?: string | undefined; effort?: string | undefined; timeoutMs?: number } = {},
): Promise<string> {
  let sessionId = "";
  let text = "";
  return probeVibe(
    deps,
    log,
    "answer",
    async ({ connection, folder }) => {
      const session = await connection.request<NewSessionResponse>("session/new", { cwd: folder, mcpServers: [] });
      sessionId = session.sessionId;
      try {
        await connection.request("session/set_mode", { sessionId, modeId: VIBE_ASK_MODE });
        const takes = (id: string, value: string) => session.configOptions?.find((o) => o.id === id)?.options.some((o) => o.value === value) ?? false;
        if (options.model && takes("model", options.model)) await connection.request("session/set_config_option", { sessionId, configId: "model", value: options.model });
        if (options.effort && takes("thinking", options.effort)) await connection.request("session/set_config_option", { sessionId, configId: "thinking", value: options.effort });
        const response = await connection.request<PromptResponse>("session/prompt", { sessionId, prompt: [{ type: "text", text: content }] });
        if (response.stopReason !== "end_turn") throw new Error(`Mistral Vibe stopped: ${response.stopReason}`);
        return text.trim();
      } finally {
        await connection.request("_session/delete", { sessionId }).catch((error: unknown) => log.debug(`deleting Vibe's session: ${errorMessage(error)}`));
      }
    },
    {
      ...(options.timeoutMs ? { timeoutMs: options.timeoutMs } : {}),
      onUpdate: (notification) => {
        if (notification.sessionId !== sessionId) return;
        const update = notification.update;
        // The last message is the answer: a tool call (refused) starts it over.
        if (update.sessionUpdate === "agent_message_chunk") text += blockText(update.content);
        else if (update.sessionUpdate === "tool_call") text = "";
      },
    },
  );
}

/** Copies a session into a new one (`session/fork`); resolves with the new id. */
export function vibeFork(deps: VibeSessionDeps, log: Logger, params: AgentForkParams): Promise<string> {
  if (params.up_to) {
    return Promise.reject(new RpcError(ErrorCode.BAD_REQUEST, "Mistral Vibe forks a whole conversation, not up to a message"));
  }
  return probeVibe({ ...deps, baseEnv: { ...deps.baseEnv, ...params.env } }, log, "fork the conversation", async ({ connection }) => {
    const forked = await connection.request<NewSessionResponse>("session/fork", { sessionId: params.session_id, cwd: params.cwd, mcpServers: [] });
    return forked.sessionId;
  });
}

interface DelegatedSignIn {
  attemptId?: string;
  signInUrl?: string;
  expiresAt?: string;
  status?: string;
}

let login: { connection: AcpConnection; cancelled: boolean } | undefined;

/**
 * Vibe's own sign-in, delegated: Vibe gives the page to open (`browser-auth-delegated`), then
 * waits for it (`action: complete`). Vibe 3.1.3 gives up when console.mistral.ai answers its
 * polling with 429, keeping the attempt: Chauffeur asks it to complete again until the attempt
 * expires. Vibe stores the key; Chauffeur never sees it.
 */
export async function vibeLogin(deps: VibeSessionDeps, log: Logger, ended: (success: boolean, error: string | null) => void): Promise<AgentLoginResult> {
  vibeLoginCancel();
  const connection = deps.spawn({
    executable: deps.resolveExecutable(deps.baseEnv),
    args: [],
    cwd: tmpdir(),
    env: deps.baseEnv,
    log,
    name: "vibe-acp",
    handlers: {
      notification: () => {},
      request: async (method) => {
        throw new RpcError(ErrorCode.METHOD_NOT_FOUND, `not supported while signing Mistral Vibe in: ${method}`);
      },
    },
  });
  const current = { connection, cancelled: false };
  login = current;
  let started: DelegatedSignIn;
  try {
    await initialize(connection, { "browser-auth-delegated": true });
    const answer = await connection.request<{ _meta?: { "browser-auth-delegated"?: DelegatedSignIn } }>("authenticate", {
      methodId: "browser-auth-delegated",
      action: "start",
    });
    started = answer._meta?.["browser-auth-delegated"] ?? {};
    if (!started.signInUrl || !started.attemptId) throw new Error("Mistral Vibe gave no sign-in page");
  } catch (error) {
    if (login === current) login = undefined;
    void connection.close();
    throw new RpcError(ErrorCode.UNAVAILABLE, `Mistral Vibe could not start its sign-in: ${errorMessage(error)}`);
  }
  const attemptId = started.attemptId;
  const expiresAt = started.expiresAt ? Date.parse(started.expiresAt) : Date.now() + 10 * 60_000;
  void (async () => {
    let error: string | null = null;
    let success = false;
    while (!current.cancelled && Date.now() < expiresAt) {
      try {
        await connection.request("authenticate", { methodId: "browser-auth-delegated", action: "complete", attemptId });
        success = true;
        break;
      } catch (failure) {
        error = errorMessage(failure);
        // Vibe's polling hit console.mistral.ai's rate limit: the attempt is kept, ask again.
        if (!/could not be retrieved|exchange/i.test(error)) break;
        await Bun.sleep(5_000);
      }
    }
    if (login === current) login = undefined;
    await connection.close().catch(() => {});
    if (current.cancelled) ended(false, "cancelled");
    else ended(success, success ? null : (error ?? "the sign-in page expired"));
  })();
  return { url: started.signInUrl };
}

export function vibeLoginCancel(): void {
  if (!login) return;
  login.cancelled = true;
  void login.connection.close();
  login = undefined;
}
