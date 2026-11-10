import type { AppServer, AppServerHandlers, SpawnOptions } from "../../src/agents/codex/app-server.ts";
import type {
  CodexMethods,
  CommandExecutionRequestApprovalParams,
  FileUpdateChange,
  ServerNotification,
  ServerRequest,
  Turn,
} from "../../src/agents/codex/protocol.ts";

export const THREAD_ID = "thread-1";
export const HOOK = "/opt/chauffeur-sidecar codex-hook pre-tool-use";

type Handler = (params: unknown) => unknown;

/** Stands in for `codex app-server`: answers requests with canned results and lets tests push. */
export class FakeAppServer implements AppServer {
  readonly requests: Array<{ method: string; params: any }> = [];
  readonly notified: string[] = [];
  readonly exited: Promise<string>;
  closed = false;
  private exit!: (reason: string) => void;
  private turns = 0;
  readonly handlers: Partial<Record<keyof CodexMethods, Handler>> = {};

  constructor(readonly options: SpawnOptions) {
    this.exited = new Promise((resolve) => {
      this.exit = resolve;
    });
  }

  async request<M extends keyof CodexMethods>(method: M, params: CodexMethods[M][0]): Promise<CodexMethods[M][1]> {
    this.requests.push({ method, params });
    const handler = this.handlers[method];
    if (handler) return (await handler(params)) as CodexMethods[M][1];
    return this.defaultResult(method, params) as CodexMethods[M][1];
  }

  notify(method: "initialized"): void {
    this.notified.push(method);
  }

  async close(): Promise<void> {
    this.closed = true;
    this.exit("exited with code 0");
  }

  /** The process dies on its own. */
  crash(reason = "exited with code 1"): void {
    this.exit(reason);
  }

  get handlersOfSession(): AppServerHandlers {
    return this.options.handlers;
  }

  push(...notifications: ServerNotification[]): void {
    for (const notification of notifications) this.options.handlers.notification(notification);
  }

  /** Sends a server request (an approval…) and resolves with the session's answer. */
  ask(request: ServerRequest): Promise<unknown> {
    return this.options.handlers.request(request);
  }

  sent(method: string): any[] {
    return this.requests.filter((r) => r.method === method).map((r) => r.params);
  }

  /** Where `thread/read` says the thread's rollout is. */
  rolloutPath: string | null = null;
    /** What `model/list` answers with. */
  models: unknown[] = [];
  /** What `skills/list` answers with, for every cwd asked. */
  skills: unknown[] = [];
  /** What `account/rateLimits/read` answers with. */
  rateLimits: unknown = null;
  /** What `account/read` answers with. */
  account: unknown = { account: { type: "chatgpt", email: "dev@example.com", planType: "pro" }, requiresOpenaiAuth: true, workspaceRouting: null };
  /** The thread's MCP servers, as `mcpServerStatus/list` gives them. */
  mcpServers: unknown[] = [];
  /** What `config/read` answers with. */
  config: unknown = { config: {}, origins: {}, layers: [] };
  /** What `config/read` answers with for these cwds (a repo's worktree), instead of `config`. */
  configByCwd: Record<string, unknown> = {};

  private defaultResult(method: keyof CodexMethods, params: any): unknown {
    switch (method) {
      case "initialize":
        return { userAgent: "chauffeur/0.162.1 (Mac OS; arm64)", codexHome: "/home/.codex", platformFamily: "unix", platformOs: "macos" };
      case "hooks/list":
        return {
          data: [
            {
              cwd: params.cwds[0],
              warnings: [],
              errors: [],
              hooks: [
                { key: "/<session-flags>/config.toml:pre_tool_use:0:0", source: "sessionFlags", handlerType: "command", command: HOOK, currentHash: "sha256:abc", trustStatus: "untrusted" },
                { key: "/repo/.codex/config.toml:pre_tool_use:0:0", source: "project", handlerType: "command", command: "eslint", currentHash: "sha256:def", trustStatus: "untrusted" },
              ],
            },
          ],
        };
      case "thread/start":
      case "thread/resume":
        return { thread: { id: params.threadId ?? THREAD_ID }, model: params.model ?? "gpt-6.1-sol" };
      case "thread/fork":
        return { thread: { id: `${params.threadId}-fork` }, model: "gpt-6.1-sol" };
      case "turn/start":
        return { turn: turn(`turn-${++this.turns}`, "inProgress") };
      case "turn/steer":
        return { turnId: params.expectedTurnId };
      case "turn/interrupt":
      case "thread/compact/start":
      case "thread/inject_items":
        return {};
      case "thread/read":
        return { thread: { id: params.threadId, path: this.rolloutPath } };
      case "model/list":
        return { data: this.models, nextCursor: null };
      case "account/rateLimits/read":
        return this.rateLimits;
      case "account/read":
        return this.account;
      case "account/login/start":
        if (params.type === "chatgptDeviceCode") {
          return { type: "chatgptDeviceCode", loginId: "device-1", verificationUrl: "https://auth.openai.com/codex/device", userCode: "ABCD-1234" };
        }
        return { type: "chatgpt", loginId: "login-1", authUrl: "https://auth.openai.com/oauth/authorize?state=1" };
      case "account/login/cancel":
        return { status: "canceled" };
      case "mcpServerStatus/list":
        return { data: this.mcpServers, nextCursor: null };
      case "mcpServer/oauth/login":
        return { authorizationUrl: `https://mcp.example.com/authorize?server=${params.name}`, loginId: "mcp-login-1" };
      case "config/mcpServer/reload":
        return {};
      case "config/read":
        return Object.hasOwn(this.configByCwd, params.cwd ?? "") ? this.configByCwd[params.cwd] : this.config;
      case "skills/extraRoots/set":
        return {};
      case "skills/list":
        return { data: (params.cwds as string[]).map((cwd) => ({ cwd, skills: this.skills, errors: [] })) };
    }
  }
}

export function turn(id: string, status: Turn["status"], extra: Partial<Turn> = {}): Turn {
  return { id, rootTurnId: id, items: [], itemsView: "notLoaded", status, error: null, startedAt: 1, completedAt: null, durationMs: null, ...extra };
}

export const codex = {
  turnStarted(id: string): ServerNotification {
    return { method: "turn/started", params: { threadId: THREAD_ID, turn: turn(id, "inProgress") } };
  },
  turnCompleted(id: string, status: Turn["status"] = "completed", extra: Partial<Turn> = {}): ServerNotification {
    return { method: "turn/completed", params: { threadId: THREAD_ID, turn: turn(id, status, { durationMs: 1500, ...extra }) } };
  },
  agentMessage(id: string, text: string, turnId = "turn-1"): ServerNotification {
    return {
      method: "item/completed",
      params: {
        threadId: THREAD_ID,
        turnId,
        completedAtMs: 1,
        item: { type: "agentMessage", id, text, phase: "final_answer", memoryCitation: null, delivery: null, questions: null },
      },
    };
  },
  fileChangeStarted(id: string, turnId = "turn-1", changes: FileUpdateChange[] = [{ path: "/w/a.txt", kind: { type: "update", move_path: null }, diff: "-a\n+b\n" }]): ServerNotification {
    return {
      method: "item/started",
      params: { threadId: THREAD_ID, turnId, startedAtMs: 1, item: { type: "fileChange", id, status: "inProgress", changes } },
    };
  },
  serverRequestResolved(requestId: number): ServerNotification {
    return { method: "serverRequest/resolved", params: { threadId: THREAD_ID, requestId } };
  },
  commandApproval(
    id: number,
    command: string,
    reason: string | null = null,
    turnId = "turn-1",
    extra: Partial<CommandExecutionRequestApprovalParams> = {},
  ): ServerRequest {
    return {
      method: "item/commandExecution/requestApproval",
      id,
      params: { kind: "command", threadId: THREAD_ID, turnId, itemId: `exec-${id}`, startedAtMs: 1, environmentId: "local", reason, command, cwd: "/w", ...extra },
    };
  },
  userInput(id: number, questions: Array<{ id: string; header: string; question: string; options: Array<{ label: string; description: string }> | null }>): ServerRequest {
    return {
      method: "item/tool/requestUserInput",
      id,
      params: {
        threadId: THREAD_ID,
        turnId: "turn-1",
        itemId: `ask-${id}`,
        questions: questions.map((q) => ({ ...q, isOther: true, isSecret: false })),
        isBlocking: true,
        autoResolutionMs: null,
      },
    };
  },
  fileApproval(id: number, itemId: string, reason: string | null = null): ServerRequest {
    return { method: "item/fileChange/requestApproval", id, params: { threadId: THREAD_ID, turnId: "turn-1", itemId, startedAtMs: 1, reason } };
  },
};
