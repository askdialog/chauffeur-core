// Maps Claude Agent SDK messages onto agent-agnostic ChatEvents. Stateful per session: it pairs
// streamed deltas with final blocks (stable message ids), tracks the context size, and reports
// session ids once.

import type {
  SDKAssistantMessage,
  SDKMessage,
  SDKPartialAssistantMessage,
  SDKResultMessage,
  SDKUserMessage,
} from "@anthropic-ai/claude-agent-sdk";
import type { ChatEvent, ImageInput, JsonValue, NoticeLevel, TurnOutcome } from "../../protocol.ts";
import { ClaudeTasks } from "./tasks.ts";

/** Tool outputs are cut to this many characters. */
export const TOOL_OUTPUT_LIMIT = 20_000;

export interface MapContext {
  /** An interrupt was requested during the turn a `result` ends. */
  interrupted?: boolean;
}

type Usage = {
  input_tokens?: number | null;
  output_tokens?: number | null;
  cache_creation_input_tokens?: number | null;
  cache_read_input_tokens?: number | null;
};

interface StreamedBlock {
  index: number;
  type: string;
  claimed: boolean;
}

/** System subtypes and message types that carry nothing the chat shows. */
const IGNORED_SYSTEM = new Set([
  "status",
  "hook_started",
  "hook_progress",
  "hook_response",
  // The session counts background tasks from it; the task_* messages say the rest.
  "background_tasks_changed",
  "commands_changed",
  // Signals for hosts that a shell command committed, pushed, merged or published a change; the
  // diff and PR state are read from git and GitHub anyway.
  "vcs_state_changed",
  "code_change_published",
  "control_request_progress",
  "elicitation_complete",
  "files_persisted",
  "memory_recall",
  "plugin_install",
  // An empty transcript row Claude Code (2.1.302+) stores as the target of a later rewind.
  "rewind_stand_in",
  "session_state_changed",
  // Claude Code's own title for the session (a forked one's, "<title> (fork)"): the chat keeps its
  // own title.
  "session_title_changed",
  "thinking_tokens",
  "worker_shutting_down",
]);
const IGNORED_TYPES = new Set([
  "tool_progress",
  "tool_use_summary",
  "conversation_reset",
  "keep_alive",
  "active_goal",
  // Per-message queued/started/completed frames, outside the SDK's public message union.
  "command_lifecycle",
]);

/**
 * What Claude Code's plugins draw in its terminal (panes, status lines, logs): nothing to show in
 * a chat, except their toasts.
 */
const TERMINAL_UI_SUBTYPES = new Set(["ui_log", "ui_status", "ui_panes", "ui_scroll", "ui_focus", "ui_invalidate"]);

/** `system/permission_check_status`, `@internal` in the SDK. `agent_id`: set inside a subagent. */
type PermissionCheckStatus = { subtype: string; tool_use_id?: unknown; status?: unknown; agent_id?: unknown };

export class ClaudeEventMapper {
  private session: { id: string; model: string | null } | undefined;
  /** Stream key (parent tool use id, "" for the main thread) → API message id being streamed. */
  private readonly streaming = new Map<string, string>();
  /** API message id → content blocks announced by the stream, in order. */
  private readonly streamedBlocks = new Map<string, StreamedBlock[]>();
  /** API message id → blocks delivered for it that the stream did not announce. */
  private readonly unstreamedBlocks = new Map<string, number>();
  /** Usage of the main thread's last API call: what the context holds after it. */
  private lastUsage: Usage | undefined;
  /** Transcript uuid of the main thread's last message in the turn: where a fork from it ends. */
  private lastMessage: string | undefined;
  private rateLimitStatus: string | undefined;
  private readonly reported = new Set<string>();
  /** Tool use id → the file a tool read, which names the images of its result. */
  private readonly readFiles = new Map<string, string>();
  /** The agent's tasks: background commands, subagents. */
  readonly tasks = new ClaudeTasks();
  /** Tool use ids whose automatic permission check (auto mode) is reported running. */
  private readonly checking = new Set<string>();

  map(message: SDKMessage, context: MapContext = {}): ChatEvent[] {
    switch (message.type) {
      case "system":
        return this.mapSystem(message);
      case "stream_event":
        return this.mapStreamEvent(message);
      case "assistant":
        if (message.parent_tool_use_id === null) this.lastMessage = message.uuid;
        return this.mapAssistant(message);
      case "user":
        if (message.parent_tool_use_id === null && message.uuid) this.lastMessage = message.uuid;
        return this.mapUser(message);
      case "result":
        // A foreground task cannot outlive its turn.
        return [...this.tasks.turnEnded(), this.turnEnded(message, context.interrupted === true)];
      case "prompt_suggestion":
        // Ephemeral: the daemon fans it out without keeping it.
        return message.suggestion.trim() ? [{ type: "prompt_suggestion", text: message.suggestion.trim() }] : [];
      case "auth_status":
        return message.error ? [notice("error", `Claude authentication failed: ${message.error}`)] : [];
      case "rate_limit_event": {
        const info = message.rate_limit_info;
        if (info.status === this.rateLimitStatus) return [];
        this.rateLimitStatus = info.status;
        if (info.status === "allowed") return [];
        const resets = info.resetsAt ? `, resets at ${new Date(info.resetsAt * 1000).toISOString()}` : "";
        const kind = info.rateLimitType ? ` (${info.rateLimitType})` : "";
        return info.status === "rejected"
          ? [notice("error", `Claude usage limit reached${kind}${resets}`)]
          : [notice("warning", `Claude usage limit nearly reached${kind}${resets}`)];
      }
      default:
        return this.unexpected(String((message as { type?: unknown }).type));
    }
  }

  private mapSystem(message: Extract<SDKMessage, { type: "system" }>): ChatEvent[] {
    // Outside the SDK's public union: a plugin's terminal UI.
    const ui = message as unknown as { subtype: string; plugin?: unknown; text?: unknown };
    if (TERMINAL_UI_SUBTYPES.has(ui.subtype)) return [];
    if (ui.subtype === "ui_toast") {
      return typeof ui.text === "string" && ui.text ? [notice("info", `${String(ui.plugin ?? "A plugin")}: ${ui.text}`)] : [];
    }
    if (ClaudeTasks.handles(message)) return this.tasks.map(message);
    // Outside the SDK's public union: auto mode's check of a tool call has run ~4 s and goes on
    // ("checking"), or is over ("done", which says nothing of the verdict).
    if (ui.subtype === "permission_check_status") return this.permissionCheck(message as unknown as PermissionCheckStatus);
    switch (message.subtype) {
      case "init": {
        const model = message.model || null;
        if (this.session?.id === message.session_id && this.session.model === model) return [];
        this.session = { id: message.session_id, model };
        return [{ type: "session_started", agent_session_id: message.session_id, model }];
      }
      case "api_retry": {
        const status = message.error_status ? ` ${message.error_status}` : "";
        const delay = Math.round(message.retry_delay_ms / 1000);
        return [
          notice(
            "warning",
            `Claude API error${status} (${message.error}), retry ${message.attempt}/${message.max_retries} in ${delay}s`,
          ),
        ];
      }
      case "compact_boundary":
        return [
          notice(
            "info",
            `Conversation compacted (${message.compact_metadata.trigger}, ${message.compact_metadata.pre_tokens} tokens before)`,
          ),
        ];
      case "informational":
        if (message.level === "info") return [];
        return [notice(message.level === "warning" ? "warning" : "info", message.content)];
      case "notification":
        return [notice("info", message.text)];
      case "local_command_output":
        return message.content ? [notice("info", message.content)] : [];
      case "mirror_error":
        return [notice("warning", `Transcript mirror failed: ${message.error}`)];
      case "permission_denied": {
        // Auto mode's classifier blocked a call: the agent reads why in the tool result, the user
        // sees it here. Other denials (rules, a PreToolUse hook) already show on the tool call.
        const checked = this.checkOver(message.tool_use_id);
        if (message.decision_reason_type !== "classifier") return checked;
        const blocked = `Auto mode blocked a ${message.tool_name} call`;
        const { category, sentence } = classifierReason(message.decision_reason || message.message);
        if (category) return [...checked, notice("warning", blocked, category)];
        return [...checked, notice("warning", sentence ? `${blocked}: ${sentence}` : blocked)];
      }
      case "model_refusal_fallback":
      case "model_refusal_no_fallback":
        return [notice("warning", message.content)];
      default:
        if (IGNORED_SYSTEM.has(message.subtype)) return [];
        return this.unexpected(`system/${String(message.subtype)}`);
    }
  }

  private permissionCheck(message: PermissionCheckStatus): ChatEvent[] {
    const id = message.tool_use_id;
    if (typeof id !== "string" || !id) return [];
    if (message.status === "checking") {
      this.checking.add(id);
      return [{ type: "permission_check", tool_use_id: id, checking: true }];
    }
    if (message.status === "done") {
      this.checking.delete(id);
      return [{ type: "permission_check", tool_use_id: id, checking: false }];
    }
    return [];
  }

  /**
   * Ends the check reported on a tool call, if one is: its verdict came another way (a denial, a
   * permission request), and the "done" frame may be lost.
   */
  checkOver(toolUseId: string | undefined): ChatEvent[] {
    if (!toolUseId || !this.checking.delete(toolUseId)) return [];
    return [{ type: "permission_check", tool_use_id: toolUseId, checking: false }];
  }

  private mapStreamEvent(message: SDKPartialAssistantMessage): ChatEvent[] {
    const key = message.parent_tool_use_id ?? "";
    const mainThread = message.parent_tool_use_id === null;
    // A subagent's stream is tied to the tool call that started it (Claude Code 2.1.302 forwards
    // a subagent's text whole, without a stream; any that comes streams to its view).
    const parent = message.parent_tool_use_id === null ? {} : { parent_tool_use_id: message.parent_tool_use_id };
    const event = message.event;
    switch (event.type) {
      case "message_start":
        this.streaming.set(key, event.message.id);
        this.streamedBlocks.set(event.message.id, []);
        if (mainThread) this.lastUsage = { ...event.message.usage };
        return [];
      case "message_delta":
        if (mainThread && this.lastUsage) {
          for (const [field, value] of Object.entries(event.usage)) {
            if (typeof value === "number") (this.lastUsage as Record<string, unknown>)[field] = value;
          }
        }
        return [];
      case "content_block_start": {
        const id = this.streaming.get(key);
        if (!id) return [];
        this.streamedBlocks.get(id)?.push({ index: event.index, type: event.content_block.type, claimed: false });
        const block = event.content_block;
        if (block.type === "text" && block.text) {
          return [{ type: "text_delta", message_id: `${id}:${event.index}`, text: block.text, ...parent }];
        }
        if (block.type === "thinking" && block.thinking) {
          return [{ type: "thinking_delta", message_id: `${id}:${event.index}`, text: block.thinking, ...parent }];
        }
        return [];
      }
      case "content_block_delta": {
        const id = this.streaming.get(key);
        if (!id) return [];
        const messageId = `${id}:${event.index}`;
        const delta = event.delta;
        if (delta.type === "text_delta" && delta.text) return [{ type: "text_delta", message_id: messageId, text: delta.text, ...parent }];
        if (delta.type === "thinking_delta" && delta.thinking) {
          return [{ type: "thinking_delta", message_id: messageId, text: delta.thinking, ...parent }];
        }
        return [];
      }
      default:
        return [];
    }
  }

  private mapAssistant(message: SDKAssistantMessage): ChatEvent[] {
    const api = message.message;
    const parent = message.parent_tool_use_id;
    if (parent === null && !this.streamedBlocks.has(api.id) && api.usage) this.lastUsage = { ...api.usage };
    const events: ChatEvent[] = [];
    for (const block of api.content) {
      const messageId = `${api.id}:${this.blockIndex(api.id, block.type)}`;
      switch (block.type) {
        case "text":
          if (block.text) events.push({ type: "assistant_text", message_id: messageId, text: block.text, parent_tool_use_id: parent });
          break;
        case "thinking":
          if (block.thinking) {
            // A subagent's thinking (forwarded with `forwardSubagentText`) belongs to its view.
            events.push({
              type: "thinking",
              message_id: messageId,
              text: block.thinking,
              ...(parent === null ? {} : { parent_tool_use_id: parent }),
            });
          }
          break;
        case "tool_use":
        case "server_tool_use":
        case "mcp_tool_use": {
          events.push({
            type: "tool_call",
            tool_use_id: block.id,
            name: block.name,
            input: toJson(block.input),
            parent_tool_use_id: parent,
          });
          const file = (block.input as { file_path?: unknown } | null)?.file_path;
          if (typeof file === "string") this.readFiles.set(block.id, file.split("/").pop() ?? file);
          break;
        }
        default:
          // Server-side tool results (web search, code execution…) arrive inside the assistant message.
          if ("tool_use_id" in block && typeof block.tool_use_id === "string" && block.type.endsWith("_tool_result")) {
            const content = "content" in block ? (block.content as unknown) : undefined;
            events.push({
              type: "tool_result",
              tool_use_id: block.tool_use_id,
              output: toolOutput(content),
              is_error: isErrorContent(content) || ("is_error" in block && block.is_error === true),
              ...withImages(toolImages(content)),
            });
          }
      }
    }
    return events;
  }

  private mapUser(message: SDKUserMessage): ChatEvent[] {
    const content = message.message.content;
    if (typeof content === "string") return [];
    const events: ChatEvent[] = [];
    for (const block of content) {
      if (block.type !== "tool_result") continue;
      events.push({
        type: "tool_result",
        tool_use_id: block.tool_use_id,
        output: toolOutput(block.content),
        is_error: block.is_error === true,
        ...withImages(named(toolImages(block.content), this.readFiles.get(block.tool_use_id))),
      });
      this.readFiles.delete(block.tool_use_id);
      this.checking.delete(block.tool_use_id);
      events.push(...this.tasks.toolResult(block.tool_use_id, block.is_error === true));
    }
    return events;
  }

  turnEnded(result: SDKResultMessage, interruptRequested: boolean): ChatEvent {
    const interrupted =
      interruptRequested || result.terminal_reason === "aborted_streaming" || result.terminal_reason === "aborted_tools";
    const failed = result.subtype !== "success" || result.is_error;
    const outcome: TurnOutcome = interrupted ? "interrupted" : failed ? "error" : "success";
    // A finished turn's transcript ends on a whole exchange: a fork can stop there.
    const forkPoint = outcome === "success" && this.session && this.lastMessage ? { session_id: this.session.id, up_to: this.lastMessage } : undefined;
    const event: ChatEvent = {
      type: "turn_ended",
      outcome,
      duration_ms: Number.isFinite(result.duration_ms) ? result.duration_ms : null,
      cost_usd: Number.isFinite(result.total_cost_usd) ? result.total_cost_usd : null,
      ...withSessionTokens(result),
      context_tokens: this.contextTokens(result),
      error: outcome === "error" ? resultError(result) : null,
      ...(forkPoint ? { fork_point: forkPoint } : {}),
    };
    this.lastMessage = undefined;
    this.streaming.clear();
    this.streamedBlocks.clear();
    this.unstreamedBlocks.clear();
    this.lastUsage = undefined;
    return event;
  }

  /** Tokens in context after the turn: the last main-thread API call's input plus its output. */
  private contextTokens(result: SDKResultMessage): number | null {
    // The result's usage sums every API call of the turn: it only equals the context for one call.
    const usage = this.lastUsage ?? (result.num_turns <= 1 ? (result.usage as Usage | undefined) : undefined);
    if (!usage) return null;
    const total =
      (usage.input_tokens ?? 0) +
      (usage.cache_creation_input_tokens ?? 0) +
      (usage.cache_read_input_tokens ?? 0) +
      (usage.output_tokens ?? 0);
    return total > 0 ? total : null;
  }

  /**
   * The index of a delivered block within its API message. The CLI delivers one assistant message
   * per completed block, all sharing the API message id; the stream announced the same blocks with
   * their indexes, so the first unclaimed streamed block of the same type is this one.
   */
  private blockIndex(messageId: string, type: string): number {
    const streamed = this.streamedBlocks.get(messageId);
    const match = streamed?.find((block) => !block.claimed && block.type === type);
    if (match) {
      match.claimed = true;
      return match.index;
    }
    const delivered = this.unstreamedBlocks.get(messageId) ?? 0;
    this.unstreamedBlocks.set(messageId, delivered + 1);
    return (streamed?.length ?? 0) + delivered;
  }

  private unexpected(kind: string): ChatEvent[] {
    if (this.reported.has(kind) || IGNORED_TYPES.has(kind)) return [];
    this.reported.add(kind);
    return [notice("info", `Chauffeur does not display Claude "${kind}" messages yet`)];
  }
}

/**
 * Tokens the session has used so far, from `modelUsage`: every model call of the query pipeline
 * (main loop, subagents, compaction), input with cache reads and writes, and output. A running
 * total like `total_cost_usd`; a zeroed one (a crash) is left out.
 */
function withSessionTokens(result: SDKResultMessage): { session_tokens?: number } {
  let total = 0;
  for (const usage of Object.values(result.modelUsage ?? {})) {
    for (const count of [usage.inputTokens, usage.cacheReadInputTokens, usage.cacheCreationInputTokens, usage.outputTokens]) {
      if (Number.isFinite(count) && count > 0) total += count;
    }
  }
  return total > 0 ? { session_tokens: total } : {};
}

export function notice(level: NoticeLevel, message: string, badge?: string): ChatEvent {
  return badge ? { type: "notice", level, message, badge } : { type: "notice", level, message };
}

function resultError(result: SDKResultMessage): string {
  if (result.subtype === "success") return result.result || "Claude ended the turn with an error";
  if (result.errors.length > 0) return result.errors.join("\n");
  switch (result.subtype) {
    case "error_max_turns":
      return "Claude reached the maximum number of turns";
    case "error_max_budget_usd":
      return "Claude reached the maximum budget";
    case "error_max_structured_output_retries":
      return "Claude could not produce a valid structured output";
    default:
      return "Claude failed during execution";
  }
}

/**
 * The images of a tool result's content, which the daemon keeps for the chat to show: Claude's
 * image blocks (an image file Read, a Playwright screenshot), MCP's (`data` and `mimeType`) and
 * Codex's (`imageUrl`, a data URL).
 */
export function toolImages(content: unknown): ImageInput[] {
  if (!Array.isArray(content)) return [];
  const images: ImageInput[] = [];
  for (const block of content) {
    if (typeof block !== "object" || block === null) continue;
    const typed = block as {
      type?: unknown;
      source?: { type?: unknown; data?: unknown; media_type?: unknown };
      data?: unknown;
      mimeType?: unknown;
      imageUrl?: unknown;
    };
    if (typed.type === "image" && typed.source?.type === "base64" && typeof typed.source.data === "string") {
      images.push(image(typed.source.data, typed.source.media_type));
    } else if (typed.type === "image" && typeof typed.data === "string") {
      images.push(image(typed.data, typed.mimeType));
    } else if (typed.type === "inputImage" && typeof typed.imageUrl === "string") {
      const match = /^data:([^;,]+);base64,(.*)$/s.exec(typed.imageUrl);
      if (match) images.push(image(match[2] ?? "", match[1]));
    }
  }
  return images;
}

function image(data: string, mediaType: unknown): ImageInput {
  return typeof mediaType === "string" ? { data, media_type: mediaType } : { data };
}

/** Images named after the file they come from, when known. */
function named(images: ImageInput[], file: string | undefined): ImageInput[] {
  return file ? images.map((image) => ({ ...image, name: file })) : images;
}

/** `images` for a tool result event, left out when there are none. */
export function withImages(images: ImageInput[]): { images?: ImageInput[] } {
  return images.length > 0 ? { images } : {};
}

/** Renders a tool result's content as text: strings as is, text blocks joined, the rest as JSON. */
export function toolOutput(content: unknown): string {
  let text: string;
  if (content === undefined || content === null) text = "";
  else if (typeof content === "string") text = content;
  else if (Array.isArray(content)) text = content.map(blockText).join("\n");
  else text = stringify(content);
  return truncate(text, TOOL_OUTPUT_LIMIT);
}

function blockText(block: unknown): string {
  if (typeof block === "string") return block;
  if (typeof block === "object" && block !== null) {
    const typed = block as { type?: unknown; text?: unknown };
    if (typed.type === "text" && typeof typed.text === "string") return typed.text;
    if (typed.type === "image") return "[image]";
    if (typed.type === "document") return "[document]";
  }
  return stringify(block);
}

function isErrorContent(content: unknown): boolean {
  const type = typeof content === "object" && content !== null ? (content as { type?: unknown }).type : undefined;
  return typeof type === "string" && type.endsWith("_error");
}

export function truncate(text: string, limit: number): string {
  if (text.length <= limit) return text;
  return `${text.slice(0, limit)}\n… [truncated ${text.length - limit} characters]`;
}

function stringify(value: unknown): string {
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

function toJson(value: unknown): JsonValue {
  if (value === undefined) return null;
  try {
    return JSON.parse(JSON.stringify(value)) as JsonValue;
  } catch {
    return String(value);
  }
}

/**
 * Auto mode's reason for a block: a category in brackets, shown as a badge
 * (`[Credential Materialization]` → `Credential materialization`, `[PII Data Handling]` →
 * `PII data handling`), or a sentence. The classifier's full message is cut to the reason it
 * ends with.
 */
function classifierReason(text: string): { category?: string; sentence?: string } {
  let reason = plain(text);
  const stated = /\bReason:\s*(.+)$/s.exec(reason);
  if (stated) reason = stated[1]!.trim();
  const bracketed = /^\[([^\]]+)\]\.?$/.exec(reason);
  if (!bracketed) return { sentence: reason.replace(/\.$/, "") };
  // Title Case to sentence case; acronyms (`PII`) stay.
  const words = bracketed[1]!.trim().replace(/\b[A-Z][a-z]+\b/g, (word) => word.toLowerCase());
  return { category: words.charAt(0).toUpperCase() + words.slice(1) };
}

/** Text without terminal escapes (the classifier's reasons may carry ANSI colors). */
function plain(text: string): string {
  return text.replace(/\u001b\[[0-9;?]*[ -\/]*[@-~]/g, "").trim();
}
