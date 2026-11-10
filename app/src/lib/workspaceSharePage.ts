// The guest page of a workspace share (story 53), the parts with no React in it: the share named
// in the page's address, its socket, what a chat's status says to a guest, and the page's state as
// the service's messages change it. A chat's events are read when the guest opens it; those that
// arrive meanwhile wait, then go on top (each chat's view skips what it has seen). The comments
// that mention the reader stay unread until their thread shows.

import type { ChatStatus, EventEnvelope, ShareComment, SharedChat, SharedNote, ToGuest, WorkspaceShareSnapshot } from "@chauffeur/protocol";
import { buildChatView, type ChatView, reduceChat } from "./chat";
import { newMentions, threadOf } from "./mentions";

/** Which page an address is: a design's share (`/s/<id>`) or a workspace's (`/w/<id>`). */
export function sharePageOf(pathname: string): { kind: "design" | "workspace"; id: string } | null {
  const match = /^\/([sw])\/([0-9a-f]{8,64})\/?$/i.exec(pathname);
  if (!match) return null;
  return { kind: match[1].toLowerCase() === "w" ? "workspace" : "design", id: match[2].toLowerCase() };
}

/** The guest socket of a workspace share, on the page's own host. */
export function guestSocketUrl(location: { protocol: string; host: string }, id: string): string {
  return `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/v1/workspace-shares/${id}/guest`;
}

/** A chat's status as a guest reads it: the owner answers what the agent asks. */
export function guestChatStatus(status: ChatStatus, owner: string): { label: string; tone: "working" | "attention" | "danger" | "idle" } {
  switch (status) {
    case "working":
      return { label: "Working", tone: "working" };
    case "starting":
      return { label: "Starting", tone: "working" };
    case "needs_input":
      return { label: `Waiting for ${owner}`, tone: "attention" };
    case "error":
      return { label: "Failed", tone: "danger" };
    case "idle":
    case "closed":
      return { label: "Idle", tone: "idle" };
  }
}

export function chatTitle(chat: Pick<SharedChat, "title" | "kind">): string {
  return chat.title?.trim() || (chat.kind === "design" ? "Design" : "Chat");
}

export type GuestState = {
  snapshot: WorkspaceShareSnapshot;
  /** The chats read, by id. */
  views: Record<string, ChatView>;
  /** The chats being read, with the events that came meanwhile; `generation` tells a read made stale by `history`. */
  loading: Record<string, { generation: number; held: EventEnvelope[] }>;
  generations: Record<string, number>;
};

export function guestState(snapshot: WorkspaceShareSnapshot): GuestState {
  return { snapshot, views: {}, loading: {}, generations: {} };
}

/** A chat's events are being read: what comes meanwhile is held. Returns the read's generation. */
export function startLoading(state: GuestState, chat: string): { state: GuestState; generation: number } {
  const generation = state.generations[chat] ?? 0;
  return { state: { ...state, loading: { ...state.loading, [chat]: { generation, held: [] } } }, generation };
}

/** A chat's events, read: its view, with what came meanwhile on top. A stale read changes nothing. */
export function loaded(state: GuestState, chat: string, generation: number, envelopes: EventEnvelope[]): GuestState {
  const pending = state.loading[chat];
  if (!pending || pending.generation !== generation) return state;
  const loading = { ...state.loading };
  delete loading[chat];
  return { ...state, loading, views: { ...state.views, [chat]: pending.held.reduce(reduceChat, buildChatView(envelopes)) } };
}

/** A chat's read failed: it is read again when shown. */
export function loadFailed(state: GuestState, chat: string): GuestState {
  if (!state.loading[chat]) return state;
  const loading = { ...state.loading };
  delete loading[chat];
  return { ...state, loading };
}

function forget(state: GuestState, chat: string): GuestState {
  const views = { ...state.views };
  const loading = { ...state.loading };
  delete views[chat];
  delete loading[chat];
  return { ...state, views, loading, generations: { ...state.generations, [chat]: (state.generations[chat] ?? 0) + 1 } };
}

function mergeNotes(notes: SharedNote[], changed: SharedNote[], removed: string[]): SharedNote[] {
  const kept = notes.filter((n) => !removed.includes(n.path) && !changed.some((c) => c.path === n.path));
  return [...kept, ...changed].sort((a, b) => a.path.localeCompare(b.path));
}

/** The page's state after one message of the service's; `error` and `refused` are the page's to say. */
export function reduceGuest(state: GuestState, message: ToGuest): GuestState {
  const snapshot = state.snapshot;
  switch (message.type) {
    case "snapshot":
      // A reconnection: events may have been missed, every chat is read again when shown.
      return { snapshot: message.snapshot, views: {}, loading: {}, generations: bump(state.generations, [...Object.keys(state.views), ...Object.keys(state.loading)]) };
    case "chats": {
      const ids = new Set(message.chats.map((c) => c.id));
      const gone = [...Object.keys(state.views), ...Object.keys(state.loading)].filter((id) => !ids.has(id));
      const next = gone.reduce(forget, state);
      return { ...next, snapshot: { ...snapshot, chats: message.chats } };
    }
    case "history":
      return forget(state, message.chat);
    case "events": {
      const view = state.views[message.chat];
      if (view) return { ...state, views: { ...state.views, [message.chat]: message.events.reduce(reduceChat, view) } };
      const pending = state.loading[message.chat];
      if (pending) {
        return { ...state, loading: { ...state.loading, [message.chat]: { ...pending, held: [...pending.held, ...message.events] } } };
      }
      return state;
    }
    case "notes":
      return { ...state, snapshot: { ...snapshot, notes: mergeNotes(snapshot.notes, message.changed, message.removed) } };
    case "suggestions":
      return { ...state, snapshot: { ...snapshot, suggestions: message.suggestions } };
    case "comments":
      return { ...state, snapshot: { ...snapshot, comments: message.comments, unread_mentions: unreadAfter(snapshot, message.comments) } };
    case "people":
      return { ...state, snapshot: { ...snapshot, people: message.people } };
    case "viewers":
      return { ...state, snapshot: { ...snapshot, viewers: message.viewers } };
    case "live":
      return { ...state, snapshot: { ...snapshot, online: message.online, last_push_at: message.last_push_at ?? snapshot.last_push_at } };
    case "error":
    case "refused":
      return state;
  }
}

/** The reader's unread mentions once the comments are these: those still there, and the new ones. */
function unreadAfter(snapshot: WorkspaceShareSnapshot, comments: ShareComment[]): string[] {
  const kept = snapshot.unread_mentions.filter((id) => comments.some((c) => c.id === id));
  const fresh = newMentions(snapshot.comments, comments, snapshot.me.email).map((c) => c.id);
  return [...kept, ...fresh.filter((id) => !kept.includes(id))];
}

/** The reader opened a thread: its mentions of them are read. */
export function seeThread(state: GuestState, thread: string): GuestState {
  const { snapshot } = state;
  const unread = snapshot.unread_mentions.filter((id) => {
    const comment = snapshot.comments.find((c) => c.id === id);
    return !comment || threadOf(comment) !== thread;
  });
  return { ...state, snapshot: { ...snapshot, unread_mentions: unread } };
}

/** The threads holding the reader's unread mentions, by chat. */
export function unreadByChat(snapshot: WorkspaceShareSnapshot): Map<string, string[]> {
  const byChat = new Map<string, string[]>();
  for (const comment of snapshot.comments) {
    if (!snapshot.unread_mentions.includes(comment.id)) continue;
    const threads = byChat.get(comment.chat) ?? [];
    const thread = threadOf(comment);
    if (!threads.includes(thread)) byChat.set(comment.chat, [...threads, thread]);
  }
  return byChat;
}

function bump(generations: Record<string, number>, chats: string[]): Record<string, number> {
  const next = { ...generations };
  for (const chat of chats) next[chat] = (next[chat] ?? 0) + 1;
  return next;
}
