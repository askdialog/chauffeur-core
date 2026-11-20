// A whole session's conversation, read from its transcript (`<folder>/<session>.jsonl`) without
// running it: a session imported from Claude Code, shown as its chat. The subagent view's mapping,
// with the user's prompts.

import type { SessionMessage } from "@anthropic-ai/claude-agent-sdk";
import {
  ErrorCode,
  type AgentSessionMessagesParams,
  type AgentSessionMessagesResult,
  type TranscriptEvent,
} from "../../protocol.ts";
import { RpcError } from "../../rpc/peer.ts";
import { ClaudeEventMapper, notice } from "./events.ts";
import { messageText, SAFE_ID, shownEvents, type TranscriptReader } from "./subagents.ts";

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
  const command = /<command-name>([^<]*)<\/command-name>/.exec(text)?.[1];
  if (command !== undefined) {
    const args = /<command-args>([^<]*)<\/command-args>/.exec(text)?.[1]?.trim();
    return [command.trim(), args].filter(Boolean).join(" ");
  }
  if (text.startsWith("<") || /^\[Request interrupted by user/.test(text)) return "";
  return text;
}
