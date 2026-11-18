// One Mistral Vibe chat: a `vibe-acp` process per session (its environment carries the workspace's
// variables), one ACP session in it (docs/adr/mistral-vibe-third-agent.md).
//
// The session runs in Vibe's `ask` mode whatever the chat's permission mode: every tool call Vibe
// gates comes as `session/request_permission`, which acp/permissions.ts allows, refuses or puts to
// the user. ACP has no steering: a message sent while a turn runs waits for its end, then goes with
// the others as the next prompt. ACP has no system prompt either: Chauffeur's instructions go
// before the first message of a new session, which Vibe keeps in its history.

import { readFile } from "node:fs/promises";
import { basename } from "node:path";
import { errorMessage } from "../../log.ts";
import { ErrorCode, type Attachment, type ChatEvent, type PermissionMode, type RequestResponse, type SessionStartParams, type SessionStartResult } from "../../protocol.ts";
import { RpcError, type RpcId } from "../../rpc/peer.ts";
import { settlesWithin } from "../../util/settles-within.ts";
import { SIDECAR_VERSION, VIBE_VERSION } from "../../version.ts";
import type { AgentSession, SessionChanges, SessionHost } from "../agent.ts";
import { attachmentNote, isImage } from "../attachments.ts";
import { notice } from "../claude/events.ts";
import { PLAN_NOTE } from "../codex/options.ts";
import { DESIGN_BROWSER, designBrowserCommand } from "../design-browser.ts";
import { PendingRequests, type Outcome } from "../pending.ts";
import { CHAUFFEUR_TOOLS, TOOLS_DESCRIPTION } from "../repo-tools.ts";
import type { AcpConnection, SpawnAcp } from "../acp/connection.ts";
import { AcpEventMapper, describe, type AcpToolCall } from "../acp/events.ts";
import { decide } from "../acp/permissions.ts";
import { ACP_PROTOCOL_VERSION, type ContentBlock, type InitializeResponse, type McpServerSpec, type NewSessionResponse, type PermissionOption, type PromptResponse, type RequestPermissionOutcome, type RequestPermissionRequest, type SessionNotification } from "../acp/protocol.ts";
import { serveTools, type ToolsServer } from "../acp/tools-server.ts";
import { vibeChauffeurTools } from "./tools.ts";

export interface VibeSessionDeps {
  spawn: SpawnAcp;
  /** The vibe-acp binary for this environment. May throw an RpcError. */
  resolveExecutable: (env: Record<string, string | undefined>) => string;
  baseEnv: Record<string, string | undefined>;
  /** How long `start` waits for Vibe to initialize and open the session. */
  initTimeoutMs?: number;
  /** Bounds shutdown confirmation; timeout does not release the retained session. */
  stopTimeoutMs?: number;
  newRequestId?: () => string;
  /** Receives every notification and request from Vibe (CHAUFFEUR_SIDECAR_TRACE). */
  trace?: (chatId: string, message: unknown) => void;
  /** How long Chauffeur's waiting tools wait before answering (see tools.ts). */
  answerWithinMs?: number;
}

/** Vibe's mode in which every tool call it gates asks the client. */
export const VIBE_ASK_MODE = "ask";

type State = "new" | "starting" | "running" | "stopping" | "ended";

/** A user message waiting for a turn; the id is the daemon's, for `user_message_delivered`. */
type Message = { text: string; attachments: Attachment[]; id?: string | undefined };

export class VibeSession implements AgentSession {
  private readonly mapper = new AcpEventMapper();
  private readonly requests: PendingRequests;
  /** The rules the user allowed always in this chat. */
  private readonly always: Set<string>;
  private connection: AcpConnection | undefined;
  private tools: ToolsServer | undefined;
  private state: State = "new";
  private stopping: Promise<void> | undefined;
  private sessionId = "";
  private failStart: ((error: unknown) => void) | undefined;
  /** Between our `turn_started` and `turn_ended`. */
  private busy = false;
  /** A `session/prompt` is running. */
  private prompting = false;
  private readonly queue: Message[] = [];
  private mode: PermissionMode;
  /** Settings changed while a turn ran, sent before the next prompt. */
  private pendingModel: string | undefined;
  private pendingEffort: string | undefined;
  /** Aborted by the user's interrupt or the session's stop (Chauffeur's tools, delegated chats). */
  private interruption = new AbortController();
  /** `session/load` replays the conversation as updates: not the chat's to show again. */
  private loading = false;
  /** Chauffeur's instructions, still to go before the first message of a new session. */
  private instructions: string | undefined;
  /** Refusals in the turn, told to the agent at once (guards) or with the next message (the user's). */
  private refusals: string[] = [];
  /** The turn's step was cancelled for a guard's refusal: Vibe is prompted again with why. */
  private cancelledForRefusal = false;
  /** The user interrupted the turn. */
  private userInterrupted = false;
  /** Told the daemon once that Vibe refused its credentials. */
  private accountLost = false;
  /** Said in the chat once the session has started. */
  private pendingNotice: string | undefined;

  constructor(
    private readonly params: SessionStartParams,
    private readonly host: SessionHost,
    private readonly deps: VibeSessionDeps,
  ) {
    this.requests = new PendingRequests((event) => this.emit(event), deps.newRequestId);
    this.always = new Set(params.always_allow ?? []);
    this.mode = params.permission_mode;
    this.instructions = params.resume_session_id ? undefined : params.append_system_prompt?.trim() || undefined;
  }

  async start(): Promise<SessionStartResult> {
    if (this.state !== "new") throw new RpcError(ErrorCode.CONFLICT, "session already started");
    this.state = "starting";
    const env = { ...this.deps.baseEnv, ...this.params.env };
    let connection: AcpConnection;
    try {
      const executable = this.deps.resolveExecutable(env);
      connection = this.deps.spawn({
        executable,
        args: [],
        cwd: this.params.design_dir ?? this.params.cwd,
        env,
        log: this.host.log,
        name: "vibe-acp",
        handlers: {
          notification: (method, params) => this.onNotification(method, params),
          request: (method, params, id) => this.onRequest(method, params, id),
        },
      });
    } catch (error) {
      this.state = "ended";
      throw error instanceof RpcError ? error : new RpcError(ErrorCode.UNAVAILABLE, `Mistral Vibe failed to start: ${errorMessage(error)}`);
    }
    this.connection = connection;
    void connection.exited.then((reason) => this.onExit(reason));

    const failed = new Promise<never>((_, reject) => {
      this.failStart = reject;
    });
    let timer: ReturnType<typeof setTimeout> | undefined;
    const timeoutMs = this.deps.initTimeoutMs ?? 60_000;
    const timedOut = new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error(`no answer after ${timeoutMs / 1000}s`)), timeoutMs);
    });
    let model: string | null;
    try {
      model = await Promise.race([this.openSession(connection), failed, timedOut]);
    } catch (error) {
      this.state = "ended";
      this.tools?.close();
      void connection.close().catch((error: unknown) => this.host.log.error(`Vibe startup cleanup remains unconfirmed: ${errorMessage(error)}`));
      if (isSignedOut(error)) this.loseAccount(errorMessage(error));
      throw new RpcError(ErrorCode.UNAVAILABLE, `Mistral Vibe failed to start: ${errorMessage(error)}`);
    } finally {
      clearTimeout(timer);
      this.failStart = undefined;
    }
    if (this.state !== "starting") throw new RpcError(ErrorCode.UNAVAILABLE, "the session was stopped while starting");
    this.state = "running";
    this.mapper.model = model;
    this.emit({ type: "session_started", agent_session_id: this.sessionId, model });
    if (this.pendingNotice) this.emit(notice("warning", this.pendingNotice));
    this.pendingNotice = undefined;
    if (this.params.prompt || this.params.prompt_attachments.length > 0) {
      this.queue.unshift({ text: this.params.prompt ?? "", attachments: this.params.prompt_attachments });
    }
    this.flushQueue();
    return { agent_session_id: this.sessionId, ...(connection.pid === undefined ? {} : { native_pid: connection.pid }) };
  }

  /** initialize → session/new or session/load → Vibe's ask mode, the chat's model and effort. */
  private async openSession(connection: AcpConnection): Promise<string | null> {
    const init = await connection.request<InitializeResponse>("initialize", {
      protocolVersion: ACP_PROTOCOL_VERSION,
      clientCapabilities: { fs: { readTextFile: false, writeTextFile: false }, terminal: false },
      clientInfo: { name: "chauffeur", title: "Chauffeur", version: SIDECAR_VERSION },
    });
    const version = init.agentInfo?.version;
    if (version && version !== VIBE_VERSION) this.host.log.warn(`vibe ${version} differs from the version Chauffeur was tried with (${VIBE_VERSION})`);
    const mcpServers = this.mcpServers();
    const directories = this.params.design_dir ? [] : this.params.additional_directories;
    let opened: NewSessionResponse;
    if (this.params.resume_session_id) {
      this.loading = true;
      try {
        opened = await connection.request<NewSessionResponse>("session/load", {
          sessionId: this.params.resume_session_id,
          cwd: this.params.design_dir ?? this.params.cwd,
          mcpServers,
          ...(directories.length > 0 ? { additionalDirectories: directories } : {}),
        });
      } finally {
        this.loading = false;
      }
      this.sessionId = this.params.resume_session_id;
    } else {
      const open = () =>
        connection.request<NewSessionResponse>("session/new", {
          cwd: this.params.design_dir ?? this.params.cwd,
          mcpServers,
          ...(directories.length > 0 ? { additionalDirectories: directories } : {}),
        });
      opened = await open();
      this.sessionId = opened.sessionId;
      // Trust takes effect from the next session: the first, still empty, is left (Vibe keeps none
      // that never got a prompt).
      if (await this.trustWorkspace(connection, opened)) {
        opened = await open();
        this.sessionId = opened.sessionId;
      }
    }
    if (opened.modes?.currentModeId !== VIBE_ASK_MODE) {
      await connection.request("session/set_mode", { sessionId: this.sessionId, modeId: VIBE_ASK_MODE });
    }
    let model = currentValue(opened, "model");
    if (this.params.model && this.params.model !== model) {
      if (offers(opened, "model", this.params.model)) {
        await this.setOption(connection, "model", this.params.model);
        model = this.params.model;
      } else {
        this.host.log.warn(`Mistral Vibe offers no model ${this.params.model}`);
        this.pendingNotice = `Mistral Vibe does not offer ${this.params.model} on this account: the chat runs on ${model ?? "its default model"}.`;
      }
    }
    if (this.params.effort && this.params.effort !== currentValue(opened, "thinking") && offers(opened, "thinking", this.params.effort)) {
      await this.setOption(connection, "thinking", this.params.effort);
    }
    return model;
  }

  /**
   * Vibe reads a folder's AGENTS.md, where Chauffeur writes the workspace's and the repos'
   * instructions, only once the user trusted the folder. Chauffeur made the workspace folder: it is
   * trusted for Vibe (`trust_cwd`, kept in Vibe's own trust store). A design chat's folder has no
   * AGENTS.md: left as it is. True when it was trusted now.
   */
  private async trustWorkspace(connection: AcpConnection, opened: NewSessionResponse): Promise<boolean> {
    if (this.params.design_dir) return false;
    const trust = (opened._meta?.workspace_trust as { status?: string } | undefined)?.status;
    if (trust !== "untrusted") return false;
    try {
      await connection.request("_trust/decision", { sessionId: this.sessionId, decision: "trust_cwd", cwd: this.params.cwd });
      return true;
    } catch (error) {
      this.host.log.warn(`Vibe did not trust the workspace folder: ${errorMessage(error)}`);
      return false;
    }
  }

  private async setOption(connection: AcpConnection, configId: string, value: string): Promise<void> {
    await connection.request("session/set_config_option", { sessionId: this.sessionId, configId, value });
  }

  /** Chauffeur's tools (not in a design chat), and a design chat's headless browser. */
  private mcpServers(): McpServerSpec[] {
    const design = this.params.design_dir;
    if (design) {
      const browser = designBrowserCommand(design);
      return [{ name: DESIGN_BROWSER, command: browser.command, args: browser.args, env: [] }];
    }
    this.tools = serveTools(
      CHAUFFEUR_TOOLS,
      TOOLS_DESCRIPTION,
      vibeChauffeurTools(this.host, this.requests, {
        delegate: !!this.params.delegate,
        interrupted: () => this.interruption.signal,
        followUp: (text) => this.followUp(text),
        ...(this.deps.answerWithinMs === undefined ? {} : { answerWithinMs: this.deps.answerWithinMs }),
      }),
      this.host.log,
    );
    return [this.tools.spec];
  }

  send(text: string, attachments: Attachment[] = [], messageId?: string): void {
    if (this.state === "stopping" || this.state === "ended") {
      throw new RpcError(ErrorCode.NOT_FOUND, "the session is stopping");
    }
    this.queue.push({ text, attachments, id: messageId });
    this.flushQueue();
  }

  /** A message of Chauffeur's for the agent (an answer that came after its tool call). */
  private followUp(text: string): void {
    if (this.state !== "running") return;
    this.queue.push({ text, attachments: [] });
    this.flushQueue();
  }

  idle(): boolean {
    return this.state === "running" && !this.busy && !this.prompting && this.queue.length === 0 && !this.requests.waiting();
  }

  async interrupt(): Promise<void> {
    this.userInterrupted = true;
    this.requests.cancelAll();
    this.interruption.abort();
    this.interruption = new AbortController();
    if (this.state !== "running" || !this.connection || !this.prompting) return;
    this.connection.notify("session/cancel", { sessionId: this.sessionId });
  }

  respond(requestId: string, response: RequestResponse): void {
    this.requests.respond(requestId, response);
  }

  async stopTask(): Promise<void> {
    throw new RpcError(ErrorCode.BAD_REQUEST, "Mistral Vibe has no task to stop: interrupt the chat instead");
  }

  async configure(changes: SessionChanges): Promise<void> {
    if (this.state !== "running") throw new RpcError(ErrorCode.CONFLICT, "the session is not running");
    if (changes.effort !== undefined && changes.effort.trim() === "") {
      throw new RpcError(ErrorCode.INVALID_PARAMS, "effort must not be empty");
    }
    if (changes.permissionMode !== undefined) this.mode = changes.permissionMode;
    if (changes.model !== undefined) this.pendingModel = changes.model;
    if (changes.effort !== undefined) this.pendingEffort = changes.effort;
    if (this.busy) {
      this.emit(notice("info", "Mistral Vibe applies model and effort changes from the next turn"));
      return;
    }
    await this.applySettings();
  }

  /** The model and effort changed since the last prompt, given to Vibe before the next one. */
  private async applySettings(): Promise<void> {
    const connection = this.connection;
    if (!connection) return;
    const model = this.pendingModel;
    const effort = this.pendingEffort;
    this.pendingModel = undefined;
    this.pendingEffort = undefined;
    if (model) {
      await this.setOption(connection, "model", model);
      this.mapper.model = model;
      this.emit({ type: "session_started", agent_session_id: this.sessionId, model });
    }
    if (effort) await this.setOption(connection, "thinking", effort);
  }

  stop(): Promise<void> {
    if (this.stopping) return this.stopping;
    const stopping = this.stopSession();
    this.stopping = stopping;
    void stopping.catch(() => {
      if (this.stopping === stopping) this.stopping = undefined;
    });
    return stopping;
  }

  private async stopSession(): Promise<void> {
    const running = this.state === "running";
    this.state = "stopping";
    this.failStart?.(new Error("the session was stopped while starting"));
    this.requests.cancelAll();
    this.interruption.abort();
    if (running && this.busy) this.endTurnLocally("interrupted", null);
    this.queue.length = 0;
    this.tools?.close();
    const connection = this.connection;
    if (connection && !(await settlesWithin((async () => { await connection.close(); await connection.exited; })(), this.deps.stopTimeoutMs ?? 15_000))) {
      throw new RpcError(ErrorCode.UNAVAILABLE, "Mistral Vibe's process exit is unconfirmed; keep the workspace fenced and retry stopping the session");
    }
    this.state = "ended";
  }

  /** Sends what waits as one prompt, once no turn runs. */
  private flushQueue(): void {
    if (this.state !== "running" || this.busy || this.prompting || this.queue.length === 0) return;
    void this.prompt(this.queue.splice(0, this.queue.length));
  }

  /**
   * One turn of the chat. When Chauffeur's guards refused a call, the turn's step is cancelled and
   * Vibe prompted again at once with why (ACP carries no reason with a refusal, so the model would
   * retry blindly): the chat sees one turn.
   */
  private async prompt(messages: Message[]): Promise<void> {
    const connection = this.connection;
    if (!connection) return;
    this.prompting = true;
    this.userInterrupted = false;
    this.beginTurn();
    const startedAt = Date.now();
    let outcome: { response?: PromptResponse; error?: string };
    try {
      if (this.pendingModel || this.pendingEffort) await this.applySettings();
      let prompt = await this.promptBlocks(messages);
      this.delivered(messages);
      for (;;) {
        outcome = { response: await connection.request<PromptResponse>("session/prompt", { sessionId: this.sessionId, prompt }) };
        const cancelledForRefusal = this.cancelledForRefusal;
        this.cancelledForRefusal = false;
        if (!cancelledForRefusal || this.userInterrupted || this.state !== "running" || this.refusals.length === 0) break;
        for (const event of this.mapper.closeOpen("Chauffeur refused this call.")) this.emit(event);
        prompt = this.refusalBlocks();
      }
    } catch (error) {
      if (isSignedOut(error)) this.loseAccount(errorMessage(error));
      outcome = { error: errorMessage(error) };
    } finally {
      this.prompting = false;
    }
    if (this.state !== "running") return;
    this.requests.cancelAll();
    this.busy = false;
    for (const event of this.mapper.endTurn(outcome, Date.now() - startedAt)) this.emit(event);
    this.flushQueue();
  }

  /** Why Chauffeur refused the calls of the turn, for the agent. */
  private refusalBlocks(): ContentBlock[] {
    const text = `Chauffeur refused these tool calls, and stopped your last step so that you read why:\n${this.refusals.map((r) => `- ${r}`).join("\n")}\nDo not retry them: carry on otherwise.`;
    this.refusals = [];
    return [{ type: "text", text }];
  }

  /** The prompt's blocks: Chauffeur's instructions first in a new session, then each message. */
  private async promptBlocks(messages: Message[]): Promise<ContentBlock[]> {
    const blocks: ContentBlock[] = [];
    if (this.instructions) {
      blocks.push({ type: "text", text: `<chauffeur-instructions>\n${this.instructions}\n</chauffeur-instructions>` });
      this.instructions = undefined;
    }
    if (this.refusals.length > 0) blocks.push(...this.refusalBlocks());
    const cwd = this.params.design_dir ?? this.params.cwd;
    for (const message of messages) {
      const body = this.mode === "plan" && !this.params.read_only && !this.params.design_dir ? `${PLAN_NOTE}\n\n${message.text}` : message.text;
      const text = `${body}${attachmentNote(message.attachments, cwd)}`.trimStart();
      if (text) blocks.push({ type: "text", text });
      for (const attachment of message.attachments) {
        if (!isImage(attachment)) continue;
        try {
          const data = await readFile(attachment.path);
          blocks.push({ type: "image", data: data.toString("base64"), mimeType: imageType(attachment.path), uri: `file://${attachment.path}` });
        } catch (error) {
          this.host.log.warn(`could not read ${basename(attachment.path)}: ${errorMessage(error)}`);
        }
      }
    }
    return blocks;
  }

  private delivered(messages: Message[]): void {
    for (const message of messages) {
      if (message.id) this.emit({ type: "user_message_delivered", message_id: message.id });
    }
  }

  private onNotification(method: string, params: unknown): void {
    this.deps.trace?.(this.host.chatId, { method, params });
    if (this.state === "ended" || method !== "session/update" || this.loading) return;
    const notification = params as SessionNotification;
    if (notification.sessionId !== this.sessionId) return;
    for (const event of this.mapper.map(notification.update)) this.emit(event);
  }

  private async onRequest(method: string, params: unknown, id: RpcId): Promise<unknown> {
    this.deps.trace?.(this.host.chatId, { method, id, params });
    if (method === "session/request_permission") return { outcome: await this.permission(params as RequestPermissionRequest) };
    this.host.log.warn(`unhandled Vibe request ${method}`);
    throw new RpcError(ErrorCode.METHOD_NOT_FOUND, `Chauffeur does not handle ${method}`);
  }

  /** Allows, refuses or asks, as acp/permissions.ts decides; the user's answer becomes Vibe's option. */
  private async permission(request: RequestPermissionRequest): Promise<RequestPermissionOutcome> {
    const tool = this.mapper.tool(request.toolCall.toolCallId) ?? describe(request.toolCall);
    const decision = decide(tool, {
      mode: this.mode,
      readOnly: this.params.read_only,
      designDir: this.params.design_dir ?? undefined,
      cwd: this.params.design_dir ?? this.params.cwd,
      roots: [this.params.cwd, ...this.params.additional_directories],
    });
    if (decision.type === "allow") return select(request.options, "allow_once");
    if (decision.type === "deny") {
      this.refusals.push(`${tool.title ?? tool.name}: ${decision.reason}`);
      this.emit(notice("warning", decision.reason));
      if (!this.cancelledForRefusal && this.prompting && this.connection) {
        this.cancelledForRefusal = true;
        this.connection.notify("session/cancel", { sessionId: this.sessionId });
      }
      return select(request.options, "reject_once");
    }
    const rule = alwaysRule(tool);
    if (this.always.has(rule)) return select(request.options, "allow_once");
    const outcome: Outcome = await this.requests.ask(
      "permission",
      (requestId) => ({ type: "permission_request", request_id: requestId, tool_name: tool.name, input: tool.input, title: tool.title }),
      { subject: { toolName: tool.name, input: tool.input, rules: [rule] }, resolution: (o) => (o.type === "allow" && o.always ? { rules: [rule] } : {}) },
    );
    switch (outcome.type) {
      case "allow":
        if (outcome.always) {
          this.always.add(rule);
          this.requests.allowCovered((call) => {
            const rules = call.rules ?? [];
            return rules.length > 0 && rules.every((r) => this.always.has(r)) ? rules : undefined;
          }, true);
        }
        return select(request.options, "allow_once");
      case "cancelled":
        return { outcome: "cancelled" };
      default:
        if (outcome.type === "deny" && outcome.message) this.refusals.push(`${tool.title ?? tool.name}: the user declined: ${outcome.message}`);
        return select(request.options, "reject_once");
    }
  }

  /** Vibe refused its credentials: the daemon marks Vibe signed out (once a session). */
  private loseAccount(message: string): void {
    if (this.accountLost) return;
    this.accountLost = true;
    this.host.emit({ type: "account_lost", message });
  }

  private onExit(reason: string): void {
    this.failStart?.(new Error(`vibe-acp ${reason}`));
    if (this.state !== "running") return;
    // Vibe died on its own: report it and let the daemon resume the session.
    this.state = "ended";
    this.tools?.close();
    this.host.log.error(`vibe-acp ${reason}`);
    this.requests.cancelAll();
    this.host.emit(notice("error", `Mistral Vibe session ended: vibe-acp ${reason}`));
    if (this.busy) this.endTurnLocally("error", `vibe-acp ${reason}`, true);
    this.host.ended();
  }

  private beginTurn(): void {
    if (this.busy) return;
    this.busy = true;
    this.emit({ type: "turn_started" });
  }

  private endTurnLocally(outcome: "error" | "interrupted", error: string | null, force = false): void {
    this.busy = false;
    this.prompting = false;
    const event: ChatEvent = { type: "turn_ended", outcome, duration_ms: null, cost_usd: null, context_tokens: null, error };
    if (force) this.host.emit(event);
    else this.emit(event);
  }

  private emit(event: ChatEvent): void {
    if (this.state === "ended" || this.state === "new") return;
    this.host.emit(event);
  }
}

/** The option of `kind` Vibe offers, the first one when it offers none such. */
function select(options: PermissionOption[], kind: "allow_once" | "reject_once"): RequestPermissionOutcome {
  const option = options.find((o) => o.kind === kind) ?? options.find((o) => o.kind.startsWith(kind.split("_")[0] ?? "")) ?? options[0];
  return option ? { outcome: "selected", optionId: option.optionId } : { outcome: "cancelled" };
}

/** What Always allow on a call stands for: its exact command, or the tool itself. */
export function alwaysRule(tool: AcpToolCall): string {
  return tool.command !== null ? `Bash(${tool.command})` : tool.name;
}

/** The session's option `id` takes `value`; true when Vibe did not list the option at all. */
function offers(session: NewSessionResponse, id: string, value: string): boolean {
  const option = session.configOptions?.find((o) => o.id === id);
  return !option || option.options.some((o) => o.value === value);
}

function currentValue(session: NewSessionResponse, id: string): string | null {
  return session.configOptions?.find((option) => option.id === id)?.currentValue ?? null;
}

/** Vibe's error when it has no key, or the key was refused. */
export function isSignedOut(error: unknown): boolean {
  const message = errorMessage(error);
  return /missing api key|unauthori[sz]ed|401|invalid api key/i.test(message);
}

function imageType(path: string): string {
  const extension = path.slice(path.lastIndexOf(".") + 1).toLowerCase();
  return extension === "jpg" || extension === "jpeg" ? "image/jpeg" : extension === "gif" ? "image/gif" : extension === "webp" ? "image/webp" : "image/png";
}
