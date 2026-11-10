// Maps the daemon's JSON-RPC methods (SidecarMethods) onto the session manager, validating params.
// Agent-agnostic: nothing here knows which agent runs a chat.

import {
  ErrorCode,
  type AgentAskParams,
  type AgentBriefParams,
  type AgentKind,
  type ContextDetail,
  type JevParams,
  type JsonValue,
  type Attachment,
  type PermissionMode,
  type PingResult,
  type PackageCryptParams,
  type RequestResponse,
  type SessionStartParams,
  type SidecarMethods,
} from "./protocol.ts";
import { RpcError } from "./rpc/peer.ts";
import { restoreContexts } from "./agents/context-restore.ts";
import { captureContext } from "./agents/context-capture.ts";
import { openPackage, sealPackage } from "./package-crypto.ts";
import type { SessionManager } from "./sessions.ts";
import { SIDECAR_VERSION } from "./version.ts";

type Handlers = {
  [M in keyof SidecarMethods]: (params: unknown) => Promise<SidecarMethods[M][1]> | SidecarMethods[M][1];
};

export function createDispatcher(
  sessions: SessionManager,
  /** What agents are started with (each agent's `baseEnv`): the sidecar's own environment. */
  environment: Record<string, string | undefined> = process.env,
): (method: string, params: unknown) => unknown {
  const handlers: Handlers = {
    "session.start": (params) => sessions.start(parseStartParams(params)),
    "session.send": (params) => {
      const p = object(params);
      sessions.send(string(p, "chat_id"), text(p), attachments(p.attachments), optionalString(p, "message_id"));
      return {};
    },
    "session.interrupt": async (params) => {
      await sessions.interrupt(string(object(params), "chat_id"));
      return {};
    },
    "session.respond": (params) => {
      const p = object(params);
      sessions.respond(string(p, "chat_id"), string(p, "request_id"), parseResponse(p.response));
      return {};
    },
    "session.stop_task": async (params) => {
      const p = object(params);
      await sessions.stopTask(string(p, "chat_id"), string(p, "task_id"));
      return {};
    },
    "session.context_usage": async (params) => {
      const p = object(params);
      return { usage: await sessions.contextUsage(string(p, "chat_id"), contextDetail(p.detail)) };
    },
    "session.mcp_connect": (params) => {
      const p = object(params);
      return sessions.mcpConnect(string(p, "chat_id"), string(p, "server"));
    },
    "session.configure": async (params) => {
      const p = object(params);
      await sessions.configure(string(p, "chat_id"), {
        model: optionalString(p, "model"),
        effort: optionalString(p, "effort"),
        fast: optionalBoolean(p, "fast"),
        permissionMode: p.permission_mode == null ? undefined : permissionMode(p.permission_mode),
      });
      return {};
    },
    "session.stop": async (params) => {
      const chat = string(object(params), "chat_id");
      await sessions.stop(chat);
      const ids = sessions.confirmedLaunches(chat);
      return ids.length ? { confirmed_launch_ids: ids } : {};
    },
    "session.close_idle": async (params) => {
      const chat = string(object(params), "chat_id");
      const closed = await sessions.closeIdle(chat);
      const ids = closed ? sessions.confirmedLaunches(chat) : [];
      return { closed, ...(ids.length ? { confirmed_launch_ids: ids } : {}) };
    },
    "agent.models": async (params) => ({ models: await sessions.models(agentKind(object(params).agent)) }),
    "agent.commands": async (params) => {
      const p = object(params);
      return { commands: await sessions.commands(agentKind(p.agent), string(p, "cwd"), directories(p.additional_directories)) };
    },
    "agent.title": async (params) => ({ title: await sessions.title(string(object(params), "text")) }),
    "agent.retitle": async (params) => {
      const p = object(params);
      const current = optionalString(p, "current");
      return { title: await sessions.retitle({ conversation: string(p, "conversation"), ...(current ? { current } : {}) }) };
    },
    "agent.brief": async (params) => {
      const p = object(params);
      const brief: AgentBriefParams = { conversation: string(p, "conversation") };
      for (const key of ["title", "model", "effort"] as const) {
        const value = optionalString(p, key);
        if (value) brief[key] = value;
      }
      return { brief: await sessions.brief(brief) };
    },
    "agent.ask": async (params) => {
      const p = object(params);
      const ask: AgentAskParams = {
        agent: agentKind(p.agent),
        instructions: string(p, "instructions"),
        request: string(p, "request"),
        timeout_ms: typeof p.timeout_ms === "number" && p.timeout_ms > 0 ? p.timeout_ms : 600_000,
      };
      for (const key of ["model", "effort"] as const) {
        const value = optionalString(p, key);
        if (value) ask[key] = value;
      }
      return { text: await sessions.ask(ask) };
    },
    "agent.account": (params) => sessions.account(agentKind(object(params).agent)),
    "agent.usage": (params) => sessions.usage(agentKind(object(params).agent)),
    "agent.login": (params) => {
      const p = object(params);
      return sessions.login(agentKind(p.agent), optionalBoolean(p, "headless") ?? false);
    },
    "agent.login_code": async (params) => {
      const p = object(params);
      await sessions.loginCode(agentKind(p.agent), string(p, "code"));
      return null;
    },
    "agent.login_cancel": (params) => {
      sessions.loginCancel(agentKind(object(params).agent));
      return null;
    },
    "agent.subagent_messages": (params) => {
      const p = object(params);
      const agentId = optionalString(p, "agent_id");
      const prompt = optionalString(p, "prompt");
      const result = optionalString(p, "result");
      return sessions.subagentMessages({
        agent: agentKind(p.agent),
        session_id: string(p, "session_id"),
        cwd: string(p, "cwd"),
        ...(agentId ? { agent_id: agentId } : {}),
        ...(prompt !== undefined ? { prompt } : {}),
        ...(result !== undefined ? { result } : {}),
      });
    },
    "agent.fork": (params) => {
      const p = object(params);
      const upTo = optionalString(p, "up_to");
      return sessions.fork({
        agent: agentKind(p.agent),
        session_id: string(p, "session_id"),
        cwd: string(p, "cwd"),
        env: stringMap(p.env ?? {}),
        ...(upTo ? { up_to: upTo } : {}),
      });
    },
    "context.restore": (params) => {
      const p = object(params);
      if (!Array.isArray(p.contexts) || p.contexts.length > 4096) throw new RpcError(ErrorCode.BAD_REQUEST, "Invalid native restoration entries");
      return restoreContexts({ claude_home: string(p, "claude_home"), codex_home: string(p, "codex_home"), contexts: p.contexts.map((value: unknown) => {
        const entry = object(value);
        if (typeof entry.bytes !== "number") throw new RpcError(ErrorCode.BAD_REQUEST, "Invalid native restoration size");
        return { agent: agentKind(entry.agent), session_id: string(entry, "session_id"), cwd: string(entry, "cwd"),
          path: string(entry, "path"), sha256: string(entry, "sha256"), bytes: entry.bytes };
      }) });
    },
    "package.seal": (params) => sealPackage(packageCryptParams(params)),
    "package.open": (params) => openPackage(packageCryptParams(params)),
    "context.capture": (params) => {
      const p = object(params);
      const chat = string(p, "chat_id");
      const live = p.live === true;
      if (!live && sessions.has(chat)) throw new RpcError(ErrorCode.CONFLICT, `chat ${chat} has a running session; stop it before capturing its conversation`);
      return captureContext({
        chat_id: chat, agent: agentKind(p.agent), session_id: string(p, "session_id"),
        cwd: string(p, "cwd"), env: stringMap(p.env ?? {}), output: string(p, "output"), live,
      }, environment);
    },
    "sidecar.ping": (): PingResult => ({ sidecar_version: SIDECAR_VERSION, agents: sessions.versions() }),
    "sidecar.set_env": (params) => {
      replaceEnvironment(environment, stringMap(object(params).env));
      return {};
    },
  };

  return (method, params) => {
    if (!Object.hasOwn(handlers, method)) {
      throw new RpcError(ErrorCode.METHOD_NOT_FOUND, `unknown method ${method}`);
    }
    return handlers[method as keyof Handlers](params ?? {});
  };
}

const AGENTS: readonly AgentKind[] = ["claude", "codex"];
const PERMISSION_MODES: readonly PermissionMode[] = ["auto", "default", "accept_edits", "plan", "bypass_permissions"];

function directories(value: unknown): string[] {
  return strings(value, "additional_directories");
}

function packageCryptParams(value: unknown): PackageCryptParams {
  const p = object(value), context = object(p.context);
  if (typeof context.source_epoch !== "number" || (p.ciphertext_bytes !== undefined && typeof p.ciphertext_bytes !== "number")) {
    throw new RpcError(ErrorCode.BAD_REQUEST, "Invalid workspace archive sizes");
  }
  const digest = optionalString(p, "ciphertext_digest");
  return {
    input: string(p, "input"), output: string(p, "output"), key: string(p, "key"),
    context: {
      workspace_id: string(context, "workspace_id"), owner_peer_id: string(context, "owner_peer_id"),
      controller_peer_id: string(context, "controller_peer_id"), operation_id: string(context, "operation_id"),
      source_peer_id: string(context, "source_peer_id"),
      // A backup has no target.
      target_peer_id: context.backup === true && context.target_peer_id === "" ? "" : string(context, "target_peer_id"),
      source_epoch: context.source_epoch,
      // A backup binds no manifest digest.
      snapshot_digest: context.backup === true && context.snapshot_digest === "" ? "" : string(context, "snapshot_digest"),
      ...(context.backup === true ? { backup: true } : {}),
    },
    ...(digest !== undefined ? { ciphertext_digest: digest } : {}),
    ...(typeof p.ciphertext_bytes === "number" ? { ciphertext_bytes: p.ciphertext_bytes } : {}),
  };
}

function strings(value: unknown, key: string): string[] {
  const list = value ?? [];
  if (!Array.isArray(list) || !list.every((d) => typeof d === "string")) {
    throw invalid(`${key} must be an array of strings`);
  }
  return list;
}

export function parseStartParams(params: unknown): SessionStartParams {
  const p = object(params);
  const agent = agentKind(p.agent);
  const env = p.env ?? {};
  if (typeof env !== "object" || Array.isArray(env) || !Object.values(env).every((v) => typeof v === "string")) {
    throw invalid("env must be an object of strings");
  }
  const start: SessionStartParams = {
    launch_id: optionalString(p, "launch_id"),
    chat_id: string(p, "chat_id"),
    workspace_id: string(p, "workspace_id"),
    agent,
    cwd: string(p, "cwd"),
    additional_directories: directories(p.additional_directories),
    fast: optionalBoolean(p, "fast") ?? false,
    permission_mode: permissionMode(p.permission_mode),
    read_only: optionalBoolean(p, "read_only") ?? false,
    env: env as Record<string, string>,
    prompt_attachments: attachments(p.prompt_attachments),
    prompt_suggestions: optionalBoolean(p, "prompt_suggestions") ?? false,
  };
  if (optionalBoolean(p, "delegate")) start.delegate = true;
  for (const key of ["model", "effort", "plans_dir", "design_dir", "notes_dir", "resume_session_id", "append_system_prompt", "prompt"] as const) {
    const value = optionalString(p, key);
    if (value !== undefined) start[key] = value;
  }
  if (p.jev !== undefined && p.jev !== null) start.jev = jevParams(p.jev);
  const always = strings(p.always_allow, "always_allow");
  if (always.length > 0) start.always_allow = always;
  return start;
}

function jevParams(value: unknown): JevParams {
  const j = object(value, "jev");
  const threshold = j.threshold_percent;
  if (typeof threshold !== "number" || !Number.isInteger(threshold) || threshold < 1 || threshold > 100) {
    throw invalid("jev.threshold_percent must be a whole percentage");
  }
  return { api_key: string(j, "api_key"), threshold_percent: threshold };
}

function parseResponse(value: unknown): RequestResponse {
  const r = object(value, "response");
  switch (r.type) {
    case "allow":
      return { type: "allow", always: r.always === true };
    case "deny":
      return { type: "deny", message: optionalString(r, "message") ?? null };
    case "elicitation": {
      if (r.action !== "accept" && r.action !== "decline" && r.action !== "cancel") {
        throw invalid("response.action must be accept, decline or cancel");
      }
      // The form's own schema validates the content where the confirmation is pending.
      return { type: "elicitation", action: r.action, content: (r.content ?? null) as JsonValue, always: r.always === true };
    }
    case "answer": {
      const answers = r.answers;
      if (typeof answers !== "object" || answers === null || Array.isArray(answers)) {
        throw invalid("response.answers must be an object");
      }
      const result: Record<string, string> = {};
      for (const [question, answer] of Object.entries(answers)) {
        if (typeof answer !== "string") throw invalid(`answer to "${question}" must be a string`);
        result[question] = answer;
      }
      return { type: "answer", answers: result };
    }
    default:
      throw invalid("response.type must be allow, deny, elicitation or answer");
  }
}

/** A message's text may be empty when it carries images. */
function text(p: Record<string, unknown>): string {
  if (typeof p.text !== "string") throw invalid("text must be a string");
  return p.text;
}

function attachments(value: unknown): Attachment[] {
  if (value === undefined || value === null) return [];
  if (!Array.isArray(value)) throw invalid("attachments must be an array");
  return value.map((item) => {
    const a = object(item, "attachment");
    return {
      file: string(a, "file"),
      name: string(a, "name"),
      media_type: string(a, "media_type"),
      size: typeof a.size === "number" ? a.size : 0,
      path: string(a, "path"),
    };
  });
}

function contextDetail(value: unknown): ContextDetail {
  if (value === undefined || value === null) return "summary";
  if (value !== "summary" && value !== "full") throw invalid("detail must be summary or full");
  return value;
}

function agentKind(value: unknown): AgentKind {
  if (typeof value !== "string" || !AGENTS.includes(value as AgentKind)) {
    throw invalid(`agent must be one of ${AGENTS.join(", ")}`);
  }
  return value as AgentKind;
}

function permissionMode(value: unknown): PermissionMode {
  if (typeof value !== "string" || !PERMISSION_MODES.includes(value as PermissionMode)) {
    throw invalid(`permission_mode must be one of ${PERMISSION_MODES.join(", ")}`);
  }
  return value as PermissionMode;
}

function object(value: unknown, name = "params"): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) throw invalid(`${name} must be an object`);
  return value as Record<string, unknown>;
}

function string(p: Record<string, unknown>, key: string): string {
  const value = p[key];
  if (typeof value !== "string" || value === "") throw invalid(`${key} must be a non-empty string`);
  return value;
}

/** Optional fields may arrive absent or null (serde writes `None` as null). */
function optionalString(p: Record<string, unknown>, key: string): string | undefined {
  const value = p[key];
  if (value === undefined || value === null) return undefined;
  if (typeof value !== "string") throw invalid(`${key} must be a string`);
  return value;
}

function optionalBoolean(p: Record<string, unknown>, key: string): boolean | undefined {
  const value = p[key];
  if (value === undefined || value === null) return undefined;
  if (typeof value !== "boolean") throw invalid(`${key} must be a boolean`);
  return value;
}

/** A map of strings to strings; an error names the key, never the value. */
function stringMap(value: unknown): Record<string, string> {
  const map = object(value, "env");
  for (const [key, entry] of Object.entries(map)) {
    if (typeof entry !== "string") throw invalid(`env.${key} must be a string`);
  }
  return map as Record<string, string>;
}

/** Makes `target` hold exactly `env`, in place: whoever holds it sees the new one. */
export function replaceEnvironment(target: Record<string, string | undefined>, env: Record<string, string>): void {
  for (const key of Object.keys(target)) {
    if (!Object.hasOwn(env, key)) delete target[key];
  }
  Object.assign(target, env);
}

function invalid(message: string): RpcError {
  return new RpcError(ErrorCode.INVALID_PARAMS, message);
}
