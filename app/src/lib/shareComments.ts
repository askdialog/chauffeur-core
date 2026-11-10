// The comments on a passage of an agent's message in a shared workspace (story 53), by its guests
// who can suggest and by its owner, and the replies that make them threads: where each thread
// shows in the transcript, the passage a selection quotes, and what the owner is told of a new
// one. The agent never reads them.

import type { ShareComment } from "@chauffeur/protocol";
import type { ChatItem } from "./chat";

/** The passage a comment quotes, and its text, at most (the share service refuses longer). */
export const QUOTE_MAX = 2000;
export const COMMENT_MAX = 5000;

/** Letters and digits only, lowercase: a passage selected in the rendered markdown is found in its source. */
function bare(text: string): string {
  return text.toLowerCase().replace(/[^\p{L}\p{N}]+/gu, "");
}

/** A comment that opens a thread, and its replies, oldest first. */
export type ShareThread = { comment: ShareComment; replies: ShareComment[] };

/**
 * Where the threads show, by the key of the agent's message they are on, oldest first: among the
 * blocks of one message, under the one holding the passage, else its last. A thread on a message
 * the view does not hold is left out, and so is a reply whose thread is gone.
 */
export function placeComments(items: readonly ChatItem[], comments: readonly ShareComment[]): Map<string, ShareThread[]> {
  const placed = new Map<string, ShareThread[]>();
  const sorted = [...comments].sort((a, b) => a.at - b.at);
  for (const comment of sorted.filter((c) => !c.reply_to)) {
    const blocks = items.filter((item) => item.kind === "assistant" && item.messageId === comment.message);
    const quote = bare(comment.quote);
    const block = blocks.find((item) => item.kind === "assistant" && bare(item.text).includes(quote)) ?? blocks.at(-1);
    const thread = { comment, replies: sorted.filter((r) => r.reply_to === comment.id) };
    if (block) placed.set(block.key, [...(placed.get(block.key) ?? []), thread]);
  }
  return placed;
}

/** The key of the block a thread shows under, when the view holds it. */
export function blockOf(placed: ReadonlyMap<string, readonly ShareThread[]>, thread: string): string | null {
  for (const [key, threads] of placed) if (threads.some((t) => t.comment.id === thread)) return key;
  return null;
}

/** The passage a guest selected, as their comment quotes it: whitespace collapsed, cut to `QUOTE_MAX`. */
export function quoteOf(selected: string): string {
  const text = selected.replace(/\s+/g, " ").trim();
  return text.length > QUOTE_MAX ? `${text.slice(0, QUOTE_MAX - 1)}…` : text;
}

/**
 * The comments not in the list before, but the reader's own and those that mention them (the
 * mention's own notice tells those): the ones to tell the owner of.
 */
export function newComments(before: readonly ShareComment[] | undefined, after: readonly ShareComment[], me?: string | null): ShareComment[] {
  const known = new Set((before ?? []).map((c) => c.id));
  return after.filter((c) => !known.has(c.id) && c.author.email !== me && !(me && c.mentions?.some((m) => m.email === me)));
}

/** What the owner is told of a new comment, or of a reply. */
export function commentNotice(comment: ShareComment, chatTitle: string | null): string {
  if (comment.reply_to) return `${comment.author.name} replies in ${chatTitle ?? "a chat"}: ${comment.text}`;
  const quote = comment.quote.length > 80 ? `${comment.quote.slice(0, 79)}…` : comment.quote;
  return `${comment.author.name} comments in ${chatTitle ?? "a chat"} on “${quote}”: ${comment.text}`;
}
