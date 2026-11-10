import type {
  McpServerStatus,
  ModelInfo,
  Options,
  SDKControlGetContextUsageResponse,
  SDKControlInitializeResponse,
  SDKMessage,
  SDKUserMessage,
  SlashCommand,
} from "@anthropic-ai/claude-agent-sdk";
import type { SessionHost } from "../../src/agents/agent.ts";
import type { QueryLike } from "../../src/agents/claude/session.ts";
import { silentLogger } from "../../src/log.ts";
import type {
  ChatEvent,
  ToolAddRepoResult,
  ToolDelegateParams,
  ToolDelegateResult,
  ToolDelegateStarted,
  ToolListReposResult,
} from "../../src/protocol.ts";
import { AsyncQueue } from "../../src/util/async-queue.ts";

export { flush } from "./flush.ts";

export class FakeHost implements SessionHost {
  readonly chatId = "chat-1";
  readonly log = silentLogger;
  readonly events: ChatEvent[] = [];
  readonly worktrees: string[] = [];
  readonly removed: string[] = [];
  endedCount = 0;
  worktreeError: Error | undefined;

  emit(event: ChatEvent): void {
    this.events.push(event);
  }

  async createWorktree(name: string): Promise<string> {
    if (this.worktreeError) throw this.worktreeError;
    this.worktrees.push(name);
    return `/workspaces/w/.worktrees/${name}`;
  }

  async removeWorktree(path: string): Promise<void> {
    if (this.worktreeError) throw this.worktreeError;
    this.removed.push(path);
  }

  /** The repos `listRepos` answers: web in the workspace, analytics not. */
  repos: ToolListReposResult = {
    workspace: "w",
    explore: false,
    state: "ready",
    repos: [
      { name: "web", description: "The monolith", path: "/workspaces/w/web" },
      { name: "analytics", description: "Dashboards", path: null },
    ],
  };
  readonly added: string[] = [];
  addRepoError: Error | undefined;

  async listRepos(): Promise<ToolListReposResult> {
    return this.repos;
  }

  async addRepo(repo: string): Promise<ToolAddRepoResult> {
    if (this.addRepoError) throw this.addRepoError;
    this.added.push(repo);
    return { path: `/workspaces/w/${repo}`, branch: "tester/w", setup: true };
  }

  /** The tasks given to subagents, and the subagents interrupted. */
  readonly delegated: Array<Omit<ToolDelegateParams, "chat_id">> = [];
  readonly stopped: string[] = [];
  readonly detached: string[] = [];
  delegateError: Error | undefined;
  /** What the daemon says it chose otherwise than asked. */
  delegateNotice: string | null = null;
  private readonly answers = new Map<string, (result: ToolDelegateResult) => void>();

  async delegate(params: Omit<ToolDelegateParams, "chat_id">): Promise<ToolDelegateStarted> {
    if (this.delegateError) throw this.delegateError;
    this.delegated.push(params);
    return {
      chat: params.chat ?? `sub-${this.delegated.length}`,
      agent: params.agent ?? "claude",
      title: "Review the diff",
      model: params.model ?? null,
      effort: params.effort ?? null,
      notice: this.delegateNotice,
    };
  }

  /** Pending until the test answers for the subagent (`answerFor`). */
  delegateWait(child: string): Promise<ToolDelegateResult> {
    return new Promise((resolve) => this.answers.set(child, resolve));
  }

  answerFor(child: string, result: Partial<ToolDelegateResult> = {}): void {
    this.answers.get(child)?.({
      chat: child,
      agent: "codex",
      title: "Review the diff",
      status: "idle",
      outcome: "success",
      error: null,
      text: "LGTM",
      ...result,
    });
  }

  async delegateStop(child: string): Promise<void> {
    this.stopped.push(child);
  }

  async delegateDetach(child: string): Promise<void> {
    this.detached.push(child);
  }

  ended(): void {
    this.endedCount++;
  }

  types(): string[] {
    return this.events.map((event) => event.type);
  }
}

/** Stands in for the SDK's Query: the test pushes SDK messages and reads what the session sent. */
export class FakeQuery implements QueryLike {
  readonly sent: SDKUserMessage[] = [];
  readonly calls: string[] = [];
  readonly options: Options;
  private readonly output = new AsyncQueue<SDKMessage | Error>();
  private readonly init: Promise<SDKControlInitializeResponse>;
  private settleInit!: (error?: Error) => void;
  private sentWaiters: Array<() => void> = [];
  applyFlagSettingsError: Error | undefined;
  /** What the initialize handshake answers: fill it in, the handshake may already have settled. */
  initResult: Record<string, unknown> = {};

  constructor(prompt: AsyncIterable<SDKUserMessage>, options: Options, initialized = true) {
    this.options = options;
    this.init = new Promise((resolve, reject) => {
      this.settleInit = (error) => (error ? reject(error) : resolve(this.initResult as SDKControlInitializeResponse));
    });
    if (initialized) this.settleInit();
    void this.consume(prompt);
  }

  /** Completes (or fails) the initialize handshake. */
  initialize(error?: Error): void {
    this.settleInit(error);
  }

  push(...messages: SDKMessage[]): void {
    for (const message of messages) this.output.push(message);
  }

  /** Makes the query's iteration throw, as a crashed CLI would. */
  crash(error: Error): void {
    this.output.push(error);
  }

  /** Ends the iteration, as a CLI exiting on its own would. */
  exit(): void {
    this.output.end();
  }

  /** Resolves once the session has handed `count` messages to the SDK. */
  async waitForSent(count: number): Promise<void> {
    while (this.sent.length < count) await new Promise<void>((resolve) => this.sentWaiters.push(resolve));
  }

  async *[Symbol.asyncIterator](): AsyncIterator<SDKMessage> {
    for await (const item of this.output) {
      if (item instanceof Error) throw item;
      yield item;
    }
  }

  initializationResult(): Promise<SDKControlInitializeResponse> {
    return this.init;
  }

  async interrupt(): Promise<undefined> {
    this.calls.push("interrupt");
    return undefined;
  }

  async setModel(model?: string): Promise<void> {
    this.calls.push(`setModel:${model}`);
  }

  /** Errors `setPermissionMode` throws, by mode (Claude Code refusing auto mode). */
  permissionModeErrors: Record<string, Error> = {};

  async setPermissionMode(mode: string): Promise<void> {
    const error = this.permissionModeErrors[mode];
    if (error) throw error;
    this.calls.push(`setPermissionMode:${mode}`);
  }

  async applyFlagSettings(settings: Record<string, unknown>): Promise<void> {
    if (this.applyFlagSettingsError) throw this.applyFlagSettingsError;
    this.calls.push(`applyFlagSettings:${JSON.stringify(settings)}`);
  }

  /** Rows `supportedModels` answers with. */
  models: ModelInfo[] = [];

  async supportedModels(): Promise<ModelInfo[]> {
    this.calls.push("supportedModels");
    return this.models;
  }

  /** Rows `supportedCommands` answers with. */
  commands: SlashCommand[] = [];

  async supportedCommands(): Promise<SlashCommand[]> {
    this.calls.push("supportedCommands");
    return this.commands;
  }

  /** What `getContextUsage` answers: the share of the context in use, in %. */
  contextPercentage = 0;
  /** The details asked of `getContextUsage`, in order. */
  readonly contextReads: string[] = [];
  contextError: Error | undefined;

  async getContextUsage(opts?: { detail?: "summary" | "full" }): Promise<SDKControlGetContextUsageResponse> {
    this.contextReads.push(opts?.detail ?? "full");
    if (this.contextError) throw this.contextError;
    const total = this.contextPercentage * 2_000;
    return {
      percentage: this.contextPercentage,
      totalTokens: total,
      maxTokens: 200_000,
      rawMaxTokens: 200_000,
      model: "claude-opus-5-5",
      categories: [
        { name: "System prompt", tokens: 3_000, color: "promptBorder", kind: "used" },
        { name: "Messages", tokens: Math.max(0, total - 3_000), color: "purple", kind: "used" },
        { name: "MCP tools (deferred)", tokens: 9_000, color: "inactive", isDeferred: true, kind: "deferred" },
        { name: "Autocompact buffer", tokens: 33_000, color: "inactive", kind: "buffer" },
        { name: "Free space", tokens: 167_000 - total, color: "promptBorder", kind: "free" },
      ],
      gridRows: [],
      memoryFiles: [{ path: "/w/CLAUDE.md", type: "Project", tokens: 120 }],
      mcpTools: [
        { name: "mcp__linear__get_issue", serverName: "linear", tokens: 400, isLoaded: false },
        { name: "mcp__linear__list_issues", serverName: "linear", tokens: 600 },
      ],
      skills: { totalSkills: 1, includedSkills: 1, tokens: 80, skillFrontmatter: [{ name: "release", source: "projectSettings", tokens: 80 }] },
      agents: [{ agentType: "reviewer", source: "projectSettings", tokens: 50 }],
      autoCompactThreshold: 167_000,
      isAutoCompactEnabled: true,
      apiUsage: null,
    } as SDKControlGetContextUsageResponse;
  }

  stopTaskError: Error | undefined;

  async stopTask(taskId: string): Promise<void> {
    if (this.stopTaskError) throw this.stopTaskError;
    this.calls.push(`stopTask:${taskId}`);
  }

  /** The servers `mcpServerStatus` reports. */
  mcpServers: McpServerStatus[] = [];
  /** What `mcpAuthenticate` answers. */
  mcpAuthAnswer: { authUrl?: string; requiresUserAction?: boolean; callbackExpected?: boolean } = {
    authUrl: "https://mcp.example.com/authorize",
    requiresUserAction: true,
  };
  reconnectError: Error | undefined;

  async mcpServerStatus(): Promise<McpServerStatus[]> {
    return this.mcpServers;
  }

  async reconnectMcpServer(name: string): Promise<void> {
    this.calls.push(`reconnectMcpServer:${name}`);
    if (this.reconnectError) throw this.reconnectError;
  }

  async mcpAuthenticate(name: string): Promise<unknown> {
    this.calls.push(`mcpAuthenticate:${name}`);
    return this.mcpAuthAnswer;
  }

  close(): void {
    this.calls.push("close");
    this.output.end();
  }

  private async consume(prompt: AsyncIterable<SDKUserMessage>): Promise<void> {
    for await (const message of prompt) {
      this.sent.push(message);
      const waiters = this.sentWaiters;
      this.sentWaiters = [];
      for (const resolve of waiters) resolve();
    }
  }
}

/** Minimal SDK messages, typed loosely: tests only fill the fields the sidecar reads. */
export const sdk = {
  init(sessionId = "sess-1", model = "claude-haiku-4-5-20251001"): SDKMessage {
    return { type: "system", subtype: "init", session_id: sessionId, model } as SDKMessage;
  },
  text(id: string, text: string, parent: string | null = null): SDKMessage {
    return {
      type: "assistant",
      parent_tool_use_id: parent,
      message: { id, content: [{ type: "text", text }], usage: { input_tokens: 10, output_tokens: 5 } },
    } as unknown as SDKMessage;
  },
  toolUse(id: string, toolUseId: string, name: string, input: unknown): SDKMessage {
    return {
      type: "assistant",
      parent_tool_use_id: null,
      message: { id, content: [{ type: "tool_use", id: toolUseId, name, input }] },
    } as unknown as SDKMessage;
  },
  toolResult(toolUseId: string, content: unknown, isError = false): SDKMessage {
    return {
      type: "user",
      parent_tool_use_id: null,
      message: { role: "user", content: [{ type: "tool_result", tool_use_id: toolUseId, content, is_error: isError }] },
    } as unknown as SDKMessage;
  },
  suggestion(text: string): SDKMessage {
    return { type: "prompt_suggestion", suggestion: text, uuid: "u-suggestion", session_id: "sess-1" } as unknown as SDKMessage;
  },
  result(fields: Record<string, unknown> = {}): SDKMessage {
    return {
      type: "result",
      subtype: "success",
      is_error: false,
      duration_ms: 1200,
      total_cost_usd: 0.01,
      num_turns: 1,
      result: "done",
      usage: { input_tokens: 10, output_tokens: 5, cache_read_input_tokens: 100, cache_creation_input_tokens: 0 },
      errors: [],
      ...fields,
    } as unknown as SDKMessage;
  },
};
