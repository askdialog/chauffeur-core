// A design chat's design brief (story 47): what the composer says about it, and when its text goes
// into the composer's draft. The daemon writes the brief from the source chat and keeps it on the
// chat until its first message; the app only fills the draft, never sends it.

import type { ChatInfo, DesignBrief } from "@chauffeur/protocol";
import { chatLabel } from "./chatTabs";

/** What shows above a design chat's composer while it has a brief. */
export type BriefBanner =
  | { state: "writing"; message: string }
  | { state: "ready"; message: string }
  | { state: "failed"; message: string }
  | null;

/** The source chat's name as its tab shows it, else "the chat" (closed and gone, or not loaded yet). */
export function sourceName(chat: ChatInfo, chats: ChatInfo[]): string {
  const source = chat.source_chat ? chats.find((c) => c.id === chat.source_chat) : undefined;
  return source ? `“${chatLabel(source, chats)}”` : "the chat";
}

export function briefBanner(chat: ChatInfo, chats: ChatInfo[]): BriefBanner {
  const brief = chat.kind === "design" ? chat.brief : undefined;
  if (!brief) return null;
  const source = sourceName(chat, chats);
  if (brief.state === "writing") return { state: "writing", message: `Summarizing ${source}…` };
  if (brief.state === "failed") {
    return { state: "failed", message: `The brief could not be written: ${brief.error?.trim() || "no reason given"}` };
  }
  return { state: "ready", message: `Design brief written from ${source}: edit it, then send.` };
}

/** Per chat: the brief text the composer already took (or let go), and whether a Regenerate asked to replace the draft. */
export type BriefMemo = { seen: string | null; replace: boolean };

/**
 * What the composer does with the brief: "fill" puts its text in the draft, "skip" lets it go
 * without touching the draft (the user wrote before it arrived), null does nothing (not written
 * yet, or already taken). A brief fills an empty draft once, so that emptying the draft by hand
 * does not bring it back; after a Regenerate it replaces the draft whatever is in it.
 */
export function briefFill(brief: DesignBrief | undefined, draft: string, memo: BriefMemo | undefined): "fill" | "skip" | null {
  if (brief?.state !== "ready" || !brief.text) return null;
  if (memo?.replace) return "fill";
  if (memo?.seen === brief.text) return null;
  return draft.trim() === "" ? "fill" : "skip";
}

const memos = new Map<string, BriefMemo>();

/** Kept while the app runs, as a dismissed suggestion is: a chat's composer mounts again on each visit. */
export const briefMemos = {
  get(chatId: string): BriefMemo | undefined {
    return memos.get(chatId);
  },
  /** The composer took this text, or let it go. */
  seen(chatId: string, text: string) {
    memos.set(chatId, { seen: text, replace: false });
  },
  /** A Regenerate was asked: its brief replaces the draft once written. */
  regenerating(chatId: string) {
    memos.set(chatId, { seen: null, replace: true });
  },
};
