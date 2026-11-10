// One Codex chat: a `codex app-server` process per session (its environment carries the
// workspace's variables), one thread in it.
//
// Messages sent while a turn runs are steered into it with `turn/steer`: the model reads them at
// its next step, as Claude does with messages queued mid-turn, and as Codex's own TUI does. Only
// the gaps around a turn (its `turn/start` in flight, or a steer that loses the race with the
// turn's end) fall back to a local queue, flushed into the turn once it has an id or into the
// next `turn/start`.
//
// With Jev on, a turn that leaves the context past the threshold is followed by a compaction
// (jev.ts): the thread's history, minus what Jev drops, goes into a new thread that the chat goes
// on in. The turn ends once it is done, messages sent meanwhile wait. `/compact` does the same on
// demand, through Codex's own compaction when Jev is off or fails. The context the new thread holds
// is estimated until Codex's first response in it measures it.

import { settlesWithin } from "../../util/settles-within.ts";
import { basename } from "node:path";
import { errorMessage } from "../../log.ts";
import {
  ErrorCode,
  type Attachment,
  type ChatEvent,
  type JevParams,
  type JsonValue,
  type McpConnectResult,
  type McpServer,
  type McpStatus,
  type PermissionMode,
  type Question,
  type RequestResponse,
  type SessionStartParams,
  type SessionStartResult,
} from "../../protocol.ts";
import { RpcError, type RpcId } from "../../rpc/peer.ts";
import { CODEX_APP_SERVER_VERSION, SIDECAR_VERSION } from "../../version.ts";
import type { AgentSession, SessionChanges, SessionHost } from "../agent.ts";
import { McpTracker } from "../mcp.ts";
import { type CodexConfigRead, type CodexDeclaration, codexDeclaration, codexDeclaredIn, codexMissingEnv, unsetVariables } from "../mcp-env.ts";
import { notice } from "../claude/events.ts";
import { PendingRequests, type Outcome } from "../pending.ts";
import { askMcpElicitation } from "./elicitation.ts";
import { CodexAlwaysAllowed, commandRules, fileRules } from "./always.ts";
import type { AppServer, SpawnAppServer } from "./app-server.ts";
import { FAST_SERVICE_TIER } from "./models.ts";
import { CODEX_TOOLS, CodexEventMapper, shellScript } from "./events.ts";
import type { HookGuard } from "./hook.ts";
import { readThreadHistory } from "./history.ts";
import { compactCodexHistory, compactionSummary, type JevAsker, jevAsker, removedTokens } from "./jev.ts";
import { mergeRepoLayers, readRepoLayers, type RepoLayer, threadOverrides, withRepoLayers } from "./repo-config.ts";
import { DELEGATE } from "../delegate-tool.ts";
import { codexChauffeurTools, codexToolCall } from "./repo-tools.ts";
import { codexSkills, repoSkillRoots, skillInput } from "./skills.ts";
import {
  appServerArgs,
  daemonSocket,
  designSandboxPolicy,
  designThreadConfig,
  LOCAL_CONTEXT_WINDOW,
  localModelName,
  modeSettings,
  OPTED_OUT_NOTIFICATIONS,
  SANDBOX_NOTE,
  sandboxNetworkOpen,
  sandboxPolicy,
  threadConfig,
  threadModel,
  userInputs,
} from "./options.ts";
import type {
  CommandExecutionApprovalDecision,
  FileChangeApprovalDecision,
  McpServerStatus,
  PermissionsRequestApprovalResponse,
  ServerNotification,
  ServerRequest,
  ThreadResumeParams,
  ThreadStartWithToolsParams,
  ToolRequestUserInputResponse,
  TurnStartParams,
  UserInput,
} from "./protocol.ts";

export interface CodexSessionDeps {
  spawn: SpawnAppServer;
  /** The codex binary for this environment. May throw an RpcError. */
  resolveExecutable: (env: Record<string, string | undefined>) => string;
  baseEnv: Record<string, string | undefined>;
  /** The guard hook's command line, or undefined to run without it. */
  hookCommand: (guard: HookGuard | undefined) => string | undefined;
  /** How long `start` waits for the app-server to initialize and open the thread. */
  initTimeoutMs?: number;
  /** Bounds shutdown confirmation; timeout does not release the retained session. */
  stopTimeoutMs?: number;
  newRequestId?: () => string;
  /** Receives every notification and request from the app-server (CHAUFFEUR_SIDECAR_TRACE). */
  trace?: (chatId: string, message: unknown) => void;
  /** Asks Jev, for compactions (TypeSafe's API by default). */
  jevAsker?: (apiKey: string, signal: AbortSignal) => JevAsker;
}

type State = "new" | "starting" | "running" | "stopping" | "ended";

/** A user message waiting for a turn; the id is the daemon's, for `user_message_delivered`. */
type Message = { text: string; attachments: Attachment[]; id?: string | undefined };

type TurnEnded = Extract<ChatEvent, { type: "turn_ended" }>;

/** Points of context a chat must gain after a compaction before Jev compacts it again. */
const RECOMPACT_GROWTH = 10;

function isCompactCommand(message: Message): boolean {
  return message.text.trim() === "/compact" && message.attachments.length === 0;
}

export class CodexSession implements AgentSession {
  private readonly mapper = new CodexEventMapper();
  private readonly requests: PendingRequests;
  /** The rules the user allowed always in this chat (see always.ts). */
  private readonly always: CodexAlwaysAllowed;
  /** App-server request id → the request id the daemon answers. */
  private readonly serverRequests = new Map<RpcId, string>();
  /**
   * Chauffeur's tool calls running, by Codex's request id. Withdrawn by Codex (its turn ended) or
   * by the session's end, they are aborted, but a `delegate` call: its delegated chat works on,
   * its answer to come as a message (`detach`), unless the user interrupted or stopped the chat.
   */
  private readonly toolCalls = new Map<RpcId, { abort: AbortController; detach: AbortController; delegate: boolean }>();
  private server: AppServer | undefined;
  private state: State = "new";
  private stopping: Promise<void> | undefined;
  private threadId = "";
  /** Codex's skills by name → their SKILL.md, for messages starting with `/name`. */
  private skills = new Map<string, string>();
  private failStart: ((error: unknown) => void) | undefined;
  /** The running turn, once Codex gave its id. */
  private activeTurnId: string | null = null;
  /** A `turn/start` is in flight. */
  private turnStarting = false;
  private readonly completedTurns = new Set<string>();
  /** Between our `turn_started` and `turn_ended`. */
  private busy = false;
  /** Messages waiting for a turn id or for the next turn. */
  private readonly queue: Message[] = [];
  private interruptPending = false;
  private mode: PermissionMode;
  private effort: string | undefined;
  /** Settings changed by `session.configure`, sent with the next `turn/start`. */
  private pendingModel: string | undefined;
  private pendingFast: boolean | undefined;
  /** The mode's approval policy and sandbox are to be sent with the next turn (also the first turn of a thread begun in another sandbox). */
  private modeChanged = false;
  private readonly mcp: McpTracker;
  /** The environment Codex was started with. */
  private agentEnv: Record<string, string | undefined> = {};
  /**
   * What Codex's config says of each broken server (its variables' names, its file), read once
   * per session and again after Codex reloads its servers' config. Undefined: not in the config.
   */
  private readonly declarations = new Map<string, CodexDeclaration | undefined>();
  /** What each repo's own `.codex/config.toml` gave the thread (see repo-config.ts). */
  private repoLayers: RepoLayer[] = [];
  /** Told the daemon once that Codex refused its credentials. */
  private accountLost = false;
  /** How the thread was opened, for the thread a compaction moves the chat to. */
  private threadSettings: ThreadStartWithToolsParams | undefined;
  /** The thread runs at fast speed (the chat's setting, as last sent). */
  private fast: boolean;
  /**
   * The compaction running: `abort` cancels Jev's requests; `nativeEnded` takes the end of Codex's
   * own compaction turn, which belongs to the compaction rather than the chat.
   */
  private compaction: { abort: AbortController; nativeEnded?: (event: TurnEnded) => void } | undefined;
  /** Codex compacted on its own in the current turn. */
  private compactedInTurn = false;
  /**
   * The context used (%) where the last compaction left it, below which Jev does not compact again
   * until it grows; `next` while unknown (the next turn's reading tells).
   */
  private compactedAt: number | "next" | undefined;

  constructor(
    private readonly params: SessionStartParams,
    private readonly host: SessionHost,
    private readonly deps: CodexSessionDeps,
  ) {
    this.requests = new PendingRequests((event) => this.emit(event), deps.newRequestId);
    this.always = new CodexAlwaysAllowed(params.always_allow);
    this.mode = params.permission_mode;
    this.effort = params.effort;
    this.fast = params.fast;
    this.mapper.fast = params.fast;
    this.mapper.jevThreshold = params.jev?.threshold_percent;
    this.mapper.local = localModelName(params.model) !== undefined;
    this.mcp = new McpTracker({
      emit: (event) => this.emit(event),
      read: () => this.readMcp(),
      log: (message) => host.log.debug(message),
    });
  }

  async start(): Promise<SessionStartResult> {
    if (this.state !== "new") throw new RpcError(ErrorCode.CONFLICT, "session already started");
    this.state = "starting";
    const env = { ...this.deps.baseEnv, ...this.params.env };
    this.agentEnv = env;
    let server: AppServer;
    let hook: string | undefined;
    try {
      const executable = this.deps.resolveExecutable(env);
      hook = this.deps.hookCommand(this.params.design_dir ? "design" : this.params.read_only ? "read-only" : undefined);
      server = this.deps.spawn({
        executable,
        args: appServerArgs(hook),
        cwd: this.params.cwd,
        env,
        log: this.host.log,
        handlers: {
          notification: (message) => this.onNotification(message),
          request: (message) => this.onRequest(message),
        },
      });
    } catch (error) {
      this.state = "ended";
      throw error instanceof RpcError ? error : new RpcError(ErrorCode.UNAVAILABLE, `Codex failed to start: ${errorMessage(error)}`);
    }
    this.server = server;
    void server.exited.then((reason) => this.onExit(reason));

    const failed = new Promise<never>((_, reject) => {
      this.failStart = reject;
    });
    let timer: ReturnType<typeof setTimeout> | undefined;
    const timeoutMs = this.deps.initTimeoutMs ?? 60_000;
    const timedOut = new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error(`no answer after ${timeoutMs / 1000}s`)), timeoutMs);
    });
    let model: string;
    try {
      ({ model } = await Promise.race([this.openThread(server, hook), failed, timedOut]));
    } catch (error) {
      this.state = "ended";
      void server.close().catch((error: unknown) => this.host.log.error(`Codex startup cleanup remains unconfirmed: ${errorMessage(error)}`));
      throw new RpcError(ErrorCode.UNAVAILABLE, `Codex failed to start: ${errorMessage(error)}`);
    } finally {
      clearTimeout(timer);
      this.failStart = undefined;
    }
    if (this.state !== "starting") throw new RpcError(ErrorCode.UNAVAILABLE, "the session was stopped while starting");
    this.state = "running";
    this.mapper.model = model || null;
    this.emit({ type: "session_started", agent_session_id: this.threadId, model: model || null });
    this.mcp.start();
    if (this.params.prompt || this.params.prompt_attachments.length > 0) {
      this.queue.unshift({ text: this.params.prompt ?? "", attachments: this.params.prompt_attachments });
    }
    this.flushQueue();
    return { agent_session_id: this.threadId, ...(server.pid === undefined ? {} : { native_pid: server.pid }) };
  }

  send(text: string, attachments: Attachment[] = [], messageId?: string): void {
    if (this.state === "stopping" || this.state === "ended") {
      throw new RpcError(ErrorCode.NOT_FOUND, "the session is stopping");
    }
    const message: Message = { text, attachments, id: messageId };
    if (this.state !== "running" || this.compaction) {
      this.queue.push(message);
    } else if (isCompactCommand(message)) {
      // Not steered into a turn: it runs once the turn ends.
      this.queue.push(message);
      if (!this.busy && !this.activeTurnId && !this.turnStarting) this.flushQueue();
    } else if (this.activeTurnId) {
      void this.steer(message, this.activeTurnId);
    } else if (this.turnStarting) {
      this.queue.push(message);
    } else {
      void this.startTurn([message]);
    }
  }

  idle(): boolean {
    return (
      this.state === "running" &&
      !this.busy &&
      !this.activeTurnId &&
      !this.turnStarting &&
      !this.compaction &&
      this.queue.length === 0 &&
      !this.requests.waiting()
    );
  }

  /**
   * Cancels the open requests (an interrupt, the turn's end, a stop) and Chauffeur's tool calls,
   * whose delegated chats are interrupted only by the user's interrupt or stop (`interrupt`).
   */
  private cancelRequests(delegated: "interrupt" | "detach"): void {
    this.requests.cancelAll();
    for (const id of this.toolCalls.keys()) this.withdrawToolCall(id, delegated);
  }

  /** A tool call Codex no longer waits for: aborted, or for `delegate`, as `cancelRequests` says. */
  private withdrawToolCall(id: RpcId, delegated: "interrupt" | "detach"): void {
    const call = this.toolCalls.get(id);
    if (!call) return;
    if (call.delegate && delegated === "detach") call.detach.abort();
    else call.abort.abort();
  }

  async interrupt(): Promise<void> {
    this.cancelRequests("interrupt");
    this.compaction?.abort.abort();
    if (this.state !== "running" || !this.server) return;
    if (this.activeTurnId) {
      try {
        await this.server.request("turn/interrupt", { threadId: this.threadId, turnId: this.activeTurnId });
      } catch (error) {
        throw new RpcError(ErrorCode.INTERNAL, `interrupt failed: ${errorMessage(error)}`);
      }
    } else if (this.turnStarting || this.compaction?.nativeEnded) {
      // Interrupted once Codex says which turn it started.
      this.interruptPending = true;
    }
  }

  respond(requestId: string, response: RequestResponse): void {
    this.requests.respond(requestId, response);
  }

  /** Codex reports no task of its own that could be stopped apart from its turn. */
  async stopTask(): Promise<void> {
    throw new RpcError(ErrorCode.BAD_REQUEST, "Codex has no task to stop: interrupt the chat instead");
  }

  async configure(changes: SessionChanges): Promise<void> {
    if (this.state !== "running") throw new RpcError(ErrorCode.CONFLICT, "the session is not running");
    if (changes.effort !== undefined && changes.effort.trim() === "") {
      throw new RpcError(ErrorCode.INVALID_PARAMS, "effort must not be empty");
    }
    if (changes.model !== undefined) this.pendingModel = changes.model;
    if (changes.effort !== undefined) this.effort = changes.effort;
    if (changes.fast !== undefined) this.pendingFast = changes.fast;
    if (changes.permissionMode !== undefined && changes.permissionMode !== this.mode) {
      this.mode = changes.permissionMode;
      this.modeChanged = true;
    }
    if (this.busy) {
      this.emit(notice("info", "Codex applies model, effort and permission changes from the next turn"));
    }
  }

  stop(): Promise<void> {
    if (this.stopping) return this.stopping;
    const stopping = this.stopSession();
    this.stopping = stopping;
    void stopping.catch(() => { if (this.stopping === stopping) this.stopping = undefined; });
    return stopping;
  }

  private async stopSession(): Promise<void> {
    const running = this.state === "running";
    this.state = "stopping";
    this.failStart?.(new Error("the session was stopped while starting"));
    this.cancelRequests("interrupt");
    this.compaction?.abort.abort();
    if (running && this.busy) this.endTurnLocally("interrupted", null);
    this.queue.length = 0;
    this.mcp.stop();
    const server = this.server;
    if (server && !(await settlesWithin((async () => { await server.close(); await server.exited; })(), this.deps.stopTimeoutMs ?? 15_000))) {
      throw new RpcError(ErrorCode.UNAVAILABLE, "Codex process exit is unconfirmed; keep the workspace fenced and retry stopping the session");
    }
    this.state = "ended";
  }

  /** The thread's MCP servers as Codex reports them. */
  private async readMcp(): Promise<McpServer[]> {
    const server = this.server;
    if (!server || !this.threadId) return [];
    const statuses: McpServerStatus[] = [];
    let cursor: string | null = null;
    do {
      const page: { data: McpServerStatus[]; nextCursor: string | null } = await server.request("mcpServerStatus/list", { threadId: this.threadId, detail: "toolsAndAuthOnly", cursor });
      statuses.push(...page.data);
      cursor = page.nextCursor;
    } while (cursor);
    const unknown = statuses.filter((status) => !status.pluginId && isBrokenStatus(codexStatus(status)) && !this.declarations.has(status.name));
    if (unknown.length > 0) await this.readDeclarations(server, unknown.map((status) => status.name));
    return statuses.map((status) => codexMcpServer(status, this.agentEnv, this.declarations.get(status.name)));
  }

  /**
   * The entries of these servers in Codex's config, as the thread sees it (its cwd: a trusted
   * project's `.codex/config.toml` over the user's). Only their variables' names and their file are
   * kept, never a value.
   */
  private async readDeclarations(server: AppServer, names: string[]): Promise<void> {
    let read: CodexConfigRead | undefined;
    try {
      read = (await server.request("config/read", { includeLayers: true, cwd: this.params.cwd })) as CodexConfigRead;
    } catch (error) {
      this.host.log.debug(`reading Codex's config: ${errorMessage(error)}`);
    }
    // The repos' layers are the thread's overrides, which `config/read` does not see.
    const seen = read ? withRepoLayers(read, this.repoLayers) : undefined;
    for (const name of names) this.declarations.set(name, seen ? codexDeclaration(name, seen, this.agentEnv) : undefined);
  }

  /** Codex read its servers' config again: so does the next reading of their variables. */
  private async reloadMcpConfig(server: AppServer): Promise<void> {
    this.declarations.clear();
    await server.request("config/mcpServer/reload", undefined);
  }

  async mcpConnect(name: string): Promise<McpConnectResult> {
    const server = this.server;
    if (!server || this.state !== "running") throw new RpcError(ErrorCode.CONFLICT, "the session is not running");
    await this.mcp.refresh();
    const known = this.mcp.find(name);
    if (!known) throw new RpcError(ErrorCode.NOT_FOUND, `no MCP server ${name} in this chat`);
    if (known.status === "needs_auth") {
      const { authorizationUrl } = await server.request("mcpServer/oauth/login", { name, threadId: this.threadId });
      // Codex says when the login completes (`mcpServer/oauthLogin/completed`); reading the server
      // meanwhile also catches it.
      this.mcp.watchLogin(name);
      return { auth_url: authorizationUrl };
    }
    try {
      await this.reloadMcpConfig(server);
    } finally {
      await this.mcp.refresh();
    }
    return { auth_url: null };
  }

  /** Codex refused its credentials in a turn: the daemon marks Codex signed out (once a session). */
  private noteAccount(message: ServerNotification): void {
    if (this.accountLost) return;
    let error: { message: string; codexErrorInfo: unknown } | null | undefined;
    if (message.method === "error") error = message.params.error;
    if (message.method === "turn/completed") error = message.params.turn.error;
    if (!error || error.codexErrorInfo !== "unauthorized") return;
    this.accountLost = true;
    this.emit({ type: "account_lost", message: error.message || "Codex's login was refused" });
  }

  /** initialize → initialized → (hooks/list) → thread/start or thread/resume. */
  private async openThread(server: AppServer, hook: string | undefined): Promise<{ model: string }> {
    const init = await server.request("initialize", {
      clientInfo: { name: "chauffeur", title: "Chauffeur", version: SIDECAR_VERSION },
      // The experimental API, for the thread's dynamic tools (Chauffeur's repo tools).
      capabilities: { experimentalApi: true, requestAttestation: false, optOutNotificationMethods: OPTED_OUT_NOTIFICATIONS },
    });
    const version = /\/(\d+\.\d+\.\d+)/.exec(init.userAgent)?.[1];
    if (version && version !== CODEX_APP_SERVER_VERSION) {
      this.host.log.warn(`codex ${version} differs from the protocol types (generated from ${CODEX_APP_SERVER_VERSION})`);
    }
    server.notify("initialized");
    const trust = hook ? await this.trustHook(server, hook) : undefined;
    await this.addRepoSkills(server);
    // A local model's Codex runs apart from the user's config (the daemon gives it a CODEX_HOME of
    // its own), which trusts no repo: their layers, mostly MCP servers, are left out rather than
    // said untrusted in the chat.
    const repoConfig = this.mapper.local ? {} : await this.repoConfig(server);
    const socket = await this.daemonSocket(server, repoConfig);
    const design = this.params.design_dir;
    const { approvalPolicy, sandbox, approvalsReviewer } = modeSettings(this.mode, this.params.read_only, !!design);
    // A thread that may not leave its sandbox is not told to ask.
    const instructions = [this.params.append_system_prompt, this.params.read_only || design ? undefined : SANDBOX_NOTE].filter(Boolean).join("\n\n");
    // With the daemon's socket, the thread begins in the sandbox that starts Codex's network proxy,
    // and its first turn sets the mode's own (see daemonSocketConfig). A design chat's first turn
    // sets its sandbox too, which a thread can only be given by name.
    if (socket || design) this.modeChanged = true;
    const common = {
      cwd: this.params.cwd,
      approvalPolicy,
      approvalsReviewer,
      sandbox: socket ? "workspace-write" : sandbox,
      config: {
        ...threadOverrides(repoConfig, {
          ...threadConfig(design ? [] : this.params.additional_directories, trust, this.params.read_only, socket),
          ...(design ? designThreadConfig(design) : {}),
        }),
        // Codex has no metadata for a local model: it would assume a hosted model's window.
        ...(this.mapper.local ? { model_context_window: LOCAL_CONTEXT_WINDOW } : {}),
      },
      ...threadModel(this.params.model),
      ...(this.params.fast ? { serviceTier: FAST_SERVICE_TIER } : {}),
      ...(instructions ? { developerInstructions: instructions } : {}),
    };
    // A design chat adds no repo and delegates nothing: no Chauffeur tools.
    this.threadSettings = { ...common, dynamicTools: design ? [] : codexChauffeurTools({ delegate: this.params.delegate }) };
    const response = this.params.resume_session_id
      ? await server.request("thread/resume", { threadId: this.params.resume_session_id, excludeTurns: true, ...common } satisfies ThreadResumeParams)
      : await server.request("thread/start", this.threadSettings);
    this.threadId = response.thread.id;
    await this.loadSkills(server);
    // A local model keeps its `ollama/` id: Codex reports Ollama's name alone.
    return { model: this.mapper.local && this.params.model ? this.params.model : response.model };
  }

  /**
   * The daemon's socket to allow in the thread's sandbox, for `chauffeur` commands: none in an
   * exploration (no `CHAUFFEUR_HOME` in its environment), nor when the user's config or a repo's
   * already gives the sandbox the network, which reaches the socket as it is.
   */
  private async daemonSocket(server: AppServer, repoConfig: Record<string, JsonValue>): Promise<string | undefined> {
    const socket = daemonSocket(this.params.env, this.params.read_only || !!this.params.design_dir);
    if (!socket || sandboxNetworkOpen(repoConfig)) return undefined;
    try {
      const read = (await server.request("config/read", { includeLayers: false, cwd: this.params.cwd })) as CodexConfigRead;
      if (sandboxNetworkOpen(read.config)) return undefined;
    } catch (error) {
      this.host.log.debug(`reading Codex's config: ${errorMessage(error)}`);
    }
    return socket;
  }

  /**
   * Codex only runs hooks the user trusted. Chauffeur's own hook comes from the app-server's
   * command line, so the sidecar trusts that exact hook (by the hash Codex computed) for this
   * thread, and no other.
   */
  private async trustHook(server: AppServer, hook: string): Promise<Record<string, JsonValue> | undefined> {
    try {
      const listed = await server.request("hooks/list", { cwds: [this.params.cwd] });
      const trust: Record<string, JsonValue> = {};
      for (const entry of listed.data) {
        for (const metadata of entry.hooks) {
          if (metadata.source === "sessionFlags" && metadata.handlerType === "command" && metadata.command === hook) {
            trust[metadata.key] = { trusted_hash: metadata.currentHash };
          }
        }
      }
      if (Object.keys(trust).length > 0) return trust;
      this.host.log.warn("Codex did not list Chauffeur's PreToolUse hook");
    } catch (error) {
      this.host.log.warn(`hooks/list failed: ${errorMessage(error)}`);
    }
    this.emit(notice("warning", "Chauffeur's `git worktree add` guard is not active for this Codex chat; the workspace instructions still forbid it"));
    return undefined;
  }

  private async startTurn(messages: Message[]): Promise<void> {
    const server = this.server;
    if (!server || this.state !== "running") return;
    this.turnStarting = true;
    // Announced now, as Claude does when it hands a message over: by the time `session.send`
    // (or `session.start` with a prompt) answers, the chat is working.
    this.beginTurn();
    const params: TurnStartParams = {
      threadId: this.threadId,
      input: messages.flatMap((message) => this.inputsOf(message)),
      // Reasoning summaries become `thinking` events.
      summary: "auto",
      ...(this.effort ? { effort: this.effort } : {}),
      ...(this.pendingModel !== undefined ? { model: localModelName(this.pendingModel) ?? (this.pendingModel || null) } : {}),
      // null clears the thread's tier: standard speed.
      ...(this.pendingFast !== undefined ? { serviceTier: this.pendingFast ? FAST_SERVICE_TIER : null } : {}),
      ...(this.modeChanged
        ? {
            approvalPolicy: modeSettings(this.mode, this.params.read_only, !!this.params.design_dir).approvalPolicy,
            approvalsReviewer: modeSettings(this.mode, this.params.read_only, !!this.params.design_dir).approvalsReviewer,
            sandboxPolicy: this.params.design_dir
              ? designSandboxPolicy(this.params.design_dir)
              : sandboxPolicy(this.mode, [this.params.cwd, ...this.params.additional_directories], this.params.read_only),
          }
        : {}),
    };
    try {
      const { turn } = await server.request("turn/start", params);
      this.delivered(messages);
      if (this.pendingModel) this.mapper.model = this.pendingModel;
      if (this.pendingFast !== undefined) this.fast = this.pendingFast;
      this.pendingModel = undefined;
      this.pendingFast = undefined;
      this.modeChanged = false;
      if (!this.activeTurnId && !this.completedTurns.has(turn.id)) this.activeTurnId = turn.id;
    } catch (error) {
      if (!this.activeTurnId) this.endTurnLocally("error", `Codex could not start the turn: ${errorMessage(error)}`);
    } finally {
      this.turnStarting = false;
    }
    if (this.activeTurnId) {
      if (this.interruptPending) {
        this.interruptPending = false;
        void this.interrupt().catch((error: unknown) => this.host.log.warn(errorMessage(error)));
      }
      // The turn ended before Codex answered, and a compaction runs now: what waits is for after it.
      if (this.compaction) return;
      const steered = this.queue.findIndex(isCompactCommand);
      for (const message of this.queue.splice(0, steered < 0 ? this.queue.length : steered)) void this.steer(message, this.activeTurnId);
    } else {
      this.flushQueue();
    }
  }

  /**
   * Runs what waits: the messages up to the first `/compact` as one turn, or that compaction. Not
   * while a turn or a compaction runs; their end calls it again.
   */
  private flushQueue(): void {
    if (this.state !== "running" || this.busy || this.activeTurnId || this.turnStarting || this.compaction || this.queue.length === 0) return;
    const index = this.queue.findIndex(isCompactCommand);
    if (index !== 0) {
      void this.startTurn(this.queue.splice(0, index < 0 ? this.queue.length : index));
      return;
    }
    const [command] = this.queue.splice(0, 1);
    this.beginTurn();
    if (command) this.delivered([command]);
    void this.compact(undefined);
  }

  /**
   * After a turn that left the context past the Jev threshold, starts compacting and holds the
   * turn's end until it is done. True when it did.
   */
  private compactAfter(ended: TurnEnded, used: number | undefined): boolean {
    const jev = this.params.jev;
    const compactedInTurn = this.compactedInTurn;
    this.compactedInTurn = false;
    if (!jev || used === undefined) return false;
    if (compactedInTurn || this.compactedAt === "next") {
      // Codex compacted on its own, or the reading after Jev's compaction: not twice in a row.
      this.compactedAt = used;
      return false;
    }
    if (ended.outcome !== "success" || this.state !== "running" || this.queue.length > 0) return false;
    if (used < jev.threshold_percent) return false;
    if (this.compactedAt !== undefined && used < this.compactedAt + RECOMPACT_GROWTH) return false;
    this.emit(notice("info", `Compacting the conversation with Jev: ${used}% of the context is used`));
    void this.compact(ended);
    return true;
  }

  /**
   * Compacts the thread: through Jev when it is on, else (or when Jev fails) Codex's own way. Ends
   * the turn it runs in: `ended`, the turn that filled the context, or one of its own (`/compact`).
   */
  private async compact(ended: TurnEnded | undefined): Promise<void> {
    const abort = new AbortController();
    this.compaction = { abort };
    const startedAt = Date.now();
    let failure: string | undefined;
    let after: number | null = null;
    try {
      let jevFailed: string | undefined;
      if (this.params.jev) {
        try {
          await this.compactWithJev(this.params.jev, abort.signal);
          this.compactedAt = "next";
        } catch (error) {
          if (abort.signal.aborted) throw error;
          jevFailed = errorMessage(error);
          this.host.log.warn(`Jev did not compact the Codex thread: ${jevFailed}`);
        }
      }
      if (!this.params.jev || jevFailed !== undefined) {
        if (jevFailed !== undefined) this.emit(notice("warning", `Jev did not compact the conversation (${jevFailed}): Codex compacts it instead`));
        after = await this.compactNatively(abort.signal);
        this.compactedInTurn = false;
        if (this.params.jev) this.compactedAt = "next";
      }
    } catch (error) {
      failure = abort.signal.aborted ? "interrupted" : errorMessage(error);
    } finally {
      this.compaction = undefined;
      // An interrupt meant for the compaction's turn, which never started.
      this.interruptPending = false;
    }
    if (this.state !== "running") return;
    if (failure) this.emit(notice("warning", `The compaction did not finish: ${failure}`));
    this.busy = false;
    this.emit(
      ended
        ? { ...ended, context_tokens: after ?? ended.context_tokens }
        : {
            type: "turn_ended",
            outcome: failure === "interrupted" ? "interrupted" : failure ? "error" : "success",
            duration_ms: Date.now() - startedAt,
            cost_usd: null,
            context_tokens: after,
            error: failure && failure !== "interrupted" ? failure : null,
          },
    );
    this.flushQueue();
  }

  /** Moves the chat to a new thread holding what Jev keeps of this one's history. */
  private async compactWithJev(jev: JevParams, signal: AbortSignal): Promise<void> {
    const server = this.server;
    if (!server || !this.threadSettings) throw new Error("the session is not running");
    const { thread: current } = await untilAborted(server.request("thread/read", { threadId: this.threadId, includeTurns: false }), signal);
    if (!current.path) throw new Error("Codex did not say where the thread is kept");
    const history = await untilAborted(readThreadHistory(current.path), signal);
    const { items, result } = await untilAborted(compactCodexHistory(history, (this.deps.jevAsker ?? jevAsker)(jev.api_key, signal)), signal);
    const model = this.mapper.model ?? this.params.model;
    const { thread } = await untilAborted(
      server.request("thread/start", {
        ...this.threadSettings,
        ...threadModel(model),
        serviceTier: this.fast ? FAST_SERVICE_TIER : null,
      }),
      signal,
    );
    await untilAborted(server.request("thread/inject_items", { threadId: thread.id, items }), signal);
    if (this.state !== "running") return;
    this.threadId = thread.id;
    // The new thread begins in the sandbox it was opened with: its first turn sets the mode's.
    this.modeChanged = true;
    this.emit({ type: "session_started", agent_session_id: thread.id, model: this.mapper.model });
    this.emit(notice("info", compactionSummary(result)));
    for (const event of this.mapper.estimateAfterCompaction(removedTokens(result))) this.emit(event);
    void this.mcp.refresh();
  }

  /**
   * Codex's own compaction of the thread: a turn of its own, whose end is the compaction's (its
   * context tokens, when Codex reported them).
   */
  private async compactNatively(signal: AbortSignal): Promise<number | null> {
    const server = this.server;
    const compaction = this.compaction;
    if (!server || !compaction) throw new Error("the session is not running");
    signal.throwIfAborted();
    const ended = new Promise<TurnEnded>((resolve) => {
      compaction.nativeEnded = resolve;
    });
    // Not raced with an interrupt, which interrupts the compaction's turn (once it started): its
    // end comes here.
    await server.request("thread/compact/start", { threadId: this.threadId });
    const event = await ended;
    if (event.outcome === "interrupted") throw new Error("interrupted");
    if (event.outcome !== "success") throw new Error(event.error ?? "Codex failed to compact");
    return event.context_tokens;
  }

  /**
   * Each repo's own `.codex/config.toml`, as Codex loads it in the repo (trusted repos only), for
   * the thread's config. What Codex leaves out, and clashes between repos, are said in the chat
   * when it starts, not when it resumes.
   */
  private async repoConfig(server: AppServer): Promise<Record<string, JsonValue>> {
    const read = await readRepoLayers(server, this.params.additional_directories);
    const merged = mergeRepoLayers(read.enabled);
    this.repoLayers = merged.given;
    const said: string[] = [
      ...read.untrusted.map(({ dir, reason }) => `${basename(dir)}'s .codex/config.toml is not loaded: Codex does not trust this repo. ${reason}`),
      ...read.failed.map(({ dir, error }) => `${basename(dir)}'s .codex/config.toml is not loaded: ${error}`),
      ...merged.clashes,
    ];
    for (const message of said) {
      this.host.log.warn(message);
      if (!this.params.resume_session_id) this.emit(notice("warning", message));
    }
    return merged.config;
  }

  /** Lets the thread see each repo's own skills (see repoSkillRoots). */
  private async addRepoSkills(server: AppServer): Promise<void> {
    const extraRoots = repoSkillRoots(this.params.additional_directories);
    if (extraRoots.length === 0) return;
    try {
      await server.request("skills/extraRoots/set", { extraRoots });
    } catch (error) {
      this.host.log.warn(`skills/extraRoots/set failed: ${errorMessage(error)}`);
    }
  }

  private async loadSkills(server: AppServer): Promise<void> {
    try {
      const skills = await codexSkills(server, this.params.cwd, this.params.additional_directories);
      this.skills = new Map(skills.map((skill) => [skill.name, skill.path]));
    } catch (error) {
      this.host.log.warn(`skills/list failed: ${errorMessage(error)}`);
    }
  }

  /** A message's inputs, with the skill it runs when it starts with `/name`. */
  private inputsOf(message: Message): UserInput[] {
    const inputs = userInputs(message.text, message.attachments, this.mode, this.params.cwd, this.params.read_only);
    const skill = skillInput(message.text, this.skills);
    return skill ? [skill, ...inputs] : inputs;
  }

  private async steer(message: Message, turnId: string): Promise<void> {
    const server = this.server;
    if (!server) return;
    try {
      await server.request("turn/steer", {
        threadId: this.threadId,
        input: this.inputsOf(message),
        expectedTurnId: turnId,
      });
      this.delivered([message]);
    } catch (error) {
      // Usually the turn ended meanwhile: run the message as the next turn.
      this.host.log.debug(`steer failed, queuing: ${errorMessage(error)}`);
      this.queue.push(message);
      this.flushQueue();
    }
  }

  private delivered(messages: Message[]): void {
    for (const message of messages) {
      if (message.id) this.emit({ type: "user_message_delivered", message_id: message.id });
    }
  }

  private onNotification(message: ServerNotification): void {
    this.deps.trace?.(this.host.chatId, message);
    if (this.state === "ended") return;
    this.noteAccount(message);
    if (ofAnotherThread(message, this.threadId)) return;
    switch (message.method) {
      case "mcpServer/startupStatus/updated":
        if (message.params.threadId === null || message.params.threadId === this.threadId) void this.mcp.refresh();
        break;
      case "mcpServer/oauthLogin/completed":
        if (message.params.success && this.server) {
          // The thread connects with its new login once its servers are reloaded.
          void this.reloadMcpConfig(this.server)
            .catch((error: unknown) => this.host.log.debug(`reloading MCP servers: ${errorMessage(error)}`))
            .then(() => this.mcp.refresh());
        }
        break;
      case "turn/started":
        if (this.pendingModel) this.mapper.model = this.pendingModel;
        this.mapper.fast = this.pendingFast ?? this.fast;
        this.activeTurnId = message.params.turn.id;
        if (this.interruptPending && this.compaction?.nativeEnded) {
          this.interruptPending = false;
          void this.interrupt().catch((error: unknown) => this.host.log.warn(errorMessage(error)));
        }
        break;
      case "serverRequest/resolved": {
        // Codex withdrew a request (turn interrupted or ended): close it on our side too.
        const requestId = this.serverRequests.get(message.params.requestId);
        if (requestId) this.requests.cancel(requestId);
        // An interrupt of the user's has already stopped its delegated chats: this is the turn's end.
        this.withdrawToolCall(message.params.requestId, "detach");
        break;
      }
      case "turn/completed":
        // Requests of the turn resolve before its `turn_ended`, which leaves the chat idle.
        this.cancelRequests("detach");
        break;
      case "item/started":
        if (message.params.item.type === "contextCompaction") this.compactedInTurn = true;
        break;
      default:
        break;
    }
    let used: number | undefined;
    for (const event of this.mapper.map(message)) {
      const nativeEnded = this.compaction?.nativeEnded;
      if (event.type === "turn_started") {
        if (!nativeEnded) this.beginTurn();
        continue;
      }
      if (event.type === "context_usage") used = event.usage.percent;
      if (event.type === "turn_ended") {
        if (nativeEnded) {
          nativeEnded(event);
          continue;
        }
        this.beginTurn();
        if (this.compactAfter(event, used)) continue;
        this.busy = false;
      }
      this.emit(event);
    }
    if (message.method === "turn/completed") {
      // A turn may have used, or lost, a server: read them again.
      if (this.state === "running") void this.mcp.refresh();
      this.completedTurns.add(message.params.turn.id);
      if (this.activeTurnId === message.params.turn.id) this.activeTurnId = null;
      this.interruptPending = false;
      this.flushQueue();
    }
  }

  private async onRequest(message: ServerRequest): Promise<unknown> {
    this.deps.trace?.(this.host.chatId, message);
    switch (message.method) {
      case "item/commandExecution/requestApproval": {
        const p = message.params;
        const input: Record<string, JsonValue> = { command: p.command ? shellScript(p.command) : null, cwd: p.cwd ?? null };
        if (p.kind !== "command") input.kind = p.kind;
        if (p.reason) input.reason = p.reason;
        const outcome = await this.askPermission(message.id, CODEX_TOOLS.command, input, p.reason ?? null, commandRules(p));
        return { decision: commandDecision(outcome) };
      }
      case "item/fileChange/requestApproval": {
        const p = message.params;
        const changes = this.mapper.changesOf(p.itemId);
        const input: Record<string, JsonValue> = {
          changes: changes.map((change) => ({ path: change.path, kind: change.kind.type, diff: change.diff })),
        };
        if (p.reason) input.reason = p.reason;
        if (p.grantRoot) input.grant_root = p.grantRoot;
        const outcome = await this.askPermission(message.id, CODEX_TOOLS.fileChange, input, p.reason ?? null, fileRules(p, changes));
        return { decision: fileDecision(outcome) };
      }
      case "item/permissions/requestApproval": {
        const p = message.params;
        const input = { permissions: p.permissions as JsonValue, cwd: p.cwd, reason: p.reason };
        const outcome = await this.askPermission(message.id, "permissions", input, p.reason);
        const granted: PermissionsRequestApprovalResponse =
          outcome.type === "allow"
            ? {
                permissions: {
                  ...(p.permissions.network ? { network: p.permissions.network } : {}),
                  ...(p.permissions.fileSystem ? { fileSystem: p.permissions.fileSystem } : {}),
                },
                scope: outcome.always ? "session" : "turn",
              }
            : { permissions: {}, scope: "turn" };
        return granted;
      }
      case "item/tool/requestUserInput": {
        // Codex's AskUserQuestion: the same question card, answers sent back by question id.
        const p = message.params;
        const questions: Question[] = p.questions.map((q) => ({
          question: q.question,
          header: q.header,
          multi_select: false,
          options: (q.options ?? []).map((o) => ({ label: o.label, description: o.description })),
        }));
        const outcome = await this.requests.ask("question", (requestId) => {
          this.serverRequests.set(message.id, requestId);
          return { type: "question", request_id: requestId, questions };
        });
        this.serverRequests.delete(message.id);
        const answers: ToolRequestUserInputResponse["answers"] = {};
        if (outcome.type === "answer") {
          for (const q of p.questions) {
            const answer = outcome.answers[q.question];
            if (answer) answers[q.id] = { answers: [answer] };
          }
        } else if (outcome.type === "deny" && outcome.message && this.activeTurnId) {
          void this.steer({ text: outcome.message, attachments: [] }, this.activeTurnId);
        }
        return { answers } satisfies ToolRequestUserInputResponse;
      }
      case "item/tool/call": {
        // Chauffeur's tools; `add_repo` puts the repo to the user, withdrawn if Codex withdraws the call, and
        // `delegate` waits for a delegated chat (`toolCalls` says what becomes of it when Codex stops waiting).
        // Aborted too while it has not asked yet (it first reads the repos from the daemon).
        const call = { abort: new AbortController(), detach: new AbortController(), delegate: message.params.tool === DELEGATE };
        this.toolCalls.set(message.id, call);
        try {
          return await codexToolCall(message.params, this.host, this.requests, {
            signal: call.abort.signal,
            detach: call.detach.signal,
            asked: (requestId) => this.serverRequests.set(message.id, requestId),
          });
        } finally {
          this.toolCalls.delete(message.id);
          this.serverRequests.delete(message.id);
        }
      }
      case "mcpServer/elicitation/request":
        return askMcpElicitation(message.params, this.requests, this.always, (requestId) => {
          if (requestId) this.serverRequests.set(message.id, requestId);
          else this.serverRequests.delete(message.id);
        });
      default:
        this.host.log.warn(`unhandled Codex request ${message.method}`);
        throw new RpcError(ErrorCode.METHOD_NOT_FOUND, `Chauffeur does not handle ${message.method}`);
    }
  }

  /**
   * Puts an approval to the user. `rules`: what Always allow on it stands for, Codex's session
   * approval (none where Codex would ask again): a request they cover is allowed without asking,
   * and allowing one always allows the open requests the chat's rules then cover, each for the
   * session as Codex would have had it.
   */
  private async askPermission(
    serverId: RpcId,
    toolName: string,
    input: Record<string, JsonValue>,
    title: string | null,
    rules: string[] = [],
  ): Promise<Outcome> {
    // Codex asks again for what it does not show (a variant of the command) or no longer knows (a
    // new app-server since the chat's session was closed): the chat's own answer.
    if (this.always.covers(rules)) return { type: "allow", always: true };
    const outcome = await this.requests.ask(
      "permission",
      (requestId) => {
        this.serverRequests.set(serverId, requestId);
        return { type: "permission_request", request_id: requestId, tool_name: toolName, input, title };
      },
      {
        subject: { toolName, input, rules },
        resolution: (o) => (rules.length > 0 && o.type === "allow" && o.always ? { rules } : {}),
      },
    );
    this.serverRequests.delete(serverId);
    if (rules.length > 0 && outcome.type === "allow" && outcome.always) {
      this.always.add(rules);
      this.requests.allowCovered((call) => this.always.covering(call.rules), true);
    }
    // Codex's decline carries no message: pass the user's words on to the running turn.
    if (outcome.type === "deny" && outcome.message && this.activeTurnId) {
      void this.steer({ text: `I declined that: ${outcome.message}`, attachments: [] }, this.activeTurnId);
    }
    return outcome;
  }

  private onExit(reason: string): void {
    this.failStart?.(new Error(`codex app-server ${reason}`));
    if (this.state !== "running") return;
    // The app-server died on its own: report it and let the daemon resume the thread.
    this.state = "ended";
    this.host.log.error(`codex app-server ${reason}`);
    // Not the user's stop: the delegated chats work on, their answers wake the resumed thread.
    this.cancelRequests("detach");
    this.host.emit(notice("error", `Codex session ended: codex app-server ${reason}`));
    if (this.busy) this.endTurnLocally("error", `codex app-server ${reason}`, true);
    this.host.ended();
  }

  private beginTurn(): void {
    if (this.busy) return;
    this.busy = true;
    this.emit({ type: "turn_started" });
  }

  private endTurnLocally(outcome: "error" | "interrupted", error: string | null, force = false): void {
    this.busy = false;
    this.activeTurnId = null;
    this.turnStarting = false;
    const event: ChatEvent = { type: "turn_ended", outcome, duration_ms: null, cost_usd: null, context_tokens: null, error };
    if (force) this.host.emit(event);
    else this.emit(event);
  }

  private emit(event: ChatEvent): void {
    if (this.state === "ended" || this.state === "new") return;
    this.host.emit(event);
    this.mcp.observe(event);
  }
}

/**
 * A notification of another thread than the chat's: Codex reports the threads of the subagents it
 * spawns (`spawn_agent`) on the same connection. Their turns, text and tools are not the chat's: a
 * subagent's `turn/completed` would end the chat's turn while its own goes on, and the rest of a
 * message streaming then would show twice. Their requests still come to the chat, so their
 * withdrawal does too.
 */
function ofAnotherThread(message: ServerNotification, threadId: string): boolean {
  if (message.method === "serverRequest/resolved") return false;
  const params = message.params as { threadId?: unknown };
  return typeof params.threadId === "string" && params.threadId !== threadId;
}

/** `promise`, or a rejection as soon as `signal` aborts (an interrupt, a stop). */
function untilAborted<T>(promise: Promise<T>, signal: AbortSignal): Promise<T> {
  if (signal.aborted) return Promise.reject(signal.reason);
  return new Promise<T>((resolve, reject) => {
    const aborted = () => reject(signal.reason);
    signal.addEventListener("abort", aborted, { once: true });
    promise.then(resolve, reject).finally(() => signal.removeEventListener("abort", aborted));
  });
}

const RUNTIME_STATUSES: Record<NonNullable<McpServerStatus["runtimeStatus"]>, McpStatus> = {
  connected: "connected",
  starting: "pending",
  notStarted: "pending",
  authenticationRequired: "needs_auth",
  failed: "failed",
  cancelled: "failed",
  disabled: "disabled",
};

/** A server's status as Codex lists it; without a runtime status (not started yet), its login and tools tell. */
function codexStatus(server: McpServerStatus): McpStatus {
  let status: McpStatus;
  if (server.runtimeStatus) status = RUNTIME_STATUSES[server.runtimeStatus] ?? "failed";
  else if (server.authStatus === "notLoggedIn") status = "needs_auth";
  else if (server.toolsError) status = "failed";
  else status = "connected";
  if (status === "failed" && server.authStatus === "notLoggedIn") status = "needs_auth";
  return status;
}

function isBrokenStatus(status: McpStatus): boolean {
  return status === "failed" || status === "needs_auth";
}

/**
 * A server as Codex lists it. A broken one lacks the variables its error names
 * (`bearer_token_env_var`) and those of its entry in Codex's config (`declaration`: also
 * `env_http_headers` and `env_vars`, which Codex drops without a word) that `env`, the session's,
 * lacks.
 */
export function codexMcpServer(server: McpServerStatus, env: Record<string, string | undefined> = {}, declaration?: CodexDeclaration): McpServer {
  const status = codexStatus(server);
  const mapped: McpServer = {
    name: server.name,
    status,
    error: server.toolsError,
    failed_call: false,
    missing_env: [],
    uses_env: [],
    declared_in: null,
  };
  if (!isBrokenStatus(status)) return mapped;
  const named = codexMissingEnv(server.toolsError);
  const variables = declaration?.variables ?? [];
  const missing = [...new Set([...named, ...unsetVariables(variables, env)])];
  return {
    ...mapped,
    missing_env: missing,
    uses_env: [...new Set([...named, ...variables])],
    declared_in: missing.length === 0 ? null : (declaration?.where ?? codexDeclaredIn(server.name, server.pluginId, env)),
  };
}

function commandDecision(outcome: Outcome): CommandExecutionApprovalDecision {
  switch (outcome.type) {
    case "allow":
      return outcome.always ? "acceptForSession" : "accept";
    case "cancelled":
      return "cancel";
    default:
      return "decline";
  }
}

function fileDecision(outcome: Outcome): FileChangeApprovalDecision {
  switch (outcome.type) {
    case "allow":
      return outcome.always ? "acceptForSession" : "accept";
    case "cancelled":
      return "cancel";
    default:
      return "decline";
  }
}
