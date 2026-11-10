// Merges a chat's consecutive streamed deltas of one message into one event: a model streams a
// few characters per delta, and each became a line for the daemon to parse and fan out. A held
// delta goes out before any other event of its chat, so the daemon sees the same text in the same
// order, only in fewer events.

import type { ChatEvent } from "./protocol.ts";

type Delta = Extract<ChatEvent, { type: "text_delta" | "thinking_delta" }>;

/** How long a delta waits for the next ones of its message, short enough to read as live. */
export const DELTA_WINDOW_MS = 40;

export class DeltaCoalescer {
  private readonly held = new Map<string, { delta: Delta; texts: string[]; timer: ReturnType<typeof setTimeout> }>();

  constructor(
    private readonly emit: (chatId: string, event: ChatEvent) => void,
    private readonly windowMs = DELTA_WINDOW_MS,
  ) {}

  push(chatId: string, event: ChatEvent): void {
    const held = this.held.get(chatId);
    if (held && isDelta(event) && sameMessage(held.delta, event)) {
      held.texts.push(event.text);
      return;
    }
    this.flush(chatId);
    if (!isDelta(event)) {
      this.emit(chatId, event);
      return;
    }
    const timer = setTimeout(() => this.flush(chatId), this.windowMs);
    this.held.set(chatId, { delta: event, texts: [event.text], timer });
  }

  /** Sends what the chat holds, if anything: before the chat ends or reaches the daemon otherwise. */
  flush(chatId: string): void {
    const held = this.held.get(chatId);
    if (!held) return;
    this.held.delete(chatId);
    clearTimeout(held.timer);
    this.emit(chatId, { ...held.delta, text: held.texts.join("") });
  }
}

function isDelta(event: ChatEvent): event is Delta {
  return event.type === "text_delta" || event.type === "thinking_delta";
}

function sameMessage(a: Delta, b: Delta): boolean {
  return a.type === b.type && a.message_id === b.message_id && a.parent_tool_use_id === b.parent_tool_use_id;
}
