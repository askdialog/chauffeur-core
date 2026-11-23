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
    if (message.type === "user") {
      if (message.isCompactSummary === true) {
        events.push({ at, event: notice("info", "Conversation compacted in Claude Code: what came before is in the summary the agent resumes from") });
        continue;
      }
      if (message.is_meta === true) continue;
      const prompt = promptOf(message.message);
      if (prompt !== null) {
        if (prompt) events.push({ at, event: { type: "user_message", text: prompt, queued: false } });
        continue;
      }
    }
    for (const event of shownEvents(mapper, message)) events.push({ at, event });
  }
  return events;
}

/** What Claude Code adds to a prompt on its own (an editor's selection, a reminder): not shown. */
const ADDED = /<(system-reminder|ide_selection|ide_opened_file|ide_diagnostics|local-command-caveat)>[\s\S]*?<\/\1>/g;

/** A message Claude Code writes on the user's side by itself: a command's output, a notice. */
const OWN = /^<(local-command-stdout|local-command-stderr|bash-stdout|bash-stderr|task-notification)>/;

/**
 * What the user typed, from a user message: its text without what Claude Code added, a slash
 * command or a shell line (`!cmd`) as typed, an image alone said so; "" for a message of Claude
 * Code's own (a command's output, a notice, an interruption); null for a message carrying tool
 * results, which the mapping reads.
 */
function promptOf(message: unknown): string | null {
  const content: unknown = typeof message === "object" && message !== null ? Reflect.get(message, "content") : undefined;
  const types = Array.isArray(content)
    ? content.map((block: unknown) => (typeof block === "object" && block !== null ? Reflect.get(block, "type") : undefined))
    : [];
  if (types.includes("tool_result")) return null;
  const raw = messageText(message).trim();
  if (OWN.test(raw) || /^\[Request interrupted by user/.test(raw)) return "";
  const command = /<command-name>([^<]*)<\/command-name>/.exec(raw)?.[1];
  if (command !== undefined) {
    const args = /<command-args>([^<]*)<\/command-args>/.exec(raw)?.[1]?.trim();
    return [command.trim(), args].filter(Boolean).join(" ");
  }
  const shell = /^<bash-input>([\s\S]*)<\/bash-input>$/.exec(raw)?.[1];
  if (shell !== undefined) return `!${shell.trim()}`;
  const text = raw.replace(ADDED, "").trim();
  if (text) return text;
  return types.includes("image") ? "(an image)" : "";
}
