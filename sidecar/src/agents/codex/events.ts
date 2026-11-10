// Maps Codex app-server notifications onto agent-agnostic ChatEvents. Stateful per session: it
// remembers tool items between `item/started` and `item/completed`, buffers command output, and
// keeps the last token usage for `turn_ended`.
//
// Codex 0.162 runs the model's commands through a JavaScript `exec` tool (code mode): the script
// calls `tools.exec_command(…)`, and the app-server still reports each such command as its own
// `commandExecution` item. A command the sandbox refuses before it runs is the exception: Codex
// tells the model and sends no item at all (only its PreToolUse hook's `hook/completed`), so the
// chat shows the retry with escalated permissions, not the refused attempt.

import { splitCommands } from "../../guard/worktree-guard.ts";
import { DELEGATE_TOOL } from "../delegate-tool.ts";
import type { ChatEvent, ImageInput, JsonValue, TurnOutcome } from "../../protocol.ts";
import { notice, TOOL_OUTPUT_LIMIT, toolImages, toolOutput, truncate, withImages } from "../claude/events.ts";
import type { FileUpdateChange, ServerNotification, ThreadItem, TokenUsageBreakdown, Turn } from "./protocol.ts";
import { estimateTokenCost } from "./pricing.ts";

/** The tool names Codex items are reported under (the same as Claude's where they match). */
export const CODEX_TOOLS = {
  command: "Bash",
  fileChange: "apply_patch",
  webSearch: "WebSearch",
  imageView: "ViewImage",
  imageGeneration: "ImageGeneration",
  sleep: "Sleep",
} as const;

const MAX_OUTLIVED = 64;

interface OpenTool {
  name: string;
  output: string;
}

/** Notifications that carry nothing the chat shows. */
const IGNORED = new Set<string>([
  "thread/started",
  "thread/status/changed",
  "thread/readState/changed",
  "thread/name/updated",
  "thread/settings/updated",
  "thread/goal/updated",
  "thread/goal/cleared",
  "thread/queue/changed",
  "thread/compacted",
  "turn/diff/updated",
  "turn/moderationMetadata",
  "hook/started",
  "item/plan/delta",
  "item/reasoning/summaryPartAdded",
  "item/fileChange/outputDelta",
  "item/fileChange/patchUpdated",
  "item/commandExecution/terminalInteraction",
  "item/mcpToolCall/progress",
  "item/autoApprovalReview/started",
  "item/autoApprovalReview/completed",
  "serverRequest/resolved",
  "rawResponseItem/completed",
  "rawResponse/completed",
  "mcpServer/startupStatus/updated",
  "mcpServer/oauthLogin/completed",
  "mcpServer/event/stream/notification",
  "account/updated",
  "account/rateLimits/updated",
  "account/gatewayOAuth/changed",
  "app/list/updated",
  "skills/changed",
  "remoteControl/status/changed",
  "model/verification",
  "model/safetyBuffering/updated",
  "modelProvider/authRecoveryStarted",
  "modelProvider/authRecoveryCompleted",
  "fs/changed",
  "process/outputDelta",
  "process/exited",
  "command/exec/outputDelta",
]);

const IGNORED_ITEMS = new Set<string>(["userMessage", "hookPrompt", "plan", "subAgentActivity", "functionCallOutput"]);

/**
 * How full the context is after a turn: the last response's tokens against the model's window.
 * Codex gives no breakdown, and compacts on OpenAI's side at a point it does not say; with Jev on,
 * the chat compacts at its threshold.
 */
function contextReading(
  tokens: number,
  window: number | null,
  model: string | null,
  jevThreshold?: number,
  estimated = false,
): ChatEvent | null {
  if (tokens <= 0 || !window || window <= 0) return null;
  return {
    type: "context_usage",
    usage: {
      model,
      used_tokens: tokens,
      max_tokens: window,
      percent: Math.round((tokens / window) * 100),
      over_limit: tokens > window,
      categories: [],
      compaction: jevThreshold === undefined ? null : { by: "jev", threshold_percent: jevThreshold },
      ...(estimated ? { estimated } : {}),
    },
  };
}

export class CodexEventMapper {
  private readonly tools = new Map<string, OpenTool>();
  /**
   * Tool items a turn ended on, already given an interim result. Codex keeps a command running
   * past its turn (a background process) and completes its item when it exits, later: that result
   * replaces the interim one, under the call already shown.
   */
  private readonly outlived = new Map<string, OpenTool>();
  private readonly fileChanges = new Map<string, FileUpdateChange[]>();
  /** Reasoning item id → index of the summary part being streamed. */
  private readonly summaryParts = new Map<string, number>();
  private lastUsage: TokenUsageBreakdown | undefined;
  private previousUsage: { threadId: string; totalTokens: number } | undefined;
  private turnId: string | null = null;
  private estimatedCost: number | null = null;
  private estimateComplete = true;
  /** Tokens the turn's model calls used so far, input (cached included) and output. */
  private turnTokens = 0;
  fast = false;
  /** The model's context window, as Codex last reported it. */
  private contextWindow: number | null = null;
  /** The tokens of the last reading given, measured or estimated. */
  private readTokens: number | undefined;
  /** The last reading given is an estimate: Codex's next token count replaces it at once. */
  private estimated = false;
  /** The thread's model, for its context readings. */
  model: string | null = null;
  /** Jev's threshold (%), when the chat compacts through Jev. */
  jevThreshold: number | undefined;
  /** The thread runs a local model, through Ollama (story 52). */
  local = false;
  private readonly reported = new Set<string>();

  map(message: ServerNotification): ChatEvent[] {
    switch (message.method) {
      case "turn/started":
        this.turnId = message.params.turn.id;
        this.estimatedCost = null;
        this.estimateComplete = true;
        this.turnTokens = 0;
        return [{ type: "turn_started" }];
      case "turn/completed":
        return this.turnEnded(message.params.threadId, message.params.turn);
      case "item/started":
        return this.itemStarted(message.params.item);
      case "item/completed":
        return this.itemCompleted(message.params.item);
      case "item/agentMessage/delta":
        return message.params.delta ? [{ type: "text_delta", message_id: message.params.itemId, text: message.params.delta }] : [];
      case "item/reasoning/summaryTextDelta": {
        const { itemId, delta, summaryIndex } = message.params;
        const previous = this.summaryParts.get(itemId);
        this.summaryParts.set(itemId, summaryIndex);
        // Summary parts are joined by a blank line in the final `thinking`.
        const text = previous !== undefined && previous !== summaryIndex ? `\n\n${delta}` : delta;
        return text ? [{ type: "thinking_delta", message_id: itemId, text }] : [];
      }
      case "item/reasoning/textDelta":
        return message.params.delta ? [{ type: "thinking_delta", message_id: message.params.itemId, text: message.params.delta }] : [];
      case "item/commandExecution/outputDelta": {
        const tool = this.tools.get(message.params.itemId) ?? this.outlived.get(message.params.itemId);
        if (tool && tool.output.length < TOOL_OUTPUT_LIMIT * 2) tool.output += message.params.delta;
        return [];
      }
      case "turn/plan/updated":
        // The plan, whole: the daemon counts its steps done into the chat's progress.
        return [
          {
            type: "plan_updated",
            steps: message.params.plan.map((step) => ({
              text: step.step,
              status: step.status === "inProgress" ? "in_progress" : step.status,
            })),
          },
        ];
      case "thread/tokenUsage/updated": {
        const { threadId, turnId, tokenUsage: { total, last } } = message.params;
        const duplicate = this.previousUsage?.threadId === threadId && this.previousUsage.totalTokens === total.totalTokens;
        this.previousUsage = { threadId, totalTokens: total.totalTokens };
        if (this.turnId === turnId && !duplicate) {
          // Each update reports the last model call. Totals include resumed/forked history;
          // pricing each call also applies long-context rates to its own input, not the sum.
          if (Number.isFinite(last.totalTokens) && last.totalTokens > 0) this.turnTokens += last.totalTokens;
          const cost = this.local ? null : estimateTokenCost(this.model, last, this.fast);
          if (cost !== null) this.estimatedCost = (this.estimatedCost ?? 0) + cost;
          else if (!this.local) this.estimateComplete = false;
        }
        this.lastUsage = message.params.tokenUsage.last;
        this.contextWindow = message.params.tokenUsage.modelContextWindow ?? this.contextWindow;
        if (!this.estimated) return [];
        // The first response in the thread a compaction left measures it: no wait for the turn's end.
        this.estimated = false;
        return this.reading(this.lastUsage.totalTokens);
      }
      case "error":
        // A final error also fails the turn, whose `turn_ended` carries it.
        return message.params.willRetry ? [notice("warning", `Codex error, retrying: ${errorText(message.params.error.message)}`)] : [];
      case "warning":
        // Codex has no metadata for any local model and says so at each thread: nothing to act on.
        if (this.local && /^Model metadata for `[^`]*` not found/.test(message.params.message)) return [];
        return [notice("warning", message.params.message)];
      case "configWarning":
        return [notice("warning", [message.params.summary, message.params.details].filter(Boolean).join(": "))];
      case "deprecationNotice":
        return [notice("info", [message.params.summary, message.params.details].filter(Boolean).join(": "))];
      case "guardianWarning":
        // Auto review's verdicts (auto mode): a denial or a failed review shows, approvals do not.
        if (/^Automatic approval review approved\b/i.test(message.params.message)) return [];
        return [notice("warning", `Codex: ${message.params.message}`)];
      case "model/rerouted":
        this.model = message.params.toModel;
        return [notice("info", `Codex switched from ${message.params.fromModel} to ${message.params.toModel} (${message.params.reason})`)];
      case "hook/completed": {
        const run = message.params.run;
        if (run.eventName !== "preToolUse" || run.status !== "blocked") return [];
        const feedback = run.entries.map((entry) => entry.text).join("\n");
        return [notice("warning", `A command was blocked: ${feedback || "by a PreToolUse hook"}`)];
      }
      default:
        if (IGNORED.has(message.method)) return [];
        return this.unexpected(message.method);
    }
  }

  /** The changes of a file change item, for its approval request. */
  changesOf(itemId: string): FileUpdateChange[] {
    return this.fileChanges.get(itemId) ?? [];
  }

  private itemStarted(item: ThreadItem): ChatEvent[] {
    // A web search starts with an empty query: its call waits for the completed item, which has it.
    if (item.type === "webSearch") return [];
    const call = toolCall(item);
    if (!call) {
      if (item.type === "contextCompaction") return [notice("info", "Conversation compacted")];
      return [];
    }
    if (item.type === "fileChange") this.fileChanges.set(item.id, item.changes);
    this.tools.set(item.id, { name: call.name, output: "" });
    return [call];
  }

  private itemCompleted(item: ThreadItem): ChatEvent[] {
    switch (item.type) {
      case "agentMessage":
        return item.text ? [{ type: "assistant_text", message_id: item.id, text: item.text, parent_tool_use_id: null }] : [];
      case "reasoning": {
        this.summaryParts.delete(item.id);
        const text = [...item.summary, ...item.content].filter(Boolean).join("\n\n");
        return text ? [{ type: "thinking", message_id: item.id, text }] : [];
      }
      default:
        break;
    }
    const open = this.tools.get(item.id) ?? this.outlived.get(item.id);
    const result = toolResult(item, open?.output ?? "");
    if (!result) {
      if (!IGNORED_ITEMS.has(item.type) && item.type !== "contextCompaction") return this.unexpected(`item/${item.type}`);
      return [];
    }
    const events: ChatEvent[] = [];
    // A tool item seen only at completion (e.g. resumed mid-turn, a web search) still gets its call.
    if (!open) {
      const call = toolCall(item);
      if (call) events.push(call);
    }
    this.tools.delete(item.id);
    this.outlived.delete(item.id);
    this.fileChanges.delete(item.id);
    events.push(result);
    return events;
  }

  /**
   * `turn_ended`, after giving the tool calls the turn left open an interim result: a command
   * still running after a turn that completed runs on in the background (its item completes
   * later), a delegated chat after a turn not interrupted works on (its answer comes as a
   * message); otherwise the turn was interrupted or failed.
   */
  private turnEnded(threadId: string, turn: Turn): ChatEvent[] {
    const events: ChatEvent[] = [];
    for (const [id, tool] of this.tools) {
      const output = tool.output ? `${truncate(tool.output, TOOL_OUTPUT_LIMIT)}\n` : "";
      const background = turn.status === "completed" && tool.name === CODEX_TOOLS.command;
      // Its delegated chat works on but after an interrupt: its answer comes as a message.
      const delegated = turn.status !== "interrupted" && tool.name === DELEGATE_TOOL;
      const state = background
        ? "still running in the background"
        : delegated
          ? "the delegated chat works on: its answer will come as a new message"
          : turn.status === "interrupted"
            ? "interrupted"
            : "did not finish";
      events.push({ type: "tool_result", tool_use_id: id, output: `${output}[${state}]`, is_error: !background && !delegated });
      this.outlived.set(id, tool);
    }
    // Codex may never complete some (a process killed with its session): keep the latest only.
    for (const id of this.outlived.keys()) {
      if (this.outlived.size <= MAX_OUTLIVED) break;
      this.outlived.delete(id);
    }
    this.tools.clear();
    this.fileChanges.clear();
    this.summaryParts.clear();
    const outcome: TurnOutcome = turn.status === "interrupted" ? "interrupted" : turn.status === "failed" ? "error" : "success";
    const usage = this.lastUsage;
    this.lastUsage = undefined;
    events.push(...this.reading(usage?.totalTokens ?? 0));
    events.push({
      type: "turn_ended",
      outcome,
      duration_ms: turn.durationMs ?? null,
      cost_usd: null,
      ...(this.estimateComplete && this.estimatedCost !== null ? { estimated_cost_usd: this.estimatedCost } : {}),
      ...(this.turnTokens > 0 ? { tokens: this.turnTokens } : {}),
      context_tokens: usage && usage.totalTokens > 0 ? usage.totalTokens : null,
      error: outcome === "error" ? errorText(turn.error?.message ?? "Codex failed the turn") : null,
      // A fork from a finished turn copies the thread through it.
      ...(outcome === "success" && threadId && turn.id ? { fork_point: { session_id: threadId, up_to: turn.id } } : {}),
    });
    this.turnId = null;
    this.estimatedCost = null;
    this.turnTokens = 0;
    return events;
  }

  /**
   * The reading after a compaction removed about `removedTokens` from the context: the last
   * reading less them, marked estimated until Codex's next token count. None without a reading
   * to start from (a session resumed since its last turn).
   */
  estimateAfterCompaction(removedTokens: number): ChatEvent[] {
    this.estimated = true;
    if (this.readTokens === undefined) return [];
    const tokens = Math.max(1, this.readTokens - Math.round(removedTokens));
    const reading = contextReading(tokens, this.contextWindow, this.model, this.jevThreshold, true);
    if (!reading) return [];
    this.readTokens = tokens;
    return [reading];
  }

  private reading(tokens: number): ChatEvent[] {
    const reading = contextReading(tokens, this.contextWindow, this.model, this.jevThreshold);
    if (!reading) return [];
    this.readTokens = tokens;
    return [reading];
  }

  private unexpected(kind: string): ChatEvent[] {
    if (this.reported.has(kind)) return [];
    this.reported.add(kind);
    return [notice("info", `Chauffeur does not display Codex "${kind}" events yet`)];
  }
}

function toolCall(item: ThreadItem): Extract<ChatEvent, { type: "tool_call" }> | undefined {
  const call = (name: string, input: JsonValue): Extract<ChatEvent, { type: "tool_call" }> => ({
    type: "tool_call",
    tool_use_id: item.id,
    name,
    input,
    parent_tool_use_id: null,
  });
  switch (item.type) {
    case "commandExecution":
      return call(CODEX_TOOLS.command, { command: shellScript(item.command), cwd: item.cwd });
    case "fileChange":
      return call(CODEX_TOOLS.fileChange, { changes: item.changes.map(changeJson) });
    case "mcpToolCall":
      return call(`mcp__${item.server}__${item.tool}`, item.arguments as JsonValue);
    case "dynamicToolCall":
      // Chauffeur's own (repo tools), named as Claude's MCP server names them.
      return call(item.namespace ? `mcp__${item.namespace}__${item.tool}` : item.tool, item.arguments as JsonValue);
    case "collabAgentToolCall":
      return call(item.tool, { prompt: item.prompt, model: item.model, receiver_thread_ids: item.receiverThreadIds });
    case "webSearch":
      return call(CODEX_TOOLS.webSearch, webSearchInput(item));
    case "imageView":
      return call(CODEX_TOOLS.imageView, { path: item.path });
    case "imageGeneration":
      return call(CODEX_TOOLS.imageGeneration, item.revisedPrompt ? { prompt: item.revisedPrompt, description: item.revisedPrompt } : {});
    case "sleep":
      return call(CODEX_TOOLS.sleep, { duration_ms: item.durationMs, description: `Wait ${formatSeconds(item.durationMs)}` });
    default:
      return undefined;
  }
}

/** What a web search looked for: its query, or the page it opened or searched in. */
function webSearchInput(item: Extract<ThreadItem, { type: "webSearch" }>): JsonValue {
  const action = item.action;
  switch (action?.type) {
    case "search": {
      const queries = action.queries?.filter(Boolean) ?? [];
      return { query: action.query || queries.join(" | ") || item.query, ...(queries.length > 1 ? { queries } : {}) };
    }
    case "openPage":
      return action.url ? { url: action.url } : { query: item.query };
    case "findInPage":
      return { ...(action.url ? { url: action.url } : {}), ...(action.pattern ? { pattern: action.pattern } : {}), query: item.query };
    default:
      return { query: item.query };
  }
}

function formatSeconds(ms: number): string {
  const seconds = ms / 1000;
  return `${Number.isInteger(seconds) ? seconds : seconds.toFixed(1)} s`;
}

function toolResult(item: ThreadItem, streamed: string): Extract<ChatEvent, { type: "tool_result" }> | undefined {
  const result = (
    output: string,
    isError: boolean,
    images: ImageInput[] = [],
  ): Extract<ChatEvent, { type: "tool_result" }> => ({
    type: "tool_result",
    tool_use_id: item.id,
    output: truncate(output, TOOL_OUTPUT_LIMIT),
    is_error: isError,
    ...withImages(images),
  });
  switch (item.type) {
    case "commandExecution": {
      const output = item.aggregatedOutput ?? streamed;
      if (item.status === "declined") return result(output || "Declined by the user", true);
      const exitCode = item.exitCode ?? 0;
      // Claude Code's Bash reads the same way: "Exit code 2" over the output.
      if (exitCode !== 0) return result(`Exit code ${exitCode}\n${output}`, true);
      return result(output, item.status === "failed");
    }
    case "fileChange": {
      const summary = item.changes.map((change) => `${change.kind.type} ${change.path}`).join("\n");
      if (item.status === "declined") return result(`Declined by the user\n${summary}`, true);
      return result(summary, item.status !== "completed");
    }
    case "mcpToolCall":
      if (item.error) return result(item.error.message, true);
      return result(
        item.result ? toolOutput(item.result.content) : "",
        item.status === "failed",
        toolImages(item.result?.content),
      );
    case "dynamicToolCall":
      return result(
        // Chauffeur's tools answer with `inputText` items: their text, as Claude's text blocks.
        toolOutput((item.contentItems ?? []).map((content) => (content.type === "inputText" ? content.text : content))),
        item.success === false || item.status === "failed",
        toolImages(item.contentItems),
      );
    case "collabAgentToolCall":
      return result(JSON.stringify(item.agentsStates), item.status === "failed");
    case "webSearch":
      return result(item.results ? JSON.stringify(item.results) : "", false);
    case "imageView":
      // The daemon copies the file Codex looked at.
      return result("", false, [{ path: item.path }]);
    case "imageGeneration": {
      if (item.failure) return result(`Image generation failed: ${item.failure.type}`, true);
      if (item.status === "failed") return result("Image generation failed", true);
      // The daemon copies the image Codex saved; `result` is the image itself, as base64.
      return result(item.savedPath ?? "", false, item.savedPath ? [{ path: item.savedPath }] : []);
    }
    case "sleep":
      return result("", false);
    default:
      return undefined;
  }
}

/** The script of `/bin/zsh -lc '<script>'`, the way Codex wraps every command; else the command. */
export function shellScript(command: string): string {
  const [words] = splitCommands(command);
  if (!words || words.length !== 3) return command;
  const [shell, flag, script] = words as [string, string, string];
  if (/(^|\/)(ba|z|da|k)?sh$/.test(shell) && /^-[a-z]*c$/.test(flag)) return script;
  return command;
}

function changeJson(change: FileUpdateChange): JsonValue {
  const kind: Record<string, JsonValue> = { type: change.kind.type };
  if (change.kind.type === "update" && change.kind.move_path) kind.move_path = change.kind.move_path;
  return { path: change.path, kind, diff: change.diff };
}

/** Codex wraps API errors as JSON in the message: keep the human part. */
export function errorText(message: string): string {
  try {
    const parsed = JSON.parse(message) as { error?: { message?: unknown }; message?: unknown };
    const inner = parsed.error?.message ?? parsed.message;
    if (typeof inner === "string" && inner) return inner;
  } catch {
    // Not JSON.
  }
  return message;
}
