// A subagent's own view: what the chat saw of it live (its tool calls in the chat, its text and
// thinking kept aside by the reducer) merged with its transcript (`chat.subagent_messages`).
// Pure; unit-tested in subagent.test.ts.

import type { ChatEvent, EventEnvelope, JsonValue, TaskStatus } from "@chauffeur/protocol";
import { buildChatView, type ChatItem, type ChatView, type ToolItem } from "./chat";

/** A subagent to show: by the tool call that started it, else by its agent id (its task's id). */
export type SubagentTarget = { toolUseId: string | null; agentId: string | null };

export type SubagentStatus = "running" | TaskStatus;

/** The header and the framing of a subagent's view. */
export type SubagentSummary = {
  /** `general-purpose`, `Explore`, a custom agent's name. */
  type: string | null;
  description: string | null;
  /** What the main agent asked of it: its Agent call's `prompt`. */
  prompt: string | null;
  status: SubagentStatus;
  background: boolean;
  /** Its task's id, which is its agent id: what `chat.stop_task` stops. */
  taskId: string | null;
  /** How long it ran, or has run so far. */
  durationMs: number | null;
  toolUses: number | null;
  /** Its final report: the Agent call's result, or its last message for a background one. */
  report: string | null;
  failed: boolean;
};

/** What Claude Code appends to a subagent's result for the main agent, and its launch message. */
export type AgentResult = {
  /** The report itself, without the trailer; null for a launch message (a background subagent). */
  report: string | null;
  agentId: string | null;
  toolUses: number | null;
  durationMs: number | null;
};

const LAUNCHED = /^(Async agent launched|Fork started)/;
const POINTER = /report was delivered to you as a message from/;

/**
 * Reads an Agent call's result: `<report>\nagentId: a1 (use SendMessage …)\n<usage>…
 * tool_uses: 3\nduration_ms: 1200</usage>` for a foreground subagent, `Async agent launched
 * successfully. … agentId: a1 …` for a background one.
 */
export function parseAgentResult(output: string): AgentResult {
  const id = /\bagentId: ([A-Za-z0-9_-]+)/.exec(output)?.[1] ?? null;
  const number = (name: string) => {
    const match = new RegExp(`\\b${name}: (\\d+)`).exec(output);
    return match ? Number(match[1]) : null;
  };
  if (LAUNCHED.test(output.trimStart())) return { report: null, agentId: id, toolUses: null, durationMs: null };
  // The report went to the main agent as a message (SubagentHandback): the result only says so.
  if (POINTER.test(output)) {
    const from = /as a message from "([A-Za-z0-9_-]+)"/.exec(output)?.[1] ?? id;
    return { report: null, agentId: from, toolUses: number("tool_uses"), durationMs: number("duration_ms") };
  }
  let report = output;
  const trailer = report.search(/\n?agentId: [A-Za-z0-9_-]+ \(/);
  if (trailer >= 0) report = report.slice(0, trailer);
  report = report.replace(/<usage>[\s\S]*?<\/usage>/g, "").replace(/^\[Subagent hand-back\]\s*/, "").trim();
  return { report: report || null, agentId: id, toolUses: number("tool_uses"), durationMs: number("duration_ms") };
}

function field(input: JsonValue | undefined, key: string): string | null {
  if (!input || typeof input !== "object" || Array.isArray(input)) return null;
  const value = input[key];
  return typeof value === "string" && value.length > 0 ? value : null;
}

/** The Agent (or Task) call that started a subagent, among the chat's items. */
export function agentCall(view: ChatView, toolUseId: string | null): ToolItem | null {
  if (!toolUseId) return null;
  for (let i = view.items.length - 1; i >= 0; i--) {
    const item = view.items[i];
    if (item.kind === "tool" && item.toolUseId === toolUseId) return item;
  }
  return null;
}

/** The tool call behind a target opened by its agent id (from the tasks bar). */
export function targetToolUse(view: ChatView, target: SubagentTarget): string | null {
  if (target.toolUseId) return target.toolUseId;
  if (!target.agentId) return null;
  const running = view.tasks.find((task) => task.taskId === target.agentId)?.toolUseId;
  if (running) return running;
  for (const [toolUseId, log] of Object.entries(view.subagents)) if (log.taskId === target.agentId) return toolUseId;
  return null;
}

/** Whether a tool call starts a subagent (Claude's Agent tool, called Task before). */
export function isAgentCall(name: string): boolean {
  return name === "Agent" || name === "Task";
}

/**
 * What the chat saw of the subagent live: its own tool calls (in the chat, under its call) and
 * its text and thinking (kept aside), in the order they came. A nested subagent's steps are its
 * own: only its Agent call shows here.
 */
export function liveItems(view: ChatView, toolUseId: string | null): ChatItem[] {
  if (!toolUseId) return [];
  const log = view.subagents[toolUseId];
  const texts = log?.items ?? [];
  const out: ChatItem[] = [];
  let next = 0;
  view.items.forEach((item, index) => {
    while (next < texts.length && texts[next]!.anchor <= index) out.push(texts[next++]!);
    if (item.kind === "tool" && item.parentToolUseId === toolUseId) out.push({ ...item, depth: 0 });
  });
  while (next < texts.length) out.push(texts[next++]!);
  return out;
}

/** A transcript's events as items, keyed apart from the chat's. */
export function transcriptItems(events: ChatEvent[]): ChatItem[] {
  const envelopes: EventEnvelope[] = events.map((event) => ({
    seq: null,
    ts: "",
    event: { scope: "chat", workspace_id: "", chat_id: "", event },
  }));
  return buildChatView(envelopes).items.map((item) => ({ ...item, key: `t-${item.key}`, depth: 0 }));
}

/**
 * The transcript, then what came live since it was read. A tool call already in the transcript
 * takes the live result it may lack; text and thinking match by message id.
 */
export function mergeConversation(transcript: ChatItem[] | null, live: ChatItem[]): ChatItem[] {
  if (!transcript) return live;
  const merged = transcript.slice();
  const tools = new Map<string, number>();
  const texts = new Set<string>();
  merged.forEach((item, index) => {
    if (item.kind === "tool") tools.set(item.toolUseId, index);
    if (item.kind === "assistant" || item.kind === "thinking") texts.add(`${item.kind}:${item.messageId}`);
  });
  for (const item of live) {
    if (item.kind === "tool") {
      const at = tools.get(item.toolUseId);
      if (at === undefined) merged.push(item);
      else if ((merged[at] as ToolItem).result == null && item.result) merged[at] = { ...(merged[at] as ToolItem), result: item.result };
      continue;
    }
    if ((item.kind === "assistant" || item.kind === "thinking") && texts.has(`${item.kind}:${item.messageId}`)) continue;
    merged.push(item);
  }
  return merged;
}

/** The header and framing of the subagent's view; `now` dates a running one. */
export function subagentSummary(view: ChatView, target: SubagentTarget, conversation: ChatItem[], now: number): SubagentSummary {
  const toolUseId = targetToolUse(view, target);
  const call = agentCall(view, toolUseId);
  const log = toolUseId ? view.subagents[toolUseId] : undefined;
  const taskId = log?.taskId ?? target.agentId ?? null;
  const task = view.tasks.find((t) => (taskId && t.taskId === taskId) || (toolUseId && t.toolUseId === toolUseId));
  const result = call?.result ? parseAgentResult(call.result.output) : null;

  let status: SubagentStatus;
  if (task) status = "running";
  else if (log?.status) status = log.status;
  else if (call?.result) status = call.result.isError ? "failed" : "completed";
  else status = view.working ? "running" : "stopped";

  const started = Date.parse(task?.startedAt ?? log?.startedAt ?? call?.ts ?? "");
  const ended = Date.parse(log?.endedAt ?? "");
  const durationMs =
    status === "running"
      ? Number.isNaN(started)
        ? null
        : Math.max(0, now - started)
      : (result?.durationMs ?? log?.durationMs ?? (Number.isNaN(started) || Number.isNaN(ended) ? null : Math.max(0, ended - started)));
  const counted = conversation.filter((item) => item.kind === "tool").length;
  const toolUses = (status === "running" ? (task?.toolUses ?? log?.toolUses) : (result?.toolUses ?? log?.toolUses)) ?? (counted || null);

  // The report a subagent handed back through its own call, else its result; a background
  // subagent's result is its launch message: its report is then its last message.
  let report = handedBack(conversation) ?? result?.report ?? null;
  if (!report && status !== "running") {
    const last = conversation.at(-1);
    if (last?.kind === "assistant") report = last.text;
  }
  return {
    type: field(call?.input, "subagent_type") ?? task?.subagentType ?? null,
    description: field(call?.input, "description") ?? task?.description ?? null,
    prompt: field(call?.input, "prompt"),
    status,
    background: task?.background ?? log?.background ?? (call?.input && typeof call.input === "object" && !Array.isArray(call.input) ? call.input.run_in_background === true : false),
    taskId: task?.taskId ?? taskId,
    durationMs,
    toolUses,
    report,
    failed: call?.result?.isError === true,
  };
}

/**
 * The report a subagent handed back with a `SubagentHandback` call (its `message`): Claude Code
 * then delivers it to the main agent as a message, and the Agent call's result only points there.
 */
function handedBack(conversation: ChatItem[]): string | null {
  for (let i = conversation.length - 1; i >= 0; i--) {
    const item = conversation[i]!;
    if (item.kind === "tool" && item.name === "SubagentHandback") return field(item.input, "message");
  }
  return null;
}

/** The conversation without its last message when the report repeats it. */
export function withoutReport(conversation: ChatItem[], report: string | null): ChatItem[] {
  const last = conversation.at(-1);
  if (report && last?.kind === "assistant" && last.text.trim() === report.trim()) return conversation.slice(0, -1);
  return conversation;
}

/** What the panel can do: open one (from the chat or the tasks bar), go into a nested one, back, close. */
export type PanelAction =
  | { type: "open"; target: SubagentTarget }
  | { type: "push"; target: SubagentTarget }
  | { type: "back"; index: number }
  | { type: "close" };

/** The subagents open in the panel: the one shown last, the ones it was opened from before it. */
export function panelStack(stack: SubagentTarget[], action: PanelAction): SubagentTarget[] {
  switch (action.type) {
    case "open":
      return [action.target];
    case "push": {
      // Opening the one already shown changes nothing.
      const top = stack.at(-1);
      if (top && top.toolUseId === action.target.toolUseId && top.agentId === action.target.agentId) return stack;
      return [...stack, action.target];
    }
    case "back":
      return stack.slice(0, action.index + 1);
    case "close":
      return [];
  }
}
