// Maps an ACP agent's `session/update` notifications onto agent-agnostic ChatEvents. Stateful per
// session: it streams a message's chunks under one id and gives the whole text once something else
// comes (a tool call, a thought, the turn's end), and remembers each tool call between its
// `tool_call` and the `tool_call_update` that ends it, for its result and its permission requests
// (which often carry only the call's id).
//
// Tool calls are named as Claude's where they match, so that the app draws them the same: a shell
// command is `Bash`, a file read `Read`, an edit `Edit` (or `Write` for a new file), an MCP tool
// `mcp__<server>__<tool>` (Vibe titles them `mcp_<server>.<tool>`).

import { randomUUID } from "node:crypto";
import type { ChatEvent, JsonValue, TurnOutcome } from "../../protocol.ts";
import { TOOL_OUTPUT_LIMIT, toolOutput, truncate } from "../claude/events.ts";
import type { ContentBlock, PromptResponse, SessionUpdate, StopReason, ToolCallContent, ToolCallFields, ToolKind } from "./protocol.ts";

/** A tool call as the mapper knows it: what was shown, and what a permission request asks for. */
export interface AcpToolCall {
  id: string;
  /** The name the chat shows it under (see the header). */
  name: string;
  input: Record<string, JsonValue>;
  kind: ToolKind | null;
  title: string | null;
  /** The files it reads or writes. */
  paths: string[];
  /** The shell command it runs, for `execute`. */
  command: string | null;
  /** Its result was given. */
  ended: boolean;
}

export class AcpEventMapper {
  private readonly tools = new Map<string, AcpToolCall>();
  private message: { id: string; text: string } | undefined;
  private thought: { id: string; text: string } | undefined;
  /** The context the session's last reading measured, and against which window. */
  private contextTokens: number | null = null;
  private contextSize: number | null = null;
  /** The session's model, for its context readings. */
  model: string | null = null;
  /** Jev's threshold (%), when the chat compacts through Jev. */
  jevThreshold: number | undefined;

  /** The tool call of this id, as announced. */
  tool(id: string): AcpToolCall | undefined {
    return this.tools.get(id);
  }

  map(update: SessionUpdate): ChatEvent[] {
    switch (update.sessionUpdate) {
      case "agent_message_chunk": {
        const text = blockText(update.content);
        if (!text) return [];
        const events = this.endThought();
        this.message ??= { id: randomUUID(), text: "" };
        this.message.text += text;
        events.push({ type: "text_delta", message_id: this.message.id, text });
        return events;
      }
      case "agent_thought_chunk": {
        const text = blockText(update.content);
        if (!text) return [];
        const events = this.endMessage();
        this.thought ??= { id: randomUUID(), text: "" };
        this.thought.text += text;
        events.push({ type: "thinking_delta", message_id: this.thought.id, text });
        return events;
      }
      case "tool_call":
        return [...this.flush(), ...this.call(update)];
      case "tool_call_update":
        return [...this.flush(), ...this.callUpdate(update)];
      case "plan":
        return [...this.flush(), { type: "plan_updated", steps: update.entries.map((entry) => ({ text: entry.content, status: entry.status })) }];
      case "usage_update":
        return this.contextReading(update.used, update.size);
      default:
        return [];
    }
  }

  /**
   * The turn's end, from `session/prompt`'s answer (or its failure): the open text, a result for
   * every call left open, then `turn_ended`.
   */
  endTurn(outcome: { response?: PromptResponse; error?: string; interrupted?: boolean }, durationMs: number | null): ChatEvent[] {
    const events = this.closeOpen("The turn ended before this call did.");
    const usage = outcome.response?.usage;
    const turnOutcome: TurnOutcome = outcome.interrupted || outcome.response?.stopReason === "cancelled" ? "interrupted" : outcome.error ? "error" : "success";
    const stopped = outcome.response ? stopError(outcome.response.stopReason) : null;
    events.push({
      type: "turn_ended",
      outcome: stopped && turnOutcome === "success" ? "error" : turnOutcome,
      duration_ms: durationMs,
      cost_usd: null,
      ...(usage?.totalTokens ? { tokens: usage.totalTokens } : {}),
      context_tokens: this.contextTokens,
      error: outcome.error ?? stopped,
    });
    return events;
  }

  /** The text being streamed given whole, and a result (`output`, an error) for every call left open. */
  closeOpen(output: string): ChatEvent[] {
    const events = this.flush();
    for (const tool of this.tools.values()) {
      if (tool.ended) continue;
      tool.ended = true;
      events.push({ type: "tool_result", tool_use_id: tool.id, output, is_error: true });
    }
    return events;
  }

  /** The text and the thought being streamed, given whole. */
  flush(): ChatEvent[] {
    return [...this.endThought(), ...this.endMessage()];
  }

  private endMessage(): ChatEvent[] {
    const message = this.message;
    this.message = undefined;
    return message?.text ? [{ type: "assistant_text", message_id: message.id, text: message.text, parent_tool_use_id: null }] : [];
  }

  private endThought(): ChatEvent[] {
    const thought = this.thought;
    this.thought = undefined;
    return thought?.text ? [{ type: "thinking", message_id: thought.id, text: thought.text }] : [];
  }

  private call(fields: ToolCallFields): ChatEvent[] {
    const tool = describe(fields);
    this.tools.set(tool.id, tool);
    const events: ChatEvent[] = [{ type: "tool_call", tool_use_id: tool.id, name: tool.name, input: tool.input, parent_tool_use_id: null }];
    if (fields.status === "completed" || fields.status === "failed") events.push(this.result(tool, fields));
    return events;
  }

  private callUpdate(fields: ToolCallFields): ChatEvent[] {
    const known = this.tools.get(fields.toolCallId);
    // An update for a call never announced: announced now, with what the update says.
    if (!known) return this.call(fields);
    if (fields.kind) known.kind = fields.kind;
    if (fields.title) known.title = fields.title;
    if (fields.status !== "completed" && fields.status !== "failed") return [];
    if (known.ended) return [];
    return [this.result(known, fields)];
  }

  private result(tool: AcpToolCall, fields: ToolCallFields): ChatEvent {
    tool.ended = true;
    return {
      type: "tool_result",
      tool_use_id: tool.id,
      output: truncate(resultText({ ...fields, title: fields.title ?? tool.title }), TOOL_OUTPUT_LIMIT),
      is_error: fields.status === "failed",
    };
  }

  private contextReading(used: number, size: number): ChatEvent[] {
    if (!(used > 0) || !(size > 0)) return [];
    // Vibe repeats a reading at each step: given once.
    if (used === this.contextTokens && size === this.contextSize) return [];
    this.contextTokens = used;
    this.contextSize = size;
    return [
      {
        type: "context_usage",
        usage: {
          model: this.model,
          used_tokens: used,
          max_tokens: size,
          percent: Math.round((used / size) * 100),
          over_limit: used > size,
          categories: [],
          compaction: this.jevThreshold === undefined ? null : { by: "jev", threshold_percent: this.jevThreshold },
        },
      },
    ];
  }
}

/** A call's name, input, paths and command, from what the agent says of it. */
export function describe(fields: ToolCallFields): AcpToolCall {
  const kind = fields.kind ?? null;
  const title = fields.title ?? null;
  const raw = record(fields.rawInput);
  const paths = [...(fields.locations ?? []).map((location) => location.path), ...diffs(fields.content).map((diff) => diff.path)];
  const path = paths[0] ?? stringField(raw, "path", "file_path", "filePath");
  const base = { id: fields.toolCallId, kind, title, paths: [...new Set(paths)], ended: false };
  const mcp = title ? /^mcp_([\w-]+)\.([\w-]+)$/.exec(title) : null;
  if (mcp) return { ...base, name: `mcp__${mcp[1]}__${mcp[2]}`, input: raw, command: null };
  // Vibe looks up its deferred tools (MCP servers') before calling them, as Claude's ToolSearch.
  if (title && /^search(ing)? for relevant tools/i.test(title)) return { ...base, name: "ToolSearch", input: raw, command: null };
  switch (kind) {
    case "execute": {
      const command = stringField(raw, "command", "cmd") ?? title?.replace(/^bash:\s*/, "") ?? "";
      return { ...base, name: "Bash", input: { command, ...(raw.cwd ? { cwd: raw.cwd } : {}) }, command };
    }
    case "edit":
    case "delete":
    case "move": {
      const changes = diffs(fields.content);
      const [only] = changes;
      if (changes.length === 1 && only) {
        const input: Record<string, JsonValue> =
          only.oldText === null || only.oldText === undefined
            ? { file_path: only.path, content: only.newText }
            : { file_path: only.path, old_string: only.oldText, new_string: only.newText };
        return { ...base, name: input.content === undefined ? "Edit" : "Write", input, command: null };
      }
      if (changes.length > 1) {
        return {
          ...base,
          name: "apply_patch",
          input: { changes: changes.map((change) => ({ path: change.path, kind: change.oldText ? "update" : "add", diff: change.newText })) },
          command: null,
        };
      }
      return { ...base, name: kind === "delete" ? "Delete" : kind === "move" ? "Move" : "Edit", input: { ...raw, ...(path ? { file_path: path } : {}) }, command: null };
    }
    case "read":
      return { ...base, name: "Read", input: { ...raw, ...(path ? { file_path: path } : {}) }, command: null };
    case "search":
      return { ...base, name: "Grep", input: { ...raw, ...(title ? { description: title } : {}) }, command: null };
    case "fetch":
      return { ...base, name: "WebFetch", input: raw, command: null };
    default:
      return { ...base, name: title ?? kind ?? "tool", input: raw, command: null };
  }
}

function resultText(fields: ToolCallFields): string {
  // Vibe adds the call's title as a last text: not part of the result.
  const texts = (fields.content ?? []).flatMap((item) => (item.type === "content" ? [blockText(item.content)] : [])).filter((text) => text !== fields.title);
  if (texts.some(Boolean)) return texts.join("\n");
  const raw = fields.rawOutput;
  if (raw === undefined || raw === null) return "";
  if (typeof raw === "string") return raw;
  const fieldsOf = record(raw);
  // Vibe's shell results: stdout and stderr.
  const stdout = stringField(fieldsOf, "stdout", "output");
  const stderr = stringField(fieldsOf, "stderr");
  if (stdout !== undefined || stderr !== undefined) return [stdout, stderr].filter(Boolean).join("\n");
  const error = record(fieldsOf.error);
  if (typeof error.message === "string") return error.message;
  return toolOutput(raw);
}

function diffs(content: ToolCallContent[] | null | undefined): Extract<ToolCallContent, { type: "diff" }>[] {
  return (content ?? []).filter((item): item is Extract<ToolCallContent, { type: "diff" }> => item.type === "diff");
}

export function blockText(block: ContentBlock | undefined): string {
  return block?.type === "text" ? block.text : "";
}

function stopError(reason: StopReason): string | null {
  switch (reason) {
    case "max_tokens":
      return "The model reached its output limit.";
    case "max_turn_requests":
      return "The agent reached its limit of steps for one turn.";
    case "refusal":
      return "The model refused to go on.";
    default:
      return null;
  }
}

function record(value: unknown): Record<string, JsonValue> {
  return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Record<string, JsonValue>) : {};
}

function stringField(fields: Record<string, JsonValue>, ...names: string[]): string | undefined {
  for (const name of names) {
    const value = fields[name];
    if (typeof value === "string") return value;
  }
  return undefined;
}
