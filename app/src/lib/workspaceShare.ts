// A workspace's share in the app (story 53), kept pure so that it is tested beside it: the guests
// and their roles in the dialog, what the sidebar says of a shared workspace, and the guests'
// suggestions as the owner meets them (pending ones above the composer, new ones told).

import type { ChatInfo, Guest, GuestRole, Suggestion, SuggestionState, WorkspaceShareState } from "@chauffeur/protocol";
import { isShared } from "./share";

/** The e-mails added as viewers, after those named already, once each. */
export function addGuests(guests: readonly Guest[], emails: readonly string[]): Guest[] {
  const out = [...guests];
  for (const email of emails) {
    if (!out.some((g) => g.email === email)) out.push({ email, role: "viewer" });
  }
  return out;
}

export function setRole(guests: readonly Guest[], email: string, role: GuestRole): Guest[] {
  return guests.map((g) => (g.email === email ? { ...g, role } : g));
}

/** Whether two lists name other people, or give someone another role (their order aside). */
export function guestsDiffer(a: readonly Guest[], b: readonly Guest[]): boolean {
  if (a.length !== b.length) return true;
  return a.some((guest) => !b.some((other) => other.email === guest.email && other.role === guest.role));
}

/** The chats a share holds: `null` for all of them, those created later too, or the ids picked. */
export type ChatSelection = string[] | null;

/** The share's selection as the dialog starts from it, its chats closed since left out. */
export function selectionOf(chatIds: readonly string[] | null | undefined, open: readonly string[]): ChatSelection {
  return chatIds ? chatIds.filter((id) => open.includes(id)) : null;
}

/** Whether two selections hold other chats (their order aside). */
export function selectionsDiffer(a: ChatSelection, b: ChatSelection): boolean {
  if (a === null || b === null) return a !== b;
  return a.length !== b.length || a.some((id) => !b.includes(id));
}

/** A chat checked or unchecked, the others kept in their order. */
export function toggleChat(selection: readonly string[], id: string, on: boolean): string[] {
  const rest = selection.filter((other) => other !== id);
  return on ? [...rest, id] : rest;
}

/** The selection as `workspace.share_start` and `workspace.share_update` take it. */
export function selectionParams(selection: ChatSelection): { chat_ids: string[] } | { all_chats: true } {
  return selection === null ? { all_chats: true } : { chat_ids: selection };
}

/** What the sidebar shows of a live share: who is looking, and a sentence for its title. */
export function shareIndicator(state: WorkspaceShareState | undefined): { viewers: number; title: string } | null {
  if (!isShared(state) || !state?.share) return null;
  const { guests, team, viewers } = state.share;
  const suggest = guests.filter((g) => g.role === "suggest").length;
  const named = guests.length === 0 ? (team ? "the team" : "nobody yet") : `${guests.length} ${guests.length === 1 ? "colleague" : "colleagues"}${team ? " and the team" : ""}`;
  const looking = viewers.length === 0 ? "nobody looking now" : viewers.length === 1 ? `${viewers[0]} is looking` : `${viewers.length} looking`;
  const suggesting = suggest > 0 ? `, ${suggest} can suggest` : "";
  return { viewers: viewers.length, title: `Shared with ${named}${suggesting}: ${looking}` };
}

/** The suggestions waiting for the owner, oldest first. */
export function pendingSuggestions(suggestions: readonly Suggestion[] | undefined): Suggestion[] {
  return (suggestions ?? []).filter((s) => s.state === "pending").sort((a, b) => a.at - b.at);
}

/** The pending suggestions not in the list before: the ones to tell the owner of. */
export function newPending(before: readonly Suggestion[] | undefined, after: readonly Suggestion[]): Suggestion[] {
  const known = new Set((before ?? []).map((s) => s.id));
  return pendingSuggestions(after).filter((s) => !known.has(s.id));
}

const NO_CHAT_IDS: readonly string[] = [];
let indexed: { chats: Readonly<Record<string, ChatInfo>>; byWorkspace: Map<string, string[]> } | null = null;

/**
 * The ids of a workspace's chats, for its row's badge; an empty list when it has none. Every row
 * asks at each change of the store: the chats are indexed once per version of the chats map.
 */
export function chatIdsOf(chats: Readonly<Record<string, ChatInfo>>, workspaceId: string): readonly string[] {
  if (indexed?.chats !== chats) {
    const byWorkspace = new Map<string, string[]>();
    for (const chat of Object.values(chats)) {
      const ids = byWorkspace.get(chat.workspace_id);
      if (ids) ids.push(chat.id);
      else byWorkspace.set(chat.workspace_id, [chat.id]);
    }
    indexed = { chats, byWorkspace };
  }
  return indexed.byWorkspace.get(workspaceId) ?? NO_CHAT_IDS;
}

/** How many suggestions wait among some chats' (a workspace's), for its row's badge. */
export function pendingCount(byChat: Readonly<Record<string, readonly Suggestion[]>>, chatIds: readonly string[]): number {
  return chatIds.reduce((count, id) => count + pendingSuggestions(byChat[id]).length, 0);
}

/** A suggestion's fate, in a word, as the guests' page and the owner's cards say it. */
export function suggestionStateLabel(state: SuggestionState): string {
  switch (state) {
    case "pending":
      return "waiting";
    case "sent":
      return "sent";
    case "sent_edited":
      return "sent, edited";
    case "dismissed":
      return "dismissed";
    case "deleted":
      return "deleted";
  }
}

/** Whether a guest may take a suggestion back: theirs, waiting or dismissed (a sent one is in the chat). */
export function canDeleteSuggestion(suggestion: Suggestion, email: string): boolean {
  return suggestion.author.email === email && (suggestion.state === "pending" || suggestion.state === "dismissed");
}

/** What the owner is told of a new suggestion. */
export function suggestionNotice(suggestion: Suggestion, chatTitle: string | null): string {
  return `${suggestion.author.name} suggests to ${chatTitle ?? "a chat"}: ${suggestion.text}`;
}
