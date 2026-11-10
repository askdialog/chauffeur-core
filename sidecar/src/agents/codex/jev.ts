// Compaction of a Codex thread through Jev (TypeSafe): the thread's history (history.ts) goes
// through the same library as Claude's plugin (plugins/jev/src), each tool call and output being
// one message, and what Jev keeps becomes the history of a new thread (`thread/inject_items`).
//
// Codex adds its context (developer instructions, AGENTS.md, environment) to a thread's first
// turn: the old thread's copy is left out, so the new thread gets the current one, once.

import { compact, reductionRatio } from "../../../plugins/jev/src/compact.ts";
import { JevClient } from "../../../plugins/jev/src/client.ts";
import type { CompactResult, JevAsker, Message } from "../../../plugins/jev/src/types.ts";
import type { JsonValue } from "../../protocol.ts";
import type { ResponseItem } from "./history.ts";

/** Below this share of the conversation removed, a new thread is not worth it. */
export const MIN_REDUCTION = 0.25;
/** Characters per token, to estimate what a compaction removed before Codex measures it. */
const CHARS_PER_TOKEN = 4;

/** The blocks of the user messages Codex writes itself at the start of a thread, whole. */
const CONTEXT_BLOCKS = [
  /^# AGENTS\.md instructions[^\n]*\n\s*<INSTRUCTIONS>[\s\S]*<\/INSTRUCTIONS>$/,
  /^<environment_context>[\s\S]*<\/environment_context>$/,
  /^<user_instructions>[\s\S]*<\/user_instructions>$/,
];

export type { JevAsker };

export interface CodexCompaction {
  items: ResponseItem[];
  result: CompactResult;
}

/** Asks Jev with the user's key; `signal` cancels its requests (an interrupt). */
export function jevAsker(apiKey: string, signal: AbortSignal): JevAsker {
  const abortable = (input: Parameters<typeof fetch>[0], init?: RequestInit) => fetch(input, { ...init, signal });
  return new JevClient({ apiKey, fetch: Object.assign(abortable, { preconnect: fetch.preconnect }) });
}

/**
 * What Jev keeps of a thread's history, as the items of a new thread. Throws when Jev fails or
 * would remove less than MIN_REDUCTION of it.
 */
export async function compactCodexHistory(history: readonly ResponseItem[], asker: JevAsker): Promise<CodexCompaction> {
  const entries = history.filter((item) => !isThreadContext(item)).map((item) => ({ item, message: messageOf(item) }));
  const messages = entries.flatMap((entry) => (entry.message ? [entry.message] : []));
  const result = await compact(messages, asker);
  const survived = new Set(result.messages);
  const original = new Set(messages);
  // A dropped output comes back as a new message holding its abridged text.
  const abridged = new Map<string, string>();
  for (const message of result.messages) {
    if (original.has(message)) continue;
    for (const output of message.toolResults ?? []) abridged.set(output.tool_use_id, output.text);
  }
  const items = entries.flatMap(({ item, message }) => {
    if (!message || survived.has(message)) return [item];
    const text = isOutput(item) ? abridged.get(callId(item)) : undefined;
    return text === undefined ? [] : [{ ...item, output: text }];
  });
  const ratio = reductionRatio(result);
  if (ratio < MIN_REDUCTION) throw new Error(`it would remove ${Math.round(ratio * 100)}% of the conversation only`);
  return { items, result };
}

/** One line for the chat: how much went, and how. */
export function compactionSummary(result: CompactResult): string {
  const { stats } = result;
  const parts = [
    `${stats.kept + stats.pinned} kept`,
    stats.resultsDropped > 0 ? `${stats.resultsDropped} outputs cut` : "",
    stats.callsDropped > 0 ? `${stats.callsDropped} removed` : "",
  ].filter(Boolean);
  return `Jev compacted the conversation, ${Math.round(reductionRatio(result) * 100)}% smaller: of ${stats.calls} tool calls, ${parts.join(", ")}`;
}

/** About how many tokens of context the compaction removed. */
export function removedTokens(result: CompactResult): number {
  return Math.max(0, result.stats.charsBefore - result.stats.charsAfter) / CHARS_PER_TOKEN;
}

export function isThreadContext(item: ResponseItem): boolean {
  if (item.type !== "message") return false;
  if (item.role === "developer" || item.role === "system") return true;
  if (item.role !== "user") return false;
  const texts = contentTexts(item.content);
  return texts.length > 0 && texts.every((text) => CONTEXT_BLOCKS.some((block) => block.test(text.trim())));
}

/** The item as Jev reads it; none for what Jev does not judge (reasoning, a compaction). */
function messageOf(item: ResponseItem): Message | undefined {
  if (item.type === "message" && (item.role === "user" || item.role === "assistant")) {
    return { role: item.role, text: contentTexts(item.content).join("\n"), toolUses: [] };
  }
  if (typeof item.call_id !== "string") return undefined;
  if (isOutput(item)) {
    return { role: "user", text: "", toolUses: [], toolResults: [{ tool_use_id: item.call_id, text: outputText(item.output) }] };
  }
  const tool = typeof item.name === "string" ? item.name : item.type;
  return { role: "assistant", text: "", toolUses: [{ tool_use_id: item.call_id, tool, input: inputOf(item) }] };
}

function isOutput(item: ResponseItem): boolean {
  return item.type.endsWith("_output");
}

function callId(item: ResponseItem): string {
  return typeof item.call_id === "string" ? item.call_id : "";
}

function inputOf(item: ResponseItem): Record<string, unknown> {
  if (typeof item.arguments === "string") {
    try {
      const parsed: unknown = JSON.parse(item.arguments);
      if (typeof parsed === "object" && parsed !== null && !Array.isArray(parsed)) return parsed as Record<string, unknown>;
    } catch {
      // Not JSON: passed on as written.
    }
    return { arguments: item.arguments };
  }
  if (item.input !== undefined) return { input: item.input };
  if (item.action !== undefined) return { action: item.action };
  return {};
}

function outputText(output: JsonValue | undefined): string {
  if (typeof output === "string") return output;
  if (Array.isArray(output)) return contentTexts(output).join("\n");
  if (typeof output === "object" && output !== null && typeof output.content === "string") return output.content;
  return JSON.stringify(output ?? null);
}

function contentTexts(content: JsonValue | undefined): string[] {
  if (typeof content === "string") return [content];
  if (!Array.isArray(content)) return [];
  return content.flatMap((part) => (typeof part === "object" && part !== null && !Array.isArray(part) && typeof part.text === "string" ? [part.text] : []));
}
