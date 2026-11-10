// The agent abstraction. The protocol layer (dispatch, session manager) only knows these
// interfaces; each agent (Claude today, Codex later) maps its own runtime onto them and onto the
// agent-agnostic ChatEvent stream.

import type {
  AgentAskParams,
  AgentBriefParams,
  AgentRetitleParams,
  AgentForkParams,
  AgentKind,
  AgentModel,
  AgentSubagentMessagesParams,
  AgentSubagentMessagesResult,
  AgentLoginResult,
  NativeAccountInfo,
  AgentUsageResult,
  Attachment,
  ChatEvent,
  ContextDetail,
  ContextUsage,
  McpConnectResult,
  PermissionMode,
  RequestResponse,
  SessionStartParams,
  SessionStartResult,
  SlashCommand,
  ToolAddRepoResult,
  ToolDelegateParams,
  ToolDelegateResult,
  ToolDelegateStarted,
  ToolListReposResult,
} from "../protocol.ts";
import type { Logger } from "../log.ts";

/** What a session may ask of the sidecar, scoped to its chat. */
export interface SessionHost {
  readonly chatId: string;
  readonly log: Logger;
  /** Sends a `session.event` notification to the daemon. */
  emit(event: ChatEvent): void;
  /** Asks the daemon to create a worktree group (`hook.worktree_create`); resolves with its path. */
  createWorktree(name: string): Promise<string>;
  /** Asks the daemon to remove a worktree it created (`hook.worktree_remove`). */
  removeWorktree(path: string): Promise<void>;
  /** The repos the agent may propose for its workspace (`tool.list_repos`). */
  listRepos(): Promise<ToolListReposResult>;
  /** Adds a repo to the chat's workspace, once the user said yes (`tool.add_repo`). */
  addRepo(repo: string): Promise<ToolAddRepoResult>;
  /** Gives a delegated chat its task: a new one, or one the chat started earlier (`tool.delegate`). */
  delegate(params: Omit<ToolDelegateParams, "chat_id">): Promise<ToolDelegateStarted>;
  /** Waits for a delegated chat's answer (`tool.delegate_wait`). */
  delegateWait(child: string): Promise<ToolDelegateResult>;
  /** Interrupts a delegated chat whose answer is no longer awaited (`tool.delegate_stop`). */
  delegateStop(child: string): Promise<void>;
  /**
   * The agent's turn ended before a delegated chat answered, not by an interrupt: it works on,
   * and the daemon sends its answer as a new message (`tool.delegate_detach`).
   */
  delegateDetach(child: string): Promise<void>;
  /**
   * The session ended on its own (crash, agent process exit). The manager forgets it, so the daemon
   * can call `session.start` again with the agent's resume id.
   */
  ended(): void;
}

export interface SessionChanges {
  model?: string | undefined;
  effort?: string | undefined;
  fast?: boolean | undefined;
  permissionMode?: PermissionMode | undefined;
}

/** One live conversation with an agent, bound to one chat. */
export interface AgentSession {
  /**
   * Starts the agent and resolves once it accepts input. Rejects (with an RpcError when the
   * cause is the caller's) if the agent cannot start. Failed initialization still needs
   * stop confirmation before the chat can be reused.
   */
  start(): Promise<SessionStartResult>;
  /**
   * Hands over a user message (and its images): it starts a turn, or the agent picks it up after
   * its current step. A message with an id that arrives mid-turn is reported with
   * `user_message_delivered` once the agent took it in.
   */
  send(text: string, attachments?: Attachment[], messageId?: string): void;
  /** Cancels pending requests and stops the current turn. */
  interrupt(): Promise<void>;
  /** Answers a `permission_request` or `question` the session emitted. */
  respond(requestId: string, response: RequestResponse): void;
  /**
   * Stops one task the session reported with `task_started`; its `task_ended` follows. Rejects
   * when the task is not running, or the agent has no such tasks.
   */
  stopTask(taskId: string): Promise<void>;
  configure(changes: SessionChanges): Promise<void>;
  /**
   * How full the context window is now. Absent from agents that read it only after a turn
   * (Codex, through its `context_usage` events).
   */
  contextUsage?(detail: ContextDetail): Promise<ContextUsage>;
  /**
   * Connects one of the session's MCP servers again: starts its login when it needs one (the URL
   * for the user to open; the session reports the server once connected), reconnects it
   * otherwise. Rejects when the server is unknown or the reconnection fails.
   */
  mcpConnect?(server: string): Promise<McpConnectResult>;
  /**
   * Nothing is lost by stopping it now: no turn running, no message waiting, no background task
   * (a shell, a Monitor, a subagent) that would wake the agent when it ends.
   */
  idle(): boolean;
  /**
   * Ends the session and confirms its tracked native root exited. Concurrent calls share
   * confirmation; a timeout/error must reject and remain retryable. This is not proof that
   * detached descendants stopped, or evidence about a previous sidecar/daemon generation.
   */
  stop(): Promise<void>;
}

export interface Agent {
  readonly kind: AgentKind;
  /** Runtime versions reported by `sidecar.ping`, e.g. `claude-agent-sdk` → `0.3.296`. */
  readonly versions: Record<string, string>;
  /** Builds a session without starting it. Throws an RpcError on invalid parameters. */
  createSession(params: SessionStartParams, host: SessionHost): AgentSession;
  /** The agent's model catalog; starts an agent process, so callers cache it. */
  listModels(): Promise<AgentModel[]>;
  /** The commands a session started in `cwd` with `directories` runs on `/name`; starts an agent process. */
  listCommands(cwd: string, directories: string[]): Promise<SlashCommand[]>;
  /** A short title for the work `text` describes; agents that cannot name work leave it out. */
  title?(text: string): Promise<string>;
  /** A workspace's title again from its conversation: the current one while it still fits. */
  retitle?(params: AgentRetitleParams): Promise<string>;
  /** A design brief written from a chat's conversation; agents that cannot write one leave it out. */
  brief?(params: AgentBriefParams): Promise<string>;
  /** One question answered in one turn, without tools; agents that cannot answer one leave it out. */
  ask?(params: AgentAskParams): Promise<string>;
  /**
   * What is left of the plan's usage limits, as the agent's own CLI reads them; `signed_out` when
   * the CLI is not logged in.
   */
  usage?(): Promise<AgentUsageResult>;
  /** Public native account metadata, without starting a turn or exposing credentials. */
  account?(): Promise<NativeAccountInfo>;
  /**
   * Starts logging the agent's CLI in: resolves with the page the user opens, then keeps waiting
   * for the login to complete (a few minutes at most) and calls `ended` once it did or gave up.
   * Headless, for a browser on another machine: a page that needs no callback to this one.
   */
  login?(ended: (success: boolean, error: string | null) => void, headless: boolean): Promise<AgentLoginResult>;
  /** Gives the headless login under way the code its page showed; absent when its page asks none. */
  loginCode?(code: string): Promise<void>;
  /** Ends the login under way, reported as not completed; nothing when none runs. */
  loginCancel?(): void;
  /**
   * A subagent's conversation from its transcript, without a session; absent from agents
   * without subagents of their own (Codex).
   */
  subagentMessages?(params: AgentSubagentMessagesParams): Promise<AgentSubagentMessagesResult>;
  /**
   * Copies a session into a new one of its own, without running it; resolves with the new id,
   * which `session.start` resumes. Absent from agents that cannot fork their sessions.
   */
  fork?(params: AgentForkParams): Promise<string>;
}
