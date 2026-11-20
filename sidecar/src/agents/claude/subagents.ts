// A subagent's conversation, read from the transcript Claude Code keeps for it
// (`~/.claude/projects/<folder>/<session>/subagents/agent-<id>.jsonl`): running or finished, with
// or without a session. The same mapping as live messages, so that the app merges both by id.
// A whole session's too (`<folder>/<session>.jsonl`), for a session imported from Claude Code.

import {
  getSessionMessages,
  getSubagentMessages,
  listSubagents,
  type SDKAssistantMessage,
  type SDKUserMessage,
  type SessionMessage,
} from "@anthropic-ai/claude-agent-sdk";
import {
  ErrorCode,
  type AgentSessionMessagesParams,
  type AgentSessionMessagesResult,
  type AgentSubagentMessagesParams,
  type AgentSubagentMessagesResult,
  type ChatEvent,
  type TranscriptEvent,
} from "../../protocol.ts";
import { RpcError } from "../../rpc/peer.ts";
import { ClaudeEventMapper, notice } from "./events.ts";

/** How transcripts are read: the SDK's readers, replaced in tests. */
export interface TranscriptReader {
  /** A subagent's messages, oldest first; empty when it has no transcript. */
  messages(sessionId: string, agentId: string, dir: string, limit?: number): Promise<SessionMessage[]>;
  /** The ids of a session's subagents that have a transcript. */
  list(sessionId: string, dir: string): Promise<string[]>;
  /** A session's own messages, oldest first, since its last compaction; empty when it has no transcript. */
  session(sessionId: string, dir: string): Promise<SessionMessage[]>;
}

export const sdkTranscripts: TranscriptReader = {
  messages: (sessionId, agentId, dir, limit) => getSubagentMessages(sessionId, agentId, limit ? { dir, limit } : { dir }),
  list: (sessionId, dir) => listSubagents(sessionId, { dir }),
  session: (sessionId, dir) => getSessionMessages(sessionId, { dir }),
};

/** Ids become file names: nothing that could leave the subagents folder. */
const SAFE_ID = /^[A-Za-z0-9_-]{1,128}$/;

/** The events a transcript gives the subagent view. */
const SHOWN = new Set<ChatEvent["type"]>(["assistant_text", "thinking", "tool_call", "tool_result"]);

/**
 * The subagent's conversation. Its id is the one given (its task's id), else the one its tool
 * result names (`agentId: …`, in a foreground subagent's report and a background one's launch
 * message), else the subagent whose first message is its prompt.
 */
export async function claudeSubagentMessages(
  reader: TranscriptReader,
  params: AgentSubagentMessagesParams,
): Promise<AgentSubagentMessagesResult> {
  if (!SAFE_ID.test(params.session_id)) throw new RpcError(ErrorCode.INVALID_PARAMS, "session_id is not a session id");
  if (params.agent_id != null && !SAFE_ID.test(params.agent_id)) {
    throw new RpcError(ErrorCode.INVALID_PARAMS, "agent_id is not an agent id");
  }
  const agentId =
    params.agent_id ?? agentIdIn(params.result) ?? (params.prompt ? await agentByPrompt(reader, params, params.prompt) : null);
  if (!agentId) return { agent_id: null, found: false, events: [] };
  const messages = await reader.messages(params.session_id, agentId, params.cwd);
  if (messages.length === 0) return { agent_id: agentId, found: false, events: [] };
  return { agent_id: agentId, found: true, events: transcriptEvents(messages) };
}

/**
 * The agent id a subagent's tool result names: `agentId: a1b2c3 (use SendMessage …)`, or, when it
 * handed its report back with SubagentHandback, `… delivered to you as a message from "a1b2c3"`.
 */
export function agentIdIn(result: string | null | undefined): string | null {
  const match = result ? (/\bagentId: ([A-Za-z0-9_-]+)/.exec(result) ?? /as a message from "([A-Za-z0-9_-]+)"/.exec(result)) : null;
  return match?.[1] && SAFE_ID.test(match[1]) ? match[1] : null;
}

/** The subagent of the session whose transcript starts with `prompt`. */
async function agentByPrompt(reader: TranscriptReader, params: AgentSubagentMessagesParams, prompt: string): Promise<string | null> {
  const wanted = prompt.trim();
  for (const id of await reader.list(params.session_id, params.cwd)) {
    if (!SAFE_ID.test(id)) continue;
    const [first] = await reader.messages(params.session_id, id, params.cwd, 1);
    if (first?.type === "user" && messageText(first.message).trim() === wanted) return id;
  }
  return null;
}

/**
 * A transcript as chat events of the subagent's own (no `parent_tool_use_id`): its text and
 * thinking with the message ids the live mapping gives them (`<api message id>:<block index>`),
 * its tool calls and results. The prompt (the tool call's input) and the rest of what the user
 * side carries (reminders, notifications) are left out; images too, which the live chat keeps.
 */
export function transcriptEvents(messages: SessionMessage[]): ChatEvent[] {
  const mapper = new ClaudeEventMapper();
  return messages.flatMap((message) => shownEvents(mapper, message));
}

/** One transcript message's events, as `transcriptEvents` keeps them. */
function shownEvents(mapper: ClaudeEventMapper, message: SessionMessage): ChatEvent[] {
  const body = message.message;
  if (typeof body !== "object" || body === null) return [];
  let mapped: ChatEvent[] = [];
  if (message.type === "assistant" && Array.isArray((body as { content?: unknown }).content)) {
    mapped = mapper.map({
      type: "assistant",
      message: body as SDKAssistantMessage["message"],
      parent_tool_use_id: null,
      uuid: message.uuid,
      session_id: message.session_id,
    } as SDKAssistantMessage);
  } else if (message.type === "user") {
    mapped = mapper.map({
      type: "user",
      message: body as SDKUserMessage["message"],
      parent_tool_use_id: null,
      session_id: message.session_id,
    } as SDKUserMessage);
  }
  const events: ChatEvent[] = [];
  for (const event of mapped) {
    if (!SHOWN.has(event.type)) continue;
    if (event.type === "tool_result" && event.images) {
      const { images: _images, ...rest } = event;
      events.push(rest);
    } else {
      events.push(event);
    }
  }
  return events;
}

/** A session's conversation from its transcript, as the chat it is imported into shows it. */
export async function claudeSessionMessages(
  reader: TranscriptReader,
  params: AgentSessionMessagesParams,
): Promise<AgentSessionMessagesResult> {
  if (!SAFE_ID.test(params.session_id)) throw new RpcError(ErrorCode.INVALID_PARAMS, "session_id is not a session id");
  const messages = await reader.session(params.session_id, params.cwd);
  if (messages.length === 0) return { found: false, events: [] };
  return { found: true, events: sessionEvents(messages) };
}

/** What the SDK's reader gives beyond its declared type: when, and which messages Claude Code wrote itself. */
type ReadMessage = SessionMessage & { timestamp?: unknown; isCompactSummary?: unknown; is_meta?: unknown };

/**
 * A session's transcript as chat events, each with its message's time: the user's prompts as
 * `user_message` (a slash command as typed; Claude Code's own notes, interruptions and the
 * messages it writes itself left out), a compaction as a notice, and the rest as the subagent
 * view maps it.
 */
export function sessionEvents(messages: SessionMessage[]): TranscriptEvent[] {
  const mapper = new ClaudeEventMapper();
  const events: TranscriptEvent[] = [];
  for (const message of messages as ReadMessage[]) {
    const at = typeof message.timestamp === "string" ? message.timestamp : null;
    if (message.type === "user" && message.isCompactSummary === true) {
      events.push({ at, event: notice("info", "Conversation compacted in Claude Code: what came before is in the summary the agent resumes from") });
      continue;
    }
    if (message.type === "user" && message.is_meta === true) continue;
    const prompt = message.type === "user" ? promptOf(message.message) : null;
    if (prompt !== null) {
      if (prompt) events.push({ at, event: { type: "user_message", text: prompt, queued: false } });
      continue;
    }
    for (const event of shownEvents(mapper, message)) events.push({ at, event });
  }
  return events;
}

/**
 * What the user typed, from a user message: its text, a slash command as typed, or "" for a
 * message of Claude Code's own (a command's output, a reminder, an interruption); null for a
 * message carrying tool results, which the mapping reads.
 */
function promptOf(message: unknown): string | null {
  const content = typeof message === "object" && message !== null ? (message as { content?: unknown }).content : undefined;
  if (Array.isArray(content) && content.some((block) => typeof block === "object" && block !== null && (block as { type?: unknown }).type === "tool_result")) {
    return null;
  }
  const text = messageText(message).trim();
  const command = /<command-name>([^<]*)<\/command-name>/.exec(text);
  if (command) {
    const args = /<command-args>([^<]*)<\/command-args>/.exec(text)?.[1]?.trim();
    return [command[1]!.trim(), args].filter(Boolean).join(" ");
  }
  if (text.startsWith("<") || /^\[Request interrupted by user/.test(text)) return "";
  return text;
}

/** A message's text: its string content, or its text blocks. */
function messageText(message: unknown): string {
  const content = typeof message === "object" && message !== null ? (message as { content?: unknown }).content : undefined;
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .map((block) => (typeof block === "object" && block !== null && (block as { type?: unknown }).type === "text" ? String((block as { text?: unknown }).text ?? "") : ""))
    .join("");
}
