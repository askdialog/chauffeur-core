// Event stream of one chat → what the chat view shows. Pure: `reduceChat` never mutates its input,
// so React can compare by reference. Unit-tested in chat.test.ts.

import type {
  AgentKind,
  Attachment,
  ChatInfo,
  QueuedMessage,
  ChatEvent,
  ChatStatus,
  CommentAuthor,
  DelegatedAnswer,
  ContextUsage,
  EventEnvelope,
  JsonValue,
  NoticeLevel,
  Question,
  RequestAgent,
  TaskKind,
  TaskStatus,
  TurnOutcome,
} from "@chauffeur/protocol";
import { withoutPrivate } from "./files";
import { AGENT_NAMES } from "./models";

type Base = {
  /** Stable key for the list. */
  key: string;
  ts: string;
  /** Nesting under subagent tool calls: 0 for the main agent. */
  depth: number;
};

/** `queued`: sent mid-turn and not read by the agent yet (it steers the turn). */
export type UserItem = Base & {
  kind: "user";
  text: string;
  queued: boolean;
  attachments: Attachment[];
  messageId: string | null;
  /** The guest of the workspace's share who suggested it (story 53); the agent never sees it. */
  suggestedBy?: CommentAuthor;
  /** The delegated chat whose answer Chauffeur sent, the parent's turn having ended before (story 49). */
  delegatedAnswer?: DelegatedAnswer;
};
export type AssistantItem = Base & {
  kind: "assistant";
  messageId: string;
  text: string;
  /** Still receiving `text_delta`s; replaced by the final `assistant_text`. */
  streaming: boolean;
  parentToolUseId: string | null;
};
export type ThinkingItem = Base & {
  kind: "thinking";
  messageId: string;
  text: string;
  streaming: boolean;
};
export type ToolItem = Base & {
  kind: "tool";
  toolUseId: string;
  name: string;
  input: JsonValue;
  parentToolUseId: string | null;
  /** One line: the command, the file path, the pattern… */
  summary: string;
  /** `attachments`: the result's images (a screenshot, an image the agent read). */
  result: { output: string; isError: boolean; attachments: Attachment[] } | null;
  /** An AskUserQuestion call that the user is asked through a question item: shown there only. */
  asked?: boolean;
};
export type RequestState = "open" | "resolved" | "expired";
export type PermissionItem = Base & {
  kind: "permission";
  requestId: string;
  toolName: string;
  input: JsonValue;
  title: string | null;
  summary: string;
  state: RequestState;
  outcome: string | null;
  /** Allowed with Always allow: the rules the chat keeps allowing, or the one that covered it. */
  rules: string[] | null;
  /** The subagent asking, when one does rather than the agent itself. */
  agent: RequestAgent | null;
};
export type ElicitationItem = Base & {
  kind: "elicitation";
  requestId: string;
  serverName: string;
  message: string;
  schema: JsonValue;
  url: string | null;
  error: string | null;
  /** What Always allow would keep accepting in the chat; null when it is not offered. */
  rule: string | null;
  state: RequestState;
  outcome: string | null;
  /** Accepted with Always allow, or by the rule the chat already kept. */
  rules: string[] | null;
};
export type QuestionItem = Base & {
  kind: "question";
  requestId: string;
  questions: Question[];
  state: RequestState;
  outcome: string | null;
  /** The answers given, by question, read from the AskUserQuestion tool result. */
  answers: Record<string, string> | null;
};
/** `count` is how many identical notices in a row it stands for; absent for a single one. */
export type NoticeItem = Base & { kind: "notice"; level: NoticeLevel; message: string; badge?: string; count?: number };
/** A command the user ran from the chat (`!`): running until its `shell_ended`. */
export type ShellItem = Base & {
  kind: "shell";
  shellId: string;
  command: string;
  running: boolean;
  exitCode: number | null;
  output: string;
  timedOut: boolean;
};
export type TurnItem = Base & {
  kind: "turn";
  /** Its number, counted from 1 as `chat.fork` counts turns. */
  turn: number;
  outcome: TurnOutcome;
  durationMs: number | null;
  costUsd: number | null;
  costEstimated: boolean;
  /** Tokens the turn used, every model call summed; null on older turns. */
  tokens: number | null;
  /** Tokens in the context window after the turn. */
  contextTokens: number | null;
  error: string | null;
  /** The agent said where it ended in its session: a fork can start from there. */
  forkable: boolean;
};

export type ChatItem =
  | UserItem
  | AssistantItem
  | ThinkingItem
  | ToolItem
  | PermissionItem
  | ElicitationItem
  | QuestionItem
  | NoticeItem
  | ShellItem
  | TurnItem;

/**
 * A subagent's text or thinking: shown in its own view, never in the chat. `anchor` is how many
 * chat items there were when it came, which places it among the subagent's tool calls.
 */
export type SubagentTextItem = (AssistantItem | ThinkingItem) & { anchor: number };

/**
 * What the chat saw of a subagent, by the tool call that started it: its task (kept after it
 * ends), and its text and thinking, live only (never stored: its transcript has them).
 */
export type SubagentLog = {
  taskId: string | null;
  background: boolean;
  /** Timestamps of its `task_started` and `task_ended`. */
  startedAt: string | null;
  endedAt: string | null;
  status: TaskStatus | null;
  /** From `task_progress`. */
  durationMs: number | null;
  toolUses: number | null;
  items: SubagentTextItem[];
};

/** A task of the agent running now: a background command, a subagent, a workflow. */
export type TaskView = {
  taskId: string;
  kind: TaskKind;
  /** The tool call that started it (a subagent's Agent call). */
  toolUseId: string | null;
  description: string;
  subagentType: string | null;
  background: boolean;
  parentTaskId: string | null;
  /** Timestamp of its first `task_started`. */
  startedAt: string;
  /** From `task_progress`, while the task runs. */
  lastTool: string | null;
  /** What it is doing now, when it says. */
  activity: string | null;
  summary: string | null;
  toolUses: number | null;
};

export type ChatView = {
  items: ChatItem[];
  /** From `status_changed`; null until the first one. */
  status: ChatStatus | null;
  /** Between `turn_started` and `turn_ended`. */
  working: boolean;
  /** Timestamp of the running turn's `turn_started`. */
  turnStartedAt: string | null;
  /** Last persisted seq applied. */
  lastSeq: number;
  model: string | null;
  sessionId: string | null;
  /** Cost of the chat so far: the sum of its turns' costs. */
  costUsd: number;
  costEstimated: boolean;
  /** Last `cost_usd` reported, a running total of the agent session (reset when it restarts). */
  reportedCostUsd: number;
  /** Last `session_tokens` reported, a running total of the agent session (reset when it restarts). */
  reportedTokens: number;
  turns: number;
  /** Number of ephemeral events seen (to key streaming items uniquely). */
  counter: number;
  /** Messages waiting for the end of the turn (`queue_changed`). */
  queue: QueuedMessage[];
  /** The agent's tasks running now, oldest first; a task leaves at its `task_ended`. */
  tasks: TaskView[];
  /**
   * The agent's guess at the next message (`prompt_suggestion`, ephemeral), until a turn starts;
   * `key` tells one suggestion from the next.
   */
  suggestion: { text: string; key: string } | null;
  /** The last reading of the context window (`context_usage`, one per turn) and when it was read. */
  context: { usage: ContextUsage; readAt: string } | null;
  /** The subagents, by the tool call that started them. */
  subagents: Record<string, SubagentLog>;
  /**
   * Tool calls (theirs, or a subagent's) that auto mode is checking before they run, by tool use
   * id (`permission_check`, ephemeral). A lost "done" must not leave one: a call also leaves at
   * its result, and every one at the end of the turn.
   */
  checking: string[];
};

export const emptyChatView: ChatView = {
  items: [],
  status: null,
  working: false,
  turnStartedAt: null,
  lastSeq: 0,
  model: null,
  sessionId: null,
  costUsd: 0,
  costEstimated: false,
  reportedCostUsd: 0,
  reportedTokens: 0,
  turns: 0,
  counter: 0,
  queue: [],
  tasks: [],
  suggestion: null,
  context: null,
  subagents: {},
  checking: [],
};

/** The chat events of an envelope, or null for any other scope. */
export function chatEventOf(envelope: EventEnvelope): { chatId: string; event: ChatEvent } | null {
  const event = envelope.event;
  if (event.scope !== "chat") return null;
  return { chatId: event.chat_id, event: event.event };
}

/** Applies one envelope of this chat. Persisted envelopes already applied are ignored. */
export function reduceChat(view: ChatView, envelope: EventEnvelope): ChatView {
  const chat = chatEventOf(envelope);
  if (!chat) return view;
  if (envelope.seq != null) {
    if (envelope.seq <= view.lastSeq) return view;
  }
  const next = apply(view, chat.event, envelope);
  if (envelope.seq != null && next.lastSeq < envelope.seq) {
    return next === view ? { ...view, lastSeq: envelope.seq } : { ...next, lastSeq: envelope.seq };
  }
  return next;
}

/** Reduces a list of envelopes from an empty view. */
export function buildChatView(envelopes: EventEnvelope[]): ChatView {
  return envelopes.reduce(reduceChat, emptyChatView);
}

function apply(current: ChatView, event: ChatEvent, envelope: EventEnvelope): ChatView {
  const ts = envelope.ts;
  // Ephemeral envelopes have no seq: a counter keys the items they create.
  const view = envelope.seq == null ? { ...current, counter: current.counter + 1 } : current;
  const key = (suffix: string) =>
    envelope.seq != null ? `s${envelope.seq}` : `e${view.counter}-${suffix}`;
  switch (event.type) {
    case "user_message":
      return push(view.suggestion ? { ...view, suggestion: null } : view, {
        kind: "user",
        key: key("user"),
        ts,
        depth: 0,
        text: event.text,
        queued: event.queued,
        attachments: event.attachments ?? [],
        messageId: event.message_id ?? null,
        ...(event.suggested_by ? { suggestedBy: event.suggested_by } : {}),
        ...(event.delegated_answer ? { delegatedAnswer: event.delegated_answer } : {}),
      });

    case "user_message_delivered": {
      const index = findLast(view.items, (item) => item.kind === "user" && item.messageId === event.message_id);
      const item = index >= 0 ? (view.items[index] as UserItem) : null;
      return item?.queued ? replace(view, index, { ...item, queued: false }) : view;
    }

    case "queue_changed":
      return { ...view, queue: event.messages };

    case "agent_changed":
      return {
        ...view,
        sessionId: null,
        model: null,
        context: null,
        suggestion: null,
        reportedCostUsd: 0,
        reportedTokens: 0,
        items: [
          ...view.items.map((item) => item.kind === "turn" ? { ...item, forkable: false } : item),
          {
            kind: "notice", key: key("provider"), ts, depth: 0, level: "info",
            message: `Switched from ${AGENT_NAMES[event.from]} to ${AGENT_NAMES[event.to]}. The conversation will be handed over as text with your next message.`,
          },
        ],
      };

    case "session_started":
      return {
        ...view,
        sessionId: event.agent_session_id,
        model: event.model ?? view.model,
        reportedCostUsd: event.agent_session_id === view.sessionId ? view.reportedCostUsd : 0,
        reportedTokens: event.agent_session_id === view.sessionId ? view.reportedTokens : 0,
      };

    case "turn_started":
      return { ...view, working: true, turnStartedAt: ts, suggestion: null };

    case "context_usage":
      return { ...view, context: { usage: event.usage, readAt: ts } };

    case "plan_updated":
      // Counted by the daemon into the chat's `progress`; not a row of the conversation.
      return view;

    case "permission_mode_effective":
    case "mcp_status":
    case "account_lost":
    case "design_share":
    case "suggestions":
    case "share_comments":
      // The daemon keeps these on the chat (`permission_mode_effective`, `mcp_servers`), the
      // agent's login (`usage.get`), the share (`design.share_state`), the suggestions
      // (`chat.suggestions`) or the guests' comments (`chat.share_comments`), never in the log.
      return view;

    case "permission_check": {
      if (!event.checking) return checkOver(view, event.tool_use_id);
      if (view.checking.includes(event.tool_use_id)) return view;
      // Late for a call that already has its result: dropped.
      const call = findLast(view.items, (item) => item.kind === "tool" && item.toolUseId === event.tool_use_id);
      if (call >= 0 && (view.items[call] as ToolItem).result) return view;
      return { ...view, checking: [...view.checking, event.tool_use_id] };
    }

    case "prompt_suggestion":
      // Late for a turn already followed by another: dropped.
      return view.working ? view : { ...view, suggestion: { text: event.text, key: key("suggestion") } };

    case "text_delta":
    case "thinking_delta": {
      const kind = event.type === "text_delta" ? "assistant" : "thinking";
      // A subagent's own words go to its view, never into the chat.
      if (event.parent_tool_use_id) return subagentText(view, event.parent_tool_use_id, kind, event.message_id, event.text, true, ts, key);
      const index = findLast(
        view.items,
        (item) =>
          item.kind === kind &&
          (item as AssistantItem | ThinkingItem).messageId === event.message_id &&
          (item as AssistantItem | ThinkingItem).streaming,
      );
      if (index >= 0) {
        const item = view.items[index] as AssistantItem | ThinkingItem;
        return replace(view, index, { ...item, text: item.text + event.text });
      }
      const item: AssistantItem | ThinkingItem =
        kind === "assistant"
          ? {
              kind,
              key: key(event.message_id),
              ts,
              depth: 0,
              messageId: event.message_id,
              text: event.text,
              streaming: true,
              parentToolUseId: null,
            }
          : {
              kind,
              key: key(event.message_id),
              ts,
              depth: 0,
              messageId: event.message_id,
              text: event.text,
              streaming: true,
            };
      return push(view, item);
    }

    case "assistant_text": {
      if (event.parent_tool_use_id) {
        return subagentText(view, event.parent_tool_use_id, "assistant", event.message_id, event.text, false, ts, key);
      }
      const depth = depthUnder(view.items, event.parent_tool_use_id);
      const index = findLast(
        view.items,
        (item) => item.kind === "assistant" && item.messageId === event.message_id && item.streaming,
      );
      if (index >= 0) {
        const item = view.items[index] as AssistantItem;
        return replace(view, index, {
          ...item,
          text: event.text,
          streaming: false,
          parentToolUseId: event.parent_tool_use_id,
          depth,
        });
      }
      return push(view, {
        kind: "assistant",
        key: key("assistant"),
        ts,
        depth,
        messageId: event.message_id,
        text: event.text,
        streaming: false,
        parentToolUseId: event.parent_tool_use_id,
      });
    }

    case "thinking": {
      if (event.parent_tool_use_id) {
        return subagentText(view, event.parent_tool_use_id, "thinking", event.message_id, event.text, false, ts, key);
      }
      const index = findLast(
        view.items,
        (item) => item.kind === "thinking" && item.messageId === event.message_id && item.streaming,
      );
      if (index >= 0) {
        const item = view.items[index] as ThinkingItem;
        return replace(view, index, { ...item, text: event.text, streaming: false });
      }
      return push(view, {
        kind: "thinking",
        key: key("thinking"),
        ts,
        depth: 0,
        messageId: event.message_id,
        text: event.text,
        streaming: false,
      });
    }

    case "tool_call":
      return push(view, {
        kind: "tool",
        key: key("tool"),
        ts,
        depth: depthUnder(view.items, event.parent_tool_use_id),
        toolUseId: event.tool_use_id,
        name: event.name,
        input: event.input,
        parentToolUseId: event.parent_tool_use_id,
        summary: toolSummary(event.name, event.input),
        result: null,
        // The question may come first: its item then stands for the call.
        asked: event.name === ASK_USER_QUESTION && findLast(view.items, (item) => isQuestionFor(item, event.input)) >= 0,
      });

    case "tool_result": {
      const checked = checkOver(view, event.tool_use_id);
      const index = findLast(
        checked.items,
        (item) => item.kind === "tool" && item.toolUseId === event.tool_use_id,
      );
      const result = { output: event.output, isError: event.is_error, attachments: event.attachments ?? [] };
      if (index >= 0) {
        const tool = checked.items[index] as ToolItem;
        const next = replace(checked, index, { ...tool, result });
        return tool.name === ASK_USER_QUESTION ? withAnswers(next, parseAnswers(event.output)) : next;
      }
      return push(checked, {
        kind: "tool",
        key: key("tool-result"),
        ts,
        depth: 0,
        toolUseId: event.tool_use_id,
        name: "tool",
        input: null,
        parentToolUseId: null,
        summary: "",
        result,
      });
    }

    case "mcp_elicitation":
      return push(view, {
        kind: "elicitation", key: key("elicitation"), ts, depth: 0,
        requestId: event.request_id, serverName: event.server_name, message: event.message,
        schema: event.schema, url: event.url, error: event.error, rule: event.rule ?? null, state: "open", outcome: null, rules: null,
      });

    case "permission_request":
      return push(view, {
        kind: "permission",
        key: key("permission"),
        ts,
        depth: 0,
        requestId: event.request_id,
        toolName: event.tool_name,
        input: event.input,
        title: event.title,
        summary: toolSummary(event.tool_name, event.input),
        state: "open",
        outcome: null,
        rules: null,
        agent: event.agent ?? null,
      });

    case "question": {
      // The AskUserQuestion call it answers is shown through the question only.
      const call = findLast(
        view.items,
        (item) => item.kind === "tool" && item.name === ASK_USER_QUESTION && item.result == null && !item.asked,
      );
      const withCall = call >= 0 ? replace(view, call, { ...(view.items[call] as ToolItem), asked: true }) : view;
      return push(withCall, {
        kind: "question",
        key: key("question"),
        ts,
        depth: 0,
        requestId: event.request_id,
        questions: event.questions,
        state: "open",
        outcome: null,
        answers: null,
      });
    }

    case "request_resolved": {
      const index = findLast(
        view.items,
        (item) =>
          (item.kind === "permission" || item.kind === "question" || item.kind === "elicitation") && item.requestId === event.request_id,
      );
      if (index < 0) return view;
      const item = view.items[index] as PermissionItem | QuestionItem | ElicitationItem;
      return replace(
        view,
        index,
        item.kind === "question"
          ? { ...item, state: "resolved", outcome: event.outcome }
          : { ...item, state: "resolved", outcome: event.outcome, rules: event.rules?.length ? event.rules : null },
      );
    }

    case "turn_ended": {
      // Anything still streaming or open belongs to the finished turn; a message sent mid-turn
      // was read by now, or opens the next turn (older events carry no delivery report).
      const items = view.items.map((item) => {
        if (item.kind === "user" && item.queued) return { ...item, queued: false };
        if ((item.kind === "assistant" || item.kind === "thinking") && item.streaming) {
          return { ...item, streaming: false };
        }
        if ((item.kind === "permission" || item.kind === "question" || item.kind === "elicitation") && item.state === "open") {
          return { ...item, state: "expired" as const };
        }
        return item;
      });
      // `cost_usd` is the session's running total: the turn cost is the increase. A lower total
      // means the agent session restarted (resumed after a crash) and counts from zero again.
      const total = event.cost_usd;
      const estimated = event.estimated_cost_usd != null;
      const turnCost =
        event.estimated_cost_usd ??
        (total == null ? null : total >= view.reportedCostUsd ? total - view.reportedCostUsd : total);
      // Codex counts the turn's tokens; Claude Code gives the session's running total, as its cost.
      const sessionTokens = event.session_tokens;
      const turnTokens =
        event.tokens ??
        (sessionTokens == null ? null : sessionTokens >= view.reportedTokens ? sessionTokens - view.reportedTokens : sessionTokens);
      return {
        ...view,
        items: [
          ...items,
          {
            kind: "turn",
            key: key("turn"),
            ts,
            depth: 0,
            turn: view.turns + 1,
            outcome: event.outcome,
            durationMs: event.duration_ms,
            costUsd: turnCost,
            costEstimated: estimated,
            tokens: turnTokens,
            contextTokens: event.context_tokens,
            error: event.error,
            forkable: event.outcome === "success" && event.fork_point != null,
          },
        ],
        working: false,
        turnStartedAt: null,
        checking: [],
        turns: view.turns + 1,
        costUsd: view.costUsd + (turnCost ?? 0),
        costEstimated: view.costEstimated || estimated,
        reportedCostUsd: total ?? view.reportedCostUsd,
        reportedTokens: sessionTokens ?? view.reportedTokens,
      };
    }

    case "status_changed":
      return {
        ...view,
        // A closed chat (its workspace archived) has no agent left to run anything.
        tasks: event.status === "closed" ? [] : view.tasks,
        suggestion: event.status === "idle" ? view.suggestion : null,
        // Interrupted, crashed or closed: no check runs any more.
        checking: event.status === "working" || event.status === "needs_input" ? view.checking : [],
        status: event.status,
        working:
          event.status === "working" || event.status === "needs_input"
            ? true
            : event.status === "idle" || event.status === "error" || event.status === "closed"
              ? false
              : view.working,
      };

    case "task_started": {
      const known = view.tasks.find((task) => task.taskId === event.task_id);
      const task: TaskView = {
        taskId: event.task_id,
        kind: event.kind,
        toolUseId: event.tool_use_id,
        description: event.description,
        subagentType: event.subagent_type,
        background: event.background,
        parentTaskId: event.parent_task_id,
        // Sent again when the task changes: it keeps its start and its progress.
        startedAt: known?.startedAt ?? ts,
        lastTool: known?.lastTool ?? null,
        activity: known?.activity ?? null,
        summary: known?.summary ?? null,
        toolUses: known?.toolUses ?? null,
      };
      const next = {
        ...view,
        tasks: known ? view.tasks.map((t) => (t === known ? task : t)) : [...view.tasks, task],
      };
      if (event.kind !== "subagent" || !event.tool_use_id) return next;
      return withSubagent(next, event.tool_use_id, (log) => ({
        ...log,
        taskId: event.task_id,
        background: event.background,
        startedAt: log.startedAt ?? ts,
      }));
    }

    case "task_progress": {
      const known = view.tasks.find((task) => task.taskId === event.task_id);
      if (!known) return view;
      const task: TaskView = {
        ...known,
        activity: event.activity ?? known.activity,
        lastTool: event.last_tool ?? known.lastTool,
        summary: event.summary ?? known.summary,
        toolUses: event.tool_uses ?? known.toolUses,
      };
      const next = { ...view, tasks: view.tasks.map((t) => (t === known ? task : t)) };
      const of = subagentOf(next, event.task_id);
      return of
        ? withSubagent(next, of, (log) => ({
            ...log,
            durationMs: event.duration_ms ?? log.durationMs,
            toolUses: event.tool_uses ?? log.toolUses,
          }))
        : next;
    }

    case "task_ended": {
      const next = view.tasks.some((task) => task.taskId === event.task_id)
        ? { ...view, tasks: view.tasks.filter((task) => task.taskId !== event.task_id) }
        : view;
      const of = subagentOf(next, event.task_id);
      return of
        ? withSubagent(next, of, (log) => ({
            ...log,
            status: event.status,
            endedAt: ts,
            items: log.items.map((item) => (item.streaming ? { ...item, streaming: false } : item)),
          }))
        : next;
    }

    case "shell_started":
      return push(view, {
        kind: "shell",
        key: key("shell"),
        ts,
        depth: 0,
        shellId: event.shell_id,
        command: event.command,
        running: true,
        exitCode: null,
        output: "",
        timedOut: false,
      });

    case "shell_ended": {
      const ended = { running: false, exitCode: event.exit_code, output: event.output, timedOut: event.timed_out };
      const index = findLast(view.items, (item) => item.kind === "shell" && item.shellId === event.shell_id);
      if (index >= 0) return replace(view, index, { ...(view.items[index] as ShellItem), ...ended });
      return push(view, {
        kind: "shell",
        key: key("shell"),
        ts,
        depth: 0,
        shellId: event.shell_id,
        command: event.command,
        ...ended,
      });
    }

    case "notice": {
      const last = view.items.at(-1);
      if (last?.kind === "notice" && last.level === event.level && last.message === event.message && last.badge === (event.badge || undefined)) {
        return replace(view, view.items.length - 1, { ...last, count: (last.count ?? 1) + 1 });
      }
      return push(view, {
        kind: "notice",
        key: key("notice"),
        ts,
        depth: 0,
        level: event.level,
        message: event.message,
        ...(event.badge ? { badge: event.badge } : {}),
      });
    }
  }
}

/** The view without a check on this tool call; the same view when there was none. */
function checkOver(view: ChatView, toolUseId: string): ChatView {
  return view.checking.includes(toolUseId)
    ? { ...view, checking: view.checking.filter((id) => id !== toolUseId) }
    : view;
}

const emptySubagent: SubagentLog = {
  taskId: null,
  background: false,
  startedAt: null,
  endedAt: null,
  status: null,
  durationMs: null,
  toolUses: null,
  items: [],
};

function withSubagent(view: ChatView, toolUseId: string, update: (log: SubagentLog) => SubagentLog): ChatView {
  return { ...view, subagents: { ...view.subagents, [toolUseId]: update(view.subagents[toolUseId] ?? emptySubagent) } };
}

/** The tool call that started the subagent task `taskId`. */
function subagentOf(view: ChatView, taskId: string): string | null {
  for (const [toolUseId, log] of Object.entries(view.subagents)) if (log.taskId === taskId) return toolUseId;
  return null;
}

/** A subagent's text or thinking, streamed (`delta`: a chunk to add) or whole. */
function subagentText(
  view: ChatView,
  parent: string,
  kind: "assistant" | "thinking",
  messageId: string,
  text: string,
  delta: boolean,
  ts: string,
  key: (suffix: string) => string,
): ChatView {
  return withSubagent(view, parent, (log) => {
    const index = log.items.findLastIndex((item) => item.kind === kind && item.messageId === messageId && item.streaming);
    if (index >= 0) {
      const items = log.items.slice();
      const item = items[index]!;
      items[index] = { ...item, text: delta ? item.text + text : text, streaming: delta };
      return { ...log, items };
    }
    const base = { key: key(`sub-${messageId}`), ts, depth: 0, messageId, text, streaming: delta, anchor: view.items.length };
    const item: SubagentTextItem = kind === "assistant" ? { ...base, kind, parentToolUseId: parent } : { ...base, kind };
    return { ...log, items: [...log.items, item] };
  });
}

function push(view: ChatView, item: ChatItem): ChatView {
  return { ...view, items: [...view.items, item] };
}

function replace(view: ChatView, index: number, item: ChatItem): ChatView {
  const items = view.items.slice();
  items[index] = item;
  return { ...view, items };
}

function findLast(items: ChatItem[], predicate: (item: ChatItem) => boolean): number {
  for (let i = items.length - 1; i >= 0; i--) {
    if (predicate(items[i])) return i;
  }
  return -1;
}

/** Depth of an item made under the tool call `parent` (a subagent's Task call). */
function depthUnder(items: ChatItem[], parent: string | null): number {
  if (!parent) return 0;
  const index = findLast(items, (item) => item.kind === "tool" && item.toolUseId === parent);
  if (index < 0) return 1;
  return Math.min(4, items[index].depth + 1);
}

/**
 * The answers in Claude's AskUserQuestion tool result, e.g.
 * `Your questions have been answered: "Which color?"="Blue", "Why?"="Because". …`.
 */
export function parseAnswers(output: string): Record<string, string> {
  const answers: Record<string, string> = {};
  const pair = /"((?:[^"\\]|\\.)*)"="((?:[^"\\]|\\.)*)"/g;
  for (let match = pair.exec(output); match; match = pair.exec(output)) {
    answers[match[1].replace(/\\(.)/g, "$1")] = match[2].replace(/\\(.)/g, "$1");
  }
  return answers;
}

export const ASK_USER_QUESTION = "AskUserQuestion";

/** An open question item asking what an AskUserQuestion call's input asks. */
function isQuestionFor(item: ChatItem, input: JsonValue): boolean {
  if (item.kind !== "question" || item.state !== "open") return false;
  const asked = input && typeof input === "object" && !Array.isArray(input) ? input.questions : null;
  const first = Array.isArray(asked) && asked[0] && typeof asked[0] === "object" && !Array.isArray(asked[0]) ? asked[0].question : null;
  return typeof first === "string" && item.questions[0]?.question === first;
}

/** Attaches answers to the latest question card that asked them. */
function withAnswers(view: ChatView, answers: Record<string, string>): ChatView {
  const asked = Object.keys(answers);
  if (asked.length === 0) return view;
  const index = findLast(
    view.items,
    (item) => item.kind === "question" && item.questions.some((q) => asked.includes(q.question)),
  );
  if (index < 0) return view;
  return replace(view, index, { ...(view.items[index] as QuestionItem), answers });
}

/**
 * A permission card's heading. A subagent's request names it in place of "Claude" (Claude Code's
 * titles read "Claude wants to …"): "general-purpose (background) wants to run …".
 */
export function permissionTitle(item: Pick<PermissionItem, "title" | "toolName" | "agent">): string {
  if (!item.agent) return item.title ?? `Allow ${item.toolName}?`;
  const who = `${item.agent.subagent_type ?? "A subagent"}${item.agent.background ? " (background)" : ""}`;
  if (!item.title) return `${who} wants to use ${item.toolName}`;
  return /^Claude\b/.test(item.title) ? item.title.replace(/^Claude\b/, who) : `${who}: ${item.title}`;
}

/**
 * A rule allowed always as its card shows it. Codex's rules name the command's folder
 * (`Bash(make) in /…/workspace`): the workspace's own reads "the workspace folder".
 */
export function ruleLabel(rule: string, workspace: string | null): string {
  if (!workspace) return rule;
  const base = withoutPrivate(workspace.replace(/\/$/, ""));
  for (const folder of [base, `/private${base}`]) {
    if (rule.endsWith(` in ${folder}`)) return `${rule.slice(0, -folder.length)}the workspace folder`;
  }
  return rule;
}

/** Chauffeur's tool through which the agent proposes a repo for its workspace (story 48), whatever the agent. */
export const ADD_REPO_TOOL = "mcp__chauffeur__add_repo";

/** What an `add_repo` request proposes: the repo, the agent's reason, the repo's description. */
export type RepoProposal = { repo: string; reason: string | null; description: string | null };

/** The repo an `add_repo` request proposes, or null for any other request. */
export function repoProposal(item: Pick<PermissionItem, "toolName" | "input">): RepoProposal | null {
  const input = item.input;
  if (item.toolName !== ADD_REPO_TOOL || input == null || typeof input !== "object" || Array.isArray(input)) return null;
  const repo = str(input.repo);
  if (!repo) return null;
  return { repo, reason: str(input.reason)?.trim() || null, description: str(input.description)?.trim() || null };
}

/**
 * How an `add_repo` request ended, for its card: the user's answer. Whether the repo was then added
 * is the tool call's result (an addition can still fail), which the agent reports.
 */
export function repoProposalOutcome(item: Pick<PermissionItem, "state" | "outcome">): string | null {
  if (item.state === "open") return null;
  if (item.state === "expired") return "expired";
  switch (item.outcome) {
    case "allowed":
      return "accepted";
    case "denied":
      return "not now";
    case "cancelled":
      return "withdrawn";
    default:
      return item.outcome;
  }
}

/** Chauffeur's tool through which the agent gives a task to a delegated chat (story 49), whatever the agent. */
export const DELEGATE_TOOL = "mcp__chauffeur__delegate";

/** A `delegate` call of the chat's own agent: shown as its own card, never folded among the steps. */
export function isDelegateCall(item: ChatItem): boolean {
  return item.kind === "tool" && item.name === DELEGATE_TOOL && item.depth === 0;
}

/**
 * The delegated chat a `delegate` call gave its task to: the one its input names (more work for a
 * delegated chat), else the one the call started; null until the daemon has it.
 */
export function delegatedChat(item: Pick<ToolItem, "toolUseId" | "input">, parentId: string, chats: ChatInfo[]): ChatInfo | null {
  const input = item.input != null && typeof item.input === "object" && !Array.isArray(item.input) ? item.input : null;
  const named = input ? str(input.chat) : null;
  const ofParent = (chat: ChatInfo) => chat.parent_chat === parentId;
  if (named) return chats.find((chat) => chat.id === named && ofParent(chat)) ?? null;
  return chats.find((chat) => ofParent(chat) && chat.parent_tool_use === item.toolUseId) ?? null;
}

/** The agent a `delegate` call asked for: its delegated chat's, else its input's. */
export function delegatedAgent(item: Pick<ToolItem, "input">, chat: ChatInfo | null): AgentKind {
  if (chat) return chat.agent;
  const input = item.input != null && typeof item.input === "object" && !Array.isArray(item.input) ? item.input : null;
  return input?.agent === "codex" ? "codex" : "claude";
}

/** Open permission requests and questions, oldest first. */
export function openRequests(view: ChatView): Array<PermissionItem | QuestionItem | ElicitationItem> {
  return view.items.filter(
    (item): item is PermissionItem | QuestionItem | ElicitationItem =>
      (item.kind === "permission" || item.kind === "question" || item.kind === "elicitation") && item.state === "open",
  );
}

function str(value: JsonValue | undefined): string | null {
  return typeof value === "string" && value.length > 0 ? value : null;
}

/** The one line worth showing for a tool call: a command, a file path, a pattern, a URL. */
export function toolSummary(name: string, input: JsonValue): string {
  if (input == null || typeof input !== "object" || Array.isArray(input)) {
    return typeof input === "string" ? input : "";
  }
  const field = (key: string) => str(input[key]);
  const firstLine = (text: string | null) => (text ? text.split("\n")[0] : null);
  const lower = name.toLowerCase();
  if (lower === "bash" || lower === "shell" || lower === "exec_command" || lower === "local_shell") {
    const command = input.command;
    if (Array.isArray(command)) return command.filter((c) => typeof c === "string").join(" ");
    return firstLine(field("command")) ?? field("description") ?? "";
  }
  if (lower === "grep" || lower === "glob") {
    const pattern = field("pattern") ?? "";
    const path = field("path");
    return path ? `${pattern} in ${path}` : pattern;
  }
  if (lower === "task" || lower === "agent") {
    return field("description") ?? firstLine(field("prompt")) ?? "";
  }
  if (name === ADD_REPO_TOOL) return field("repo") ?? "";
  if (name === DELEGATE_TOOL) return firstLine(field("prompt")) ?? "";
  return (
    field("file_path") ??
    field("notebook_path") ??
    field("path") ??
    field("url") ??
    field("query") ??
    firstLine(field("command")) ??
    field("description") ??
    field("pattern") ??
    ""
  );
}

/** The agent's steps between two of its messages (tool calls, thinking, a subagent's work), folded into one row. */
export type ActivityGroup = { kind: "activity"; key: string; items: ChatItem[] };
export type ChatRow = ChatItem | ActivityGroup;

/** A step of the agent's work rather than something said to or by the user. Requests are never folded: they wait for an answer. */
function isActivity(item: ChatItem): boolean {
  if (isDelegateCall(item)) return false;
  if (item.kind === "tool" || item.kind === "thinking") return true;
  return item.depth > 0 && (item.kind === "assistant" || item.kind === "notice");
}

// The last group built on each first step: a group whose steps did not change is the same object
// from one pass to the next, so that its memoised row does not render again.
const groups = new WeakMap<ChatItem, ActivityGroup>();

function groupOf(run: ChatItem[]): ActivityGroup {
  const known = groups.get(run[0]);
  if (known && known.items.length === run.length && known.items.every((item, i) => item === run[i])) return known;
  const group: ActivityGroup = { kind: "activity", key: `activity-${run[0].key}`, items: run };
  groups.set(run[0], group);
  return group;
}

/**
 * The rows of the chat view: each run of two or more steps becomes one group, keyed by its first
 * step so that it keeps its key (and its open state) while the run grows. A group whose steps are
 * unchanged is the object returned before.
 */
export function groupActivity(items: ChatItem[]): ChatRow[] {
  const rows: ChatRow[] = [];
  let run: ChatItem[] = [];
  const flush = () => {
    if (run.length === 1) rows.push(run[0]);
    else if (run.length > 1) rows.push(groupOf(run));
    run = [];
  };
  for (const item of items) {
    if (isActivity(item)) {
      run.push(item);
    } else {
      flush();
      rows.push(item);
    }
  }
  flush();
  return rows;
}

/** What a folded group did, for its one line: tool names by count, most used first, and failures. */
export function activitySummary(group: ActivityGroup): {
  tools: number;
  names: Array<{ name: string; count: number }>;
  failed: number;
  pending: boolean;
} {
  const counts = new Map<string, number>();
  let tools = 0;
  let failed = 0;
  let pending = false;
  for (const item of group.items) {
    if (item.kind !== "tool") continue;
    tools += 1;
    const name = shortToolName(item.name);
    counts.set(name, (counts.get(name) ?? 0) + 1);
    if (item.result == null) pending = true;
    else if (item.result.isError) failed += 1;
  }
  const names = [...counts].map(([name, count]) => ({ name, count })).sort((a, b) => b.count - a.count);
  return { tools, names, failed, pending };
}

/** `mcp__gitnexus__list_repos` → `gitnexus list_repos`. */
export function shortToolName(name: string): string {
  const mcp = /^mcp__(.+?)__(.+)$/.exec(name);
  return mcp ? `${mcp[1]} ${mcp[2]}` : name;
}

/** The task's last step: its own summary, else what it does now, else the tool it used last; and how many tools it used. */
export function taskStep(task: TaskView): string | null {
  const tools = task.toolUses ? `${task.toolUses} tool ${task.toolUses === 1 ? "use" : "uses"}` : null;
  const step = task.summary ?? task.activity ?? (task.lastTool ? shortToolName(task.lastTool) : null);
  if (step && tools) return `${step} · ${tools}`;
  return step ?? tools;
}
