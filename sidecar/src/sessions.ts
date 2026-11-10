// Owns the live sessions, one per chat, and routes calls to them whatever the agent behind.

import type { Agent, AgentSession, SessionChanges, SessionHost } from "./agents/agent.ts";
import {
  ErrorCode,
  type AgentAskParams,
  type AgentBriefParams,
  type AgentRetitleParams,
  type AgentForkParams,
  type AgentForkResult,
  type AgentKind,
  type AgentLoginEndedParams,
  type AgentLoginResult,
  type AgentModel,
  type AgentSubagentMessagesParams,
  type AgentSubagentMessagesResult,
  type NativeAccountInfo,
  type AgentUsageResult,
  type Attachment,
  type ChatEvent,
  type ContextDetail,
  type ContextUsage,
  type McpConnectResult,
  type RequestResponse,
  type SessionStartParams,
  type SessionStartResult,
  type SlashCommand,
  type ToolAddRepoResult,
  type ToolDelegateParams,
  type ToolDelegateResult,
  type ToolDelegateStarted,
  type ToolListReposResult,
} from "./protocol.ts";
import { DeltaCoalescer } from "./delta-coalescer.ts";
import { RpcError } from "./rpc/peer.ts";
import type { Logger } from "./log.ts";

/** How sessions reach the daemon. */
export interface DaemonLink {
  emit(chatId: string, event: ChatEvent): void;
  createWorktree(chatId: string, name: string): Promise<string>;
  removeWorktree(chatId: string, path: string): Promise<void>;
  listRepos(chatId: string): Promise<ToolListReposResult>;
  addRepo(chatId: string, repo: string): Promise<ToolAddRepoResult>;
  delegate(params: ToolDelegateParams): Promise<ToolDelegateStarted>;
  delegateWait(chatId: string, child: string): Promise<ToolDelegateResult>;
  delegateStop(chatId: string, child: string): Promise<void>;
  delegateDetach(chatId: string, child: string): Promise<void>;
  /** A login `login` started ended (`agent.login_ended`). */
  loginEnded(params: AgentLoginEndedParams): void;
}

export class SessionManager {
  private readonly agents = new Map<AgentKind, Agent>();
  private readonly sessions = new Map<string, AgentSession>();
  private readonly stopping = new Map<AgentSession, Promise<void>>();
  // Tombstones retain only dispatch metadata, not stopped sessions, SDK queries or secrets.
  private readonly launches = new Map<string, { chat: string; session?: AgentSession; stopped: boolean }>();
  private readonly events: DeltaCoalescer;

  constructor(
    agents: Agent[],
    private readonly daemon: DaemonLink,
    private readonly log: Logger,
  ) {
    for (const agent of agents) this.agents.set(agent.kind, agent);
    this.events = new DeltaCoalescer((chatId, event) => daemon.emit(chatId, event));
  }

  get size(): number {
    return this.sessions.size;
  }

  has(chatId: string): boolean {
    return this.sessions.has(chatId);
  }

  /** An agent's model catalog. */
  models(kind: AgentKind): Promise<AgentModel[]> {
    const agent = this.agents.get(kind);
    if (!agent) throw new RpcError(ErrorCode.UNAVAILABLE, `agent "${kind}" is not supported by this sidecar`);
    return agent.listModels();
  }

  /** The commands an agent offers in a folder. */
  commands(kind: AgentKind, cwd: string, directories: string[]): Promise<SlashCommand[]> {
    const agent = this.agents.get(kind);
    if (!agent) throw new RpcError(ErrorCode.UNAVAILABLE, `agent "${kind}" is not supported by this sidecar`);
    return agent.listCommands(cwd, directories);
  }

  /** Names work with Claude, falling back to Codex when Claude is unavailable. */
  async title(text: string): Promise<string> {
    try {
      const claude = this.agents.get("claude");
      if (!claude?.title) throw new RpcError(ErrorCode.UNAVAILABLE, "Claude is not available in this sidecar");
      return await claude.title(text);
    } catch (error) {
      this.log.warn(`Claude could not name the work; trying Codex: ${error}`);
      const codex = this.agents.get("codex");
      if (!codex?.title) throw error;
      return codex.title(text);
    }
  }

  /** Reconsiders a title with the same provider fallback as initial naming. */
  async retitle(params: AgentRetitleParams): Promise<string> {
    try {
      const claude = this.agents.get("claude");
      if (!claude?.retitle) throw new RpcError(ErrorCode.UNAVAILABLE, "Claude is not available in this sidecar");
      return await claude.retitle(params);
    } catch (error) {
      this.log.warn(`Claude could not rename the work; trying Codex: ${error}`);
      const codex = this.agents.get("codex");
      if (!codex?.retitle) throw error;
      return codex.retitle(params);
    }
  }

  /** One question answered in one turn by the agent named, without tools. */
  ask(params: AgentAskParams): Promise<string> {
    const agent = this.agents.get(params.agent);
    if (!agent?.ask) throw new RpcError(ErrorCode.UNAVAILABLE, `${params.agent} is not available in this sidecar`);
    return agent.ask(params);
  }

  /** A design brief for a design chat, from Claude whatever the source chat's agent. */
  brief(params: AgentBriefParams): Promise<string> {
    const claude = this.agents.get("claude");
    if (!claude?.brief) throw new RpcError(ErrorCode.UNAVAILABLE, "Claude is not available in this sidecar");
    return claude.brief(params);
  }

  account(kind: AgentKind): Promise<NativeAccountInfo> {
    const agent = this.agents.get(kind);
    if (!agent?.account) throw new RpcError(ErrorCode.UNAVAILABLE, "Native account metadata is unavailable");
    return agent.account();
  }

  usage(kind: AgentKind): Promise<AgentUsageResult> {
    const agent = this.agents.get(kind);
    if (!agent?.usage) throw new RpcError(ErrorCode.UNAVAILABLE, `${kind} is not available in this sidecar`);
    return agent.usage();
  }

  /** Starts logging an agent's CLI in; the daemon hears of its end through `loginEnded`. */
  login(kind: AgentKind, headless: boolean): Promise<AgentLoginResult> {
    const agent = this.agents.get(kind);
    if (!agent?.login) throw new RpcError(ErrorCode.UNAVAILABLE, `${kind} cannot be logged in from Chauffeur`);
    return agent.login((success, error) => this.daemon.loginEnded({ agent: kind, success, error }), headless);
  }

  loginCode(kind: AgentKind, code: string): Promise<void> {
    const agent = this.agents.get(kind);
    if (!agent?.loginCode) throw new RpcError(ErrorCode.BAD_REQUEST, `${kind}'s login takes no code`);
    return agent.loginCode(code);
  }

  loginCancel(kind: AgentKind): void {
    this.agents.get(kind)?.loginCancel?.();
  }

  mcpConnect(chatId: string, server: string): Promise<McpConnectResult> {
    const session = this.get(chatId);
    if (!session.mcpConnect) throw new RpcError(ErrorCode.UNAVAILABLE, "this agent cannot reconnect its MCP servers");
    return session.mcpConnect(server);
  }

  /** A subagent's conversation from its transcript: no session needed. */
  subagentMessages(params: AgentSubagentMessagesParams): Promise<AgentSubagentMessagesResult> {
    const agent = this.agents.get(params.agent);
    if (!agent?.subagentMessages) {
      throw new RpcError(ErrorCode.UNAVAILABLE, `${params.agent} runs no subagents with a conversation of their own`);
    }
    return agent.subagentMessages(params);
  }

  /** Copies an agent session into a new one (a chat forked into a new tab): no session needed. */
  async fork(params: AgentForkParams): Promise<AgentForkResult> {
    const agent = this.agents.get(params.agent);
    if (!agent?.fork) throw new RpcError(ErrorCode.UNAVAILABLE, `${params.agent} cannot fork its sessions in this sidecar`);
    return { session_id: await agent.fork(params) };
  }

  versions(): Record<string, string> {
    const versions: Record<string, string> = {};
    for (const agent of this.agents.values()) Object.assign(versions, agent.versions);
    return versions;
  }

  async start(params: SessionStartParams): Promise<SessionStartResult> {
    const chatId = params.chat_id;
    let launch: { chat: string; session?: AgentSession; stopped: boolean } | undefined;
    if (params.launch_id !== undefined) {
      if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(params.launch_id)) {
        throw new RpcError(ErrorCode.INVALID_PARAMS, "launch_id must be a canonical UUID");
      }
      if (this.launches.has(params.launch_id)) throw new RpcError(ErrorCode.CONFLICT, "Agent launch was already observed; reconcile its stop instead of replaying it");
      // Constructors cannot start a native process. Until session.start is entered, rejection
      // proves that this particular intent created no process, even if the chat was occupied.
      launch = { chat: chatId, stopped: true };
      this.launches.set(params.launch_id, launch);
    }
    if (this.sessions.has(chatId)) {
      throw new RpcError(ErrorCode.CONFLICT, `chat ${chatId} already has a running session; stop it first`);
    }
    const agent = this.agents.get(params.agent);
    if (!agent) {
      const available = [...this.agents.keys()].join(", ");
      throw new RpcError(
        ErrorCode.UNAVAILABLE,
        `agent "${params.agent}" is not supported by this sidecar yet (available: ${available})`,
      );
    }
    let session: AgentSession | undefined;
    const host = this.hostFor(chatId, () => session);
    session = agent.createSession(params, host);
    if (launch) { launch.session = session; launch.stopped = false; }
    this.sessions.set(chatId, session);
    try {
      return await session.start();
    } catch (error) {
      try {
        await this.stopSession(chatId, session);
      } catch (cleanup) {
        this.log.error(`failed start retained for stop reconciliation: ${String(cleanup)}`);
      }
      throw error;
    }
  }

  send(chatId: string, text: string, attachments: Attachment[] = [], messageId?: string): void {
    this.get(chatId).send(text, attachments, messageId);
  }

  interrupt(chatId: string): Promise<void> {
    return this.get(chatId).interrupt();
  }

  respond(chatId: string, requestId: string, response: RequestResponse): void {
    this.get(chatId).respond(requestId, response);
  }

  stopTask(chatId: string, taskId: string): Promise<void> {
    return this.get(chatId).stopTask(taskId);
  }

  async contextUsage(chatId: string, detail: ContextDetail): Promise<ContextUsage> {
    const session = this.get(chatId);
    if (!session.contextUsage) throw new RpcError(ErrorCode.UNAVAILABLE, "this agent reads its context only after a turn");
    return session.contextUsage(detail);
  }

  configure(chatId: string, changes: SessionChanges): Promise<void> {
    return this.get(chatId).configure(changes);
  }

  /** Stopping a chat without a session is a no-op: the daemon may stop defensively. */
  async stop(chatId: string): Promise<void> {
    const session = this.sessions.get(chatId);
    if (!session) return;
    await this.stopSession(chatId, session);
  }

  private stopSession(chatId: string, session: AgentSession): Promise<void> {
    const pending = this.stopping.get(session);
    if (pending) return pending;
    // Register before calling into the agent: it can synchronously report an end.
    const stopping = Promise.resolve().then(() => session.stop()).then(() => this.forget(chatId, session));
    this.stopping.set(session, stopping);
    void stopping.finally(() => { this.stopping.delete(session); }).catch(() => {});
    return stopping;
  }

  /**
   * Stops the session if nothing is lost by it (`AgentSession.idle`); the next message starts it
   * again with its context. Resolves with whether the chat has no session now.
   */
  async closeIdle(chatId: string): Promise<boolean> {
    const session = this.sessions.get(chatId);
    if (!session) return true;
    if (!session.idle()) return false;
    await this.stop(chatId);
    return true;
  }

  async stopAll(): Promise<void> {
    await Promise.allSettled([...this.sessions.keys()].map((chatId) => this.stop(chatId)));
  }

  private get(chatId: string): AgentSession {
    const session = this.sessions.get(chatId);
    if (!session) throw new RpcError(ErrorCode.NOT_FOUND, `chat ${chatId} has no running session`);
    return session;
  }

  /** Forgets `session` unless a newer session already took its chat. */
  private forget(chatId: string, session: AgentSession | undefined): void {
    this.events.flush(chatId);
    if (!session) return;
    if (this.sessions.get(chatId) === session) this.sessions.delete(chatId);
    for (const launch of this.launches.values()) {
      if (launch.session === session) { launch.stopped = true; delete launch.session; }
    }
  }

  /** Metadata from this sidecar generation only; absence is never an exit receipt. */
  confirmedLaunches(chat: string): string[] {
    return [...this.launches].filter(([, launch]) => launch.chat === chat && launch.stopped).map(([id]) => id);
  }

  private hostFor(chatId: string, owner: () => AgentSession | undefined): SessionHost {
    // A call to the daemon finds the chat's text up to date, as it would without coalescing.
    const daemon = (): DaemonLink => {
      this.events.flush(chatId);
      return this.daemon;
    };
    return {
      chatId,
      log: this.log.child(chatId),
      emit: (event) => this.events.push(chatId, event),
      createWorktree: (name) => daemon().createWorktree(chatId, name),
      removeWorktree: (path) => daemon().removeWorktree(chatId, path),
      listRepos: () => daemon().listRepos(chatId),
      addRepo: (repo) => daemon().addRepo(chatId, repo),
      delegate: (params) => daemon().delegate({ ...params, chat_id: chatId }),
      delegateWait: (child) => daemon().delegateWait(chatId, child),
      delegateStop: (child) => daemon().delegateStop(chatId, child),
      delegateDetach: (child) => daemon().delegateDetach(chatId, child),
      ended: () => {
        const session = owner();
        if (session && !this.stopping.has(session)) this.forget(chatId, session);
      },
    };
  }
}
