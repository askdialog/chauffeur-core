// One Claude chat: a long-lived SDK query() fed by a streaming input queue, so messages sent
// mid-turn are queued by the CLI itself.

import { ClaudeProcess } from "./process.ts";
import { settlesWithin } from "../../util/settles-within.ts";
import { randomUUID } from "node:crypto";
import type {
  McpServerStatus as SdkMcpServerStatus,
  Options,
  PermissionMode as SdkPermissionMode,
  Query,
  SDKControlGetContextUsageResponse,
  SDKMessage,
  SDKUserMessage,
} from "@anthropic-ai/claude-agent-sdk";
import {
  ErrorCode,
  type Attachment,
  type ChatEvent,
  type ContextDetail,
  type ContextUsage,
  type McpConnectResult,
  type McpServer,
  type McpStatus,
  type PermissionMode,
  type RequestResponse,
  type SessionStartParams,
  type SessionStartResult,
} from "../../protocol.ts";
import { RpcError } from "../../rpc/peer.ts";
import { errorMessage } from "../../log.ts";
import { AsyncQueue } from "../../util/async-queue.ts";
import type { AgentSession, SessionChanges, SessionHost } from "../agent.ts";
import { McpTracker } from "../mcp.ts";
import { claudeDeclaration, type Declaration, referencedVariables, unsetReferences } from "../mcp-env.ts";
import { contextUsageOf } from "./context.ts";
import { ClaudeEventMapper, notice } from "./events.ts";
import { buildHooks, type Confinement, type WriteFolder, writeRefusal } from "./hooks.ts";
import { jevPluginDir } from "./jev.ts";
import { buildClaudeOptions, effortLevel, PERMISSION_MODES } from "./options.ts";
import { claudeRepoTools } from "./repo-tools.ts";
import { PendingRequests } from "../pending.ts";
import { AlwaysAllowed } from "./always.ts";
import { claudeCanUseTool } from "./requests.ts";
import { userContent } from "./content.ts";

/** The part of the SDK's Query the session uses (tests provide a fake). */
export type QueryLike = AsyncIterable<SDKMessage> &
  Pick<
    Query,
    | "interrupt"
    | "setModel"
    | "setPermissionMode"
    | "applyFlagSettings"
    | "initializationResult"
    | "supportedModels"
    | "supportedCommands"
    | "getContextUsage"
    | "stopTask"
    | "close"
  > &
  Partial<Pick<Query, "mcpServerStatus" | "reconnectMcpServer">>;

export type QueryFn = (args: { prompt: AsyncIterable<SDKUserMessage>; options: Options }) => QueryLike;

export interface ClaudeSessionDeps {
  query: QueryFn;
  /** `pathToClaudeCodeExecutable`, or undefined for the SDK's own resolution. May throw an RpcError. */
  resolveExecutable: () => string | undefined;
  baseEnv: Record<string, string | undefined>;
  /** How long `start` waits for the CLI's initialize handshake. */
  initTimeoutMs?: number;
  /** Grace for native root exit and, separately, SDK consumption-loop completion. */
  stopTimeoutMs?: number;
  newRequestId?: () => string;
  /** Receives every raw SDK message (CHAUFFEUR_SIDECAR_TRACE). */
  trace?: (chatId: string, message: SDKMessage) => void;
  /** Where Chauffeur's Jev plugin is (written to the temp dir by default). */
  jevPluginDir?: () => string;
  /** How long the reading of the context after a turn may hold the turn's end. */
  contextTimeoutMs?: number;
}

type TurnEnded = Extract<ChatEvent, { type: "turn_ended" }>;

const AUTO_UNAVAILABLE = "Auto mode is unavailable for this chat";
const AUTO_FALLBACK = 'Claude asks before acting instead, as in "Ask for permissions"';
/** A design chat without auto mode still writes its screens without asking. */
const DESIGN_AUTO_FALLBACK = "Claude writes the screens without asking, and asks before other tools (MCP servers, reading outside the repos and the notes)";

/** Claude Code's permission modes as Chauffeur names them (`dontAsk` has no counterpart). */
const OUR_PERMISSION_MODES = new Map<SdkPermissionMode, PermissionMode>(
  Object.entries(PERMISSION_MODES).map(([ours, theirs]) => [theirs, ours as PermissionMode]),
);

/** Points of context a chat must gain after a compaction before Jev compacts it again. */
const RECOMPACT_GROWTH = 10;

type State = "new" | "starting" | "running" | "stopping" | "ended";

export class ClaudeSession implements AgentSession {
  private readonly input = new AsyncQueue<SDKUserMessage>();
  private readonly abortController = new AbortController();
  private readonly requests: PendingRequests;
  private readonly mapper = new ClaudeEventMapper();
  private readonly ready: Promise<void>;
  private markReady!: () => void;
  private query: QueryLike | undefined;
  private loop: Promise<void> | undefined;
  private state: State = "new";
  private stopping: Promise<void> | undefined;
  private readonly process = new ClaudeProcess((data) => this.host.log.debug(`claude: ${data.trimEnd()}`));
  /** A turn is running: between `turn_started` and `turn_ended`. */
  private busy = false;
  /** Background tasks that will wake the agent when they end (not ambient watchers). */
  private backgroundTasks = 0;
  /**
   * Messages sent mid-turn that the agent has not read yet. The CLI says nothing when it folds
   * one in: that happens when it next calls the model, after a tool result, or when it runs them
   * as the next turn. The first reply after that point marks them read.
   */
  private undelivered: string[] = [];
  private foldPoint = false;
  private interruptRequested = false;
  private failStart: ((error: unknown) => void) | undefined;
  /**
   * The `/compact` the session sent after a turn that filled the context past the Jev threshold:
   * that turn ends once the compaction has (`ended`), with the context it left (`after`).
   */
  private compaction: { ended: TurnEnded; after: number | null } | undefined;
  /** A compaction happened in the current turn: ours, Claude Code's own, or a `/compact` typed. */
  private compactedInTurn = false;
  /**
   * The context used (%, as `/context` counts it) at the end of the last turn that compacted: a
   * compaction that left it past the threshold is not redone until it grows again.
   */
  private compactedAt: number | undefined;
  /** The permission mode the chat asks for: auto may be unavailable, see `effectiveMode`. */
  private requestedMode: SdkPermissionMode;
  /** The permission mode Claude Code runs in, as last reported to the daemon. */
  private effectiveMode: PermissionMode | undefined;
  private readonly mcp: McpTracker;
  /** The environment Claude Code was started with: an MCP server's `${VAR}` is checked in it. */
  private agentEnv: Record<string, string | undefined> = {};
  private readonly declarations = new Map<string, Declaration | undefined>();
  /** Told the daemon once that Claude Code refused its credentials. */
  private accountLost = false;

  constructor(
    private readonly params: SessionStartParams,
    private readonly host: SessionHost,
    private readonly deps: ClaudeSessionDeps,
  ) {
    this.requests = new PendingRequests((event) => this.emit(event), deps.newRequestId);
    this.ready = new Promise((resolve) => {
      this.markReady = resolve;
    });
    // A design chat runs in auto mode whatever was asked: its guards and sandbox keep it to its
    // folder, the classifier decides the rest (MCP servers, reads elsewhere).
    this.requestedMode = params.design_dir ? "auto" : params.read_only ? "plan" : PERMISSION_MODES[params.permission_mode];
    // Until Claude Code says otherwise, it runs in the mode asked: the daemon assumes so too.
    this.effectiveMode = OUR_PERMISSION_MODES.get(this.requestedMode);
    this.mcp = new McpTracker({
      emit: (event) => this.emit(event),
      read: () => this.readMcp(),
      log: (message) => host.log.debug(message),
    });
  }

  async start(): Promise<SessionStartResult> {
    if (this.state !== "new") throw new RpcError(ErrorCode.CONFLICT, "session already started");
    this.state = "starting";
    let query: QueryLike;
    try {
      // A design chat writes only its design folder (even inside an exploration), an exploration
      // only its plans.
      const { design_dir: design, plans_dir: plans, cwd } = this.params;
      const confinement: Confinement | undefined = design ? "design" : this.params.read_only ? "exploration" : undefined;
      const folder: WriteFolder | undefined = design
        ? { kind: "design", dir: design, cwd }
        : plans
          ? { kind: "plans", dir: plans, cwd }
          : undefined;
      const options = buildClaudeOptions(this.params, {
        baseEnv: this.deps.baseEnv,
        executable: this.deps.resolveExecutable(),
        canUseTool: claudeCanUseTool(
          this.requests,
          confinement ? (tool, input) => writeRefusal(folder, tool, input) : undefined,
          (agentId) => this.mapper.tasks.agent(agentId),
          (toolUseId) => {
            for (const event of this.mapper.checkOver(toolUseId)) this.emit(event);
          },
          new AlwaysAllowed(this.params.always_allow),
        ),
        hooks: buildHooks(this.host, confinement, folder),
        abortController: this.abortController,
        stderr: (data) => this.host.log.debug(`claude: ${data.trimEnd()}`),
        jevPluginDir: this.deps.jevPluginDir ?? (() => jevPluginDir()),
        repoTools: design ? undefined : claudeRepoTools(this.host, this.requests, { delegate: this.params.delegate }),
      });
      options.spawnClaudeCodeProcess = this.process.spawn;
      this.agentEnv = options.env ?? {};
      query = this.deps.query({ prompt: this.inputMessages(), options });
    } catch (error) {
      this.state = "ended";
      this.shutdown();
      throw error instanceof RpcError ? error : new RpcError(ErrorCode.UNAVAILABLE, `Claude Code failed to start: ${errorMessage(error)}`);
    }
    this.query = query;
    // Queued now so it stays first; the input stream holds it until the CLI is initialized.
    if (this.params.prompt || this.params.prompt_attachments.length > 0) {
      this.send(this.params.prompt ?? "", this.params.prompt_attachments);
    }

    const failed = new Promise<never>((_, reject) => {
      this.failStart = reject;
    });
    this.loop = this.consume(query);
    let timer: ReturnType<typeof setTimeout> | undefined;
    const timeoutMs = this.deps.initTimeoutMs ?? 60_000;
    const timedOut = new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error(`no answer after ${timeoutMs / 1000}s`)), timeoutMs);
    });
    let init: unknown;
    try {
      init = await Promise.race([query.initializationResult(), failed, timedOut]);
    } catch (error) {
      this.state = "ended";
      this.shutdown();
      throw new RpcError(ErrorCode.UNAVAILABLE, `Claude Code failed to start: ${errorMessage(error)}`);
    } finally {
      clearTimeout(timer);
      this.failStart = undefined;
    }
    if (this.state !== "starting") throw new RpcError(ErrorCode.UNAVAILABLE, "the session was stopped while starting");
    this.state = "running";
    this.markReady();
    await this.checkAutoMode(init);
    this.mcp.start();
    return { agent_session_id: this.params.resume_session_id ?? null, ...(this.process.pid === undefined ? {} : { native_pid: this.process.pid }) };
  }

  /** The session's MCP servers as Claude Code reports them (not the SDK's in-process ones). */
  private async readMcp(): Promise<McpServer[]> {
    const read = this.query?.mcpServerStatus;
    if (!this.query || typeof read !== "function") return [];
    const servers = await read.call(this.query);
    const check: McpEnvCheck = { env: this.agentEnv, declaration: (name, scope) => this.declaration(name, scope) };
    return servers
      .filter((server) => server.source !== "sdk" && server.config?.type !== "sdk")
      .map((server) => mcpServerOf(server, check));
  }

  /** A server's declaration, read once per session (the file is read again by the next one). */
  private declaration(name: string, scope: string | undefined): Declaration | undefined {
    const key = `${scope ?? ""}\0${name}`;
    if (!this.declarations.has(key)) {
      const context = { env: this.agentEnv, cwd: this.params.cwd, directories: this.params.additional_directories };
      this.declarations.set(key, claudeDeclaration(name, scope, context));
    }
    return this.declarations.get(key);
  }

  async mcpConnect(name: string): Promise<McpConnectResult> {
    const query = this.query as (QueryLike & ClaudeMcpControls) | undefined;
    if (!query || this.state !== "running") throw new RpcError(ErrorCode.CONFLICT, "the session is not running");
    await this.mcp.refresh();
    const server = this.mcp.find(name);
    if (!server) throw new RpcError(ErrorCode.NOT_FOUND, `no MCP server ${name} in this chat`);
    if (server.status === "needs_auth") {
      if (typeof query.mcpAuthenticate !== "function") {
        throw new RpcError(ErrorCode.UNAVAILABLE, "this Claude Agent SDK cannot log MCP servers in");
      }
      const answer = await query.mcpAuthenticate(name, undefined);
      // claude.ai connectors are logged in on claude.ai, without a callback to Claude Code: ask it
      // to reconnect them while the user is at it. Others come back to the CLI's own callback.
      const reconnect = answer?.callbackExpected === false ? () => this.reconnectQuietly(name) : undefined;
      this.mcp.watchLogin(name, reconnect);
      if (answer?.authUrl) return { auth_url: answer.authUrl };
      return { auth_url: null };
    }
    if (typeof query.reconnectMcpServer !== "function") {
      throw new RpcError(ErrorCode.UNAVAILABLE, "this Claude Agent SDK cannot reconnect MCP servers");
    }
    try {
      await query.reconnectMcpServer(name);
    } finally {
      await this.mcp.refresh();
    }
    return { auth_url: null };
  }

  private async reconnectQuietly(name: string): Promise<void> {
    if (this.mcp.find(name)?.status !== "needs_auth") return;
    await this.query?.reconnectMcpServer?.(name);
  }

  send(text: string, attachments: Attachment[] = [], messageId?: string): void {
    if (this.state === "stopping" || this.state === "ended") {
      throw new RpcError(ErrorCode.NOT_FOUND, "the session is stopping");
    }
    if (messageId) {
      // Idle: the message starts a turn, so it is taken in at once.
      if (this.busy) this.undelivered.push(messageId);
      else this.emit({ type: "user_message_delivered", message_id: messageId });
    }
    this.input.push({
      type: "user",
      message: { role: "user", content: userContent(text, attachments, this.params.cwd) },
      parent_tool_use_id: null,
      uuid: randomUUID(),
    });
  }

  idle(): boolean {
    return (
      this.state === "running" &&
      !this.busy &&
      !this.compaction &&
      this.input.size === 0 &&
      this.backgroundTasks === 0 &&
      !this.requests.waiting()
    );
  }

  async interrupt(): Promise<void> {
    // The turn ends, not the background subagents (perTaskStopAffordance): their requests stay
    // open, answerable across turns.
    this.requests.cancelTurn();
    if (this.state !== "running" || !this.busy || !this.query) return;
    this.interruptRequested = true;
    try {
      await this.query.interrupt();
    } catch (error) {
      throw new RpcError(ErrorCode.INTERNAL, `interrupt failed: ${errorMessage(error)}`);
    }
  }

  respond(requestId: string, response: RequestResponse): void {
    this.requests.respond(requestId, response);
  }

  async stopTask(taskId: string): Promise<void> {
    if (this.state !== "running" || !this.query) throw new RpcError(ErrorCode.NOT_FOUND, "the session is not running");
    if (!this.mapper.tasks.running(taskId)) throw new RpcError(ErrorCode.NOT_FOUND, `no running task ${taskId}`);
    try {
      // Claude Code reports the task stopped with a task_notification.
      await this.query.stopTask(taskId);
    } catch (error) {
      throw new RpcError(ErrorCode.INTERNAL, `stopping the task failed: ${errorMessage(error)}`);
    }
  }

  async configure(changes: SessionChanges): Promise<void> {
    if (this.state !== "running" || !this.query) throw new RpcError(ErrorCode.CONFLICT, "the session is not running");
    const query = this.query;
    // Before anything changes: a design chat's mode is not the client's to pick (accept_edits is
    // what design chats made before auto mode keep in the daemon, and is ignored as auto is).
    if (this.params.design_dir && changes.permissionMode !== undefined && changes.permissionMode !== "auto" && changes.permissionMode !== "accept_edits") {
      throw new RpcError(ErrorCode.CONFLICT, "a design chat edits its screens and nothing else: its permission mode is fixed");
    }
    const effort = changes.effort === undefined ? undefined : effortLevel(changes.effort);
    try {
      if (changes.model !== undefined) await query.setModel(changes.model || undefined);
      if (changes.permissionMode !== undefined && !this.params.read_only && !this.params.design_dir) {
        this.requestedMode = PERMISSION_MODES[changes.permissionMode];
        await this.setPermissionMode(query, this.requestedMode);
      } else if (changes.model !== undefined && this.requestedMode === "auto") {
        // Auto mode depends on the model: asked again, it is taken back or refused for this one.
        await this.setPermissionMode(query, "auto");
      }
    } catch (error) {
      throw new RpcError(ErrorCode.INTERNAL, `configure failed: ${errorMessage(error)}`);
    }
    if (effort !== undefined) {
      try {
        await query.applyFlagSettings({ effortLevel: effort });
      } catch (error) {
        this.emit(notice("warning", `Effort "${effort}" will apply when the chat restarts (${errorMessage(error)})`));
      }
    }
    if (changes.fast !== undefined) {
      try {
        await query.applyFlagSettings({ fastMode: changes.fast });
      } catch (error) {
        this.emit(notice("warning", `Fast mode will change when the chat restarts (${errorMessage(error)})`));
      }
    }
  }

  /**
   * Claude Code refuses auto mode where it is unavailable (a model without it, a plan, a setting):
   * the chat then asks, as in `default` (a design chat: `acceptEdits`), and says why.
   */
  private async setPermissionMode(query: QueryLike, mode: SdkPermissionMode): Promise<void> {
    try {
      await query.setPermissionMode(mode);
      this.reportMode(mode, true);
    } catch (error) {
      if (mode !== "auto") throw error;
      const fallback = this.params.design_dir ? "acceptEdits" : "default";
      await query.setPermissionMode(fallback);
      this.reportMode(fallback, true);
      this.emit(notice("warning", `${AUTO_UNAVAILABLE} (${errorMessage(error)}). ${this.params.design_dir ? DESIGN_AUTO_FALLBACK : AUTO_FALLBACK}`));
    }
  }

  /**
   * Tells the daemon the permission mode Claude Code runs in when it changes (from the mode asked,
   * at first), so that the picker shows a fallback from auto rather than the mode asked for.
   * `always` after the chat asked for a mode: the daemon's idea of it changed with the request.
   */
  private reportMode(mode: unknown, always = false): void {
    const ours = OUR_PERMISSION_MODES.get(mode as SdkPermissionMode);
    if (ours === undefined || (ours === this.effectiveMode && !always)) return;
    this.effectiveMode = ours;
    this.emit({ type: "permission_mode_effective", mode: ours });
  }

  /**
   * A session started in auto mode where it is unavailable runs in `default` (Claude Code
   * downgrades it without an error): says so, from the mode the initialize handshake reports. A
   * design chat is switched to `acceptEdits` then, so that it still writes its screens without
   * asking.
   */
  private async checkAutoMode(init: unknown): Promise<void> {
    const current = (init as { current_permission_mode?: unknown } | undefined)?.current_permission_mode;
    this.reportMode(current);
    if (this.requestedMode !== "auto") return;
    if (typeof current !== "string" || current === "auto") return;
    const model = this.params.model ? ` with ${this.params.model}` : "";
    const why = `${AUTO_UNAVAILABLE}${model} (its model, plan or settings turn it off)`;
    if (!this.params.design_dir) {
      this.emit(notice("warning", `${why}. ${AUTO_FALLBACK}`));
      return;
    }
    try {
      await this.query?.setPermissionMode("acceptEdits");
      this.reportMode("acceptEdits", true);
      this.emit(notice("warning", `${why}. ${DESIGN_AUTO_FALLBACK}`));
    } catch (error) {
      this.emit(notice("warning", `${why}, and accepting edits failed (${errorMessage(error)}): Claude asks before writing the screens too`));
    }
  }

  stop(): Promise<void> {
    if (this.stopping) return this.stopping;
    const stopping = this.stopSession();
    this.stopping = stopping;
    // A failed attempt stays stopping but can be retried against the same native child.
    void stopping.catch(() => { if (this.stopping === stopping) this.stopping = undefined; });
    return stopping;
  }

  private async stopSession(): Promise<void> {
    const running = this.state === "running";
    this.state = "stopping";
    this.failStart?.(new Error("the session was stopped while starting"));
    this.requests.cancelAll();
    if (running && this.busy) this.endTurn({ outcome: "interrupted", error: null });
    this.shutdown();
    const timeout = this.deps.stopTimeoutMs ?? 5_000;
    await this.process.stop(timeout);
    if (this.loop && !(await settlesWithin(this.loop, timeout))) {
      throw new RpcError(ErrorCode.UNAVAILABLE, "Claude session shutdown is unconfirmed; keep the workspace fenced and retry stopping it");
    }
    for (const event of this.mapper.tasks.endAll("The agent session was closed")) this.emit(event);
    this.state = "ended";
  }

  /** Feeds the SDK. Emits `turn_started` when a message is handed over while the session is idle. */
  private async *inputMessages(): AsyncGenerator<SDKUserMessage> {
    await this.ready;
    for await (const message of this.input) {
      if (this.state !== "running") return;
      this.beginTurn();
      yield message;
    }
  }

  private async consume(query: QueryLike): Promise<void> {
    let failure: unknown;
    try {
      for await (const message of query) await this.handle(message);
    } catch (error) {
      failure = error;
    }
    await this.queryEnded(failure);
  }

  private async handle(message: SDKMessage): Promise<void> {
    this.deps.trace?.(this.host.chatId, message);
    if (this.state === "ended") return;
    this.noteAccount(message);
    if (message.type === "result") {
      // Keep turn_started / turn_ended paired even for a turn the CLI started on its own.
      this.beginTurn();
      const interrupted = this.interruptRequested;
      this.interruptRequested = false;
      const queuedTurns = message.queued_turn_count ?? 0;
      // Folded into the turn that just ended, unless they run as the next one.
      if (queuedTurns === 0) this.markDelivered();
      else this.foldPoint = true;
      let events = this.mapper.map(message, { interrupted });
      // One reading of the context per turn: the app's meter shows it, Jev's threshold uses it.
      const reading = await this.readContext();
      const used = reading?.percentage;
      if (reading) events = withReading(events, contextUsageOf(reading, { jevThreshold: this.params.jev?.threshold_percent }));
      if (this.compaction) {
        events = this.compactionEnded(this.compaction, events);
        this.compaction = undefined;
        this.noteCompaction(used);
      } else if (this.compactedInTurn) {
        // Claude Code compacted on its own, or a /compact was typed: not twice in a row.
        this.noteCompaction(used);
      } else if (queuedTurns === 0 && this.compactAfter(events, used)) {
        // The turn goes on, compacting: still busy, so that messages sent meanwhile wait.
        return;
      }
      this.busy = false;
      for (const event of events) this.emit(event);
      // A turn may have used, or lost, a server: read them again.
      void this.mcp.refresh();
      // Messages sent mid-turn that the CLI did not fold into this turn run as the next one.
      if (queuedTurns > 0) this.beginTurn();
      return;
    }
    // A suggestion for a turn already followed by another (a queued message, a steer) is stale.
    if (message.type === "prompt_suggestion" && this.busy) return;
    // Each turn's init, and status messages, carry the mode Claude Code runs in.
    if (message.type === "system" && (message.subtype === "init" || message.subtype === "status")) {
      this.reportMode(message.permissionMode);
    }
    if (message.type === "system" && message.subtype === "background_tasks_changed") {
      this.backgroundTasks = message.tasks.filter((task) => !task.ambient).length;
    }
    if (message.type === "system" && message.subtype === "compact_boundary") {
      this.compactedInTurn = true;
      if (this.compaction) this.compaction.after = message.compact_metadata.post_tokens ?? null;
    }
    this.trackDelivery(message);
    // Only the agent itself opens a turn: a background subagent goes on after the turn that
    // started it ended, and the chat stays idle meanwhile (its task shows as running). When the
    // subagent hands back, Claude Code starts a turn of the agent's own, which opens one here.
    if (isTurnActivity(message) && fromAgentItself(message)) this.beginTurn();
    for (const event of this.mapper.map(message)) this.emit(event);
  }

  /**
   * After a turn that left the context past the Jev threshold, as Claude Code's `/context` counts
   * it, sends `/compact` (Jev replaces the summary) and holds the turn's end until it is done.
   * True when it did.
   */
  private compactAfter(events: ChatEvent[], used: number | undefined): boolean {
    const jev = this.params.jev;
    const ended = events.at(-1);
    if (!jev || ended?.type !== "turn_ended" || ended.outcome !== "success") return false;
    if (used === undefined || used < jev.threshold_percent) return false;
    if (this.compactedAt !== undefined && used < this.compactedAt + RECOMPACT_GROWTH) return false;
    if (this.state !== "running") return false;
    for (const event of events.slice(0, -1)) this.emit(event);
    this.emit(notice("info", `Compacting the conversation with Jev: ${used}% of the context is used`));
    this.compaction = { ended, after: null };
    this.input.push({
      type: "user",
      message: { role: "user", content: "/compact" },
      parent_tool_use_id: null,
      uuid: randomUUID(),
    });
    return true;
  }

  /** After a turn that compacted: where the context stands now. */
  private noteCompaction(used: number | undefined): void {
    this.compactedInTurn = false;
    if (this.params.jev) this.compactedAt = used;
  }

  /**
   * Claude Code's `/context` summary (the last response's usage and estimates, a few ms);
   * undefined when unavailable or slow, so that it never holds a turn's end for long.
   */
  private async readContext(): Promise<SDKControlGetContextUsageResponse | undefined> {
    if (this.state !== "running" || !this.query) return undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const timeoutMs = this.deps.contextTimeoutMs ?? 5_000;
    try {
      return await Promise.race([
        this.query.getContextUsage({ detail: "summary" }),
        new Promise<never>((_, reject) => {
          timer = setTimeout(() => reject(new Error(`no answer after ${timeoutMs / 1000}s`)), timeoutMs);
        }),
      ]);
    } catch (error) {
      this.host.log.debug(`context usage unavailable: ${errorMessage(error)}`);
      return undefined;
    } finally {
      clearTimeout(timer);
    }
  }

  /**
   * How full the context is now, for the app's context panel: `full` counts each category with
   * the token-count API (slower) and lists what they hold.
   */
  async contextUsage(detail: ContextDetail): Promise<ContextUsage> {
    if (this.state !== "running" || !this.query) throw new RpcError(ErrorCode.NOT_FOUND, "the session is not running");
    try {
      const reading = await this.query.getContextUsage({ detail });
      return contextUsageOf(reading, { jevThreshold: this.params.jev?.threshold_percent, details: detail === "full" });
    } catch (error) {
      throw new RpcError(ErrorCode.INTERNAL, `reading the context failed: ${errorMessage(error)}`);
    }
  }

  /** The end of the turn a compaction followed, with the context the compaction left. */
  private compactionEnded(compaction: { ended: TurnEnded; after: number | null }, events: ChatEvent[]): ChatEvent[] {
    const result = events.find((event): event is TurnEnded => event.type === "turn_ended");
    const failed = result && result.outcome !== "success" ? [notice("warning", `The compaction did not finish: ${result.error ?? result.outcome}`)] : [];
    return [
      ...events.filter((event) => event.type !== "turn_ended"),
      ...failed,
      { ...compaction.ended, context_tokens: compaction.after ?? compaction.ended.context_tokens },
    ];
  }

  private trackDelivery(message: SDKMessage): void {
    if (this.undelivered.length === 0) return;
    if (message.type === "user" && isTurnActivity(message)) {
      this.foldPoint = true;
    } else if (this.foldPoint && (message.type === "assistant" || message.type === "stream_event") && message.parent_tool_use_id === null) {
      this.markDelivered();
    }
  }

  private markDelivered(): void {
    this.foldPoint = false;
    for (const id of this.undelivered.splice(0)) this.emit({ type: "user_message_delivered", message_id: id });
  }

  private async queryEnded(failure: unknown): Promise<void> {
    this.failStart?.(failure ?? new Error("Claude Code exited during startup"));
    if (this.state !== "running") return;
    // The CLI crashed or exited on its own: report it and let the daemon resume the chat. Its
    // requests are reported cancelled first: a background subagent's may outlive any turn.
    this.requests.cancelAll();
    this.state = "stopping";
    const reason = failure === undefined ? "Claude Code exited unexpectedly" : errorMessage(failure);
    this.host.log.error(`session ended: ${reason}`);
    this.host.emit(notice("error", `Claude session ended: ${reason}`));
    for (const event of this.mapper.tasks.endAll("The agent process stopped")) this.host.emit(event);
    if (this.busy) this.endTurn({ outcome: "error", error: reason }, true);
    this.shutdown();
    try {
      await this.process.stop(this.deps.stopTimeoutMs ?? 5_000);
      this.state = "ended";
      this.host.ended();
    } catch (error) {
      this.host.log.error(`Claude exit remains unconfirmed: ${errorMessage(error)}`);
      this.host.emit(notice("error", "Claude process exit is unconfirmed; retry stopping the session before resuming"));
    }
  }

  private beginTurn(): void {
    if (this.busy) return;
    this.busy = true;
    this.emit({ type: "turn_started" });
  }

  private endTurn(end: { outcome: "error" | "interrupted"; error: string | null }, force = false): void {
    this.busy = false;
    const event: ChatEvent = {
      type: "turn_ended",
      outcome: end.outcome,
      duration_ms: null,
      cost_usd: null,
      context_tokens: null,
      error: end.error,
    };
    if (force) this.host.emit(event);
    else this.emit(event);
  }

  /** Ends the input, aborts the CLI and releases a pending start. */
  private shutdown(): void {
    this.mcp.stop();
    this.input.end();
    this.markReady();
    if (!this.abortController.signal.aborted) this.abortController.abort();
    try {
      this.query?.close();
    } catch (error) {
      this.host.log.debug(`close failed: ${errorMessage(error)}`);
    }
  }

  private emit(event: ChatEvent): void {
    if (this.state === "ended" || this.state === "new") return;
    this.host.emit(event);
    this.mcp.observe(event);
  }

  /** Claude Code refused its own credentials: the daemon marks Claude signed out (once a session). */
  private noteAccount(message: SDKMessage): void {
    if (this.accountLost) return;
    let why: string | undefined;
    if (message.type === "auth_status" && message.error) why = message.error;
    if (message.type === "assistant" && (message.error === "authentication_failed" || message.error === "oauth_org_not_allowed")) {
      why = message.error === "authentication_failed" ? "Claude Code's login was refused" : "Claude Code's organization does not allow this login";
    }
    if (why === undefined) return;
    this.accountLost = true;
    this.emit({ type: "account_lost", message: why });
  }
}

/** The SDK's MCP controls the session uses that its types do not declare. */
type ClaudeMcpControls = {
  mcpAuthenticate?: (
    serverName: string,
    redirectUri: string | undefined,
  ) => Promise<{ authUrl?: string; requiresUserAction?: boolean; callbackExpected?: boolean } | undefined>;
};

const MCP_STATUSES: Record<SdkMcpServerStatus["status"], McpStatus> = {
  connected: "connected",
  pending: "pending",
  "needs-auth": "needs_auth",
  failed: "failed",
  disabled: "disabled",
};

/** What tells the variables a broken server lacks: the session's environment, its declarations. */
export interface McpEnvCheck {
  env: Record<string, string | undefined>;
  declaration: (name: string, scope: string | undefined) => Declaration | undefined;
}

export function mcpServerOf(server: SdkMcpServerStatus, check?: McpEnvCheck): McpServer {
  const status = MCP_STATUSES[server.status] ?? "failed";
  const mapped: McpServer = {
    name: server.name,
    status,
    error: server.error ?? null,
    failed_call: false,
    missing_env: [],
    uses_env: [],
    declared_in: null,
  };
  if (!check || (status !== "failed" && status !== "needs_auth")) return mapped;
  // The status hides a stdio server's `env`: its declaration has it.
  const declaration = check.declaration(server.name, server.scope);
  const configs = [server.config, declaration?.config];
  const missing = unsetReferences(configs, check.env);
  return {
    ...mapped,
    missing_env: missing,
    uses_env: referencedVariables(configs),
    declared_in: missing.length > 0 ? (declaration?.where ?? null) : null,
  };
}

/** The turn's events with its context reading, just before its `turn_ended`. */
function withReading(events: ChatEvent[], usage: ContextUsage): ChatEvent[] {
  const reading: ChatEvent = { type: "context_usage", usage };
  const end = events.findIndex((event) => event.type === "turn_ended");
  return end < 0 ? [...events, reading] : [...events.slice(0, end), reading, ...events.slice(end)];
}

/** Not a subagent's: its own messages carry the Agent tool call that started it. */
function fromAgentItself(message: SDKMessage): boolean {
  return (message as { parent_tool_use_id?: string | null }).parent_tool_use_id == null;
}

/** Messages that only exist inside a turn (the agent's or a subagent's). */
function isTurnActivity(message: SDKMessage): boolean {
  if (message.type === "assistant" || message.type === "stream_event") return true;
  if (message.type !== "user") return false;
  const content = message.message.content;
  return Array.isArray(content) && content.some((block) => block.type === "tool_result");
}
