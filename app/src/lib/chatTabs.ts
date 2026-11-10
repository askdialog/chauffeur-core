// A workspace's chat tabs: which chats are open, what the right-click menu of a tab allows, which
// tab shows once one leaves, and which closed chat ⇧⌘T brings back.

import type { ChatInfo, WorkspaceState } from "@chauffeur/protocol";
import { movedBefore } from "./tabDrag";

/**
 * A tab's label: its title, else "Chat n" by its place among every chat of the workspace (the
 * closed ones too, so that closing a chat renames no other).
 */
export function chatLabel(chat: ChatInfo, all: ChatInfo[]): string {
  if (chat.title) return chat.title;
  const index = all.findIndex((c) => c.id === chat.id);
  return `Chat ${(index < 0 ? all.length : index) + 1}`;
}

/**
 * The chats shown as tabs, in the order the user dragged them to (`tab_position`); those never
 * placed (new ones) after, oldest first, as `all` comes.
 */
export function openChats(all: ChatInfo[]): ChatInfo[] {
  return tabOrder(all.filter((c) => !c.closed));
}

function tabOrder(chats: ChatInfo[]): ChatInfo[] {
  const place = (c: ChatInfo) => c.tab_position ?? Number.MAX_SAFE_INTEGER;
  return [...chats].sort((a, b) => place(a) - place(b));
}

/**
 * Every chat of a workspace with the tab place it has once `id` moved before `before` (last
 * without it), as the daemon numbers them for `chat.move`: shown before its answer comes.
 */
export function placedChats(all: ChatInfo[], id: string, before: string | undefined): ChatInfo[] {
  const order = movedBefore(tabOrder(all).map((c) => c.id), id, before);
  return all.map((c) => ({ ...c, tab_position: order.indexOf(c.id) }));
}

/** The tab a workspace shows: the one selected while open, else the newest open one. */
export function shownChat(open: ChatInfo[], selected: string | undefined): ChatInfo | null {
  const newest = open.reduce<ChatInfo | null>((last, c) => (last && last.created_at > c.created_at ? last : c), null);
  return open.find((c) => c.id === selected) ?? newest;
}

/** The chats the user closed, the most recently active first. */
export function closedChats(all: ChatInfo[]): ChatInfo[] {
  const at = (c: ChatInfo) => c.last_activity_at ?? c.created_at;
  return all.filter((c) => c.closed).sort((a, b) => at(b).localeCompare(at(a)));
}

/**
 * What a chat's tab shows before its label. Tasks its agent still runs after its turn (a background
 * subagent, a command) show it working, as `agentBusy` counts them; a request waits for the user
 * even then.
 */
export function chatTabStatus(chat: ChatInfo): "working" | "needs_input" | "error" | "idle" {
  if (chat.status === "needs_input") return "needs_input";
  if (chat.status === "working" || chat.status === "starting" || chat.running_tasks > 0) return "working";
  if (chat.status === "error") return "error";
  return "idle";
}

/** What the Chat view's tab shows of its open chats: one waiting for the user first, else one working. */
export function chatsIndicator(open: ChatInfo[]): "attention" | "working" | null {
  const statuses = open.map(chatTabStatus);
  if (statuses.includes("needs_input")) return "attention";
  if (statuses.includes("working")) return "working";
  return null;
}

/**
 * Whether closing or deleting the chat would stop its agent's work: a turn, a question or a
 * permission request waiting, background tasks, or messages waiting in its queue.
 */
export function agentBusy(chat: ChatInfo, queued: number): boolean {
  return (
    chat.status === "working" ||
    chat.status === "starting" ||
    chat.status === "needs_input" ||
    chat.running_tasks > 0 ||
    queued > 0
  );
}

export type TabActions = {
  rename: boolean;
  close: boolean;
  delete: boolean;
  /** "Design from this chat": a new chat, as the "+" allows one. */
  design: boolean;
  /** "Fork into a new tab": a new chat with the conversation so far. */
  fork: boolean;
  /** Why the others are not allowed, for their tooltip. */
  reason: string | null;
};

/**
 * What a tab's menu allows in a workspace in this state: an archived one's chats are read-only. A
 * workspace moved to another machine is piloted there through this daemon while that machine
 * answers (`reachable`); design chats work on this machine's files and stay off.
 */
export function tabActions(state: WorkspaceState, movedTo: string | null = null, reachable = true): TabActions {
  if (movedTo) {
    if (!reachable) {
      return {
        rename: false,
        close: false,
        delete: false,
        design: false,
        fork: false,
        reason: `This workspace runs on ${movedTo}, which does not answer: its chats are read-only until it does`,
      };
    }
    return {
      rename: true,
      close: true,
      delete: true,
      design: false,
      fork: true,
      reason: `This workspace runs on ${movedTo}: design chats are not available here yet`,
    };
  }
  if (state === "ready") return { rename: true, close: true, delete: true, design: true, fork: true, reason: null };
  if (state === "creating") {
    return { rename: true, close: false, delete: false, design: true, fork: false, reason: "The workspace is being created" };
  }
  return {
    rename: false,
    close: false,
    delete: false,
    design: false,
    fork: false,
    reason: "This workspace is archived: its chats are read-only",
  };
}

/**
 * Why a chat cannot be forked now, else null: copied mid-turn, its conversation would end on a
 * step nobody finishes; a design chat's screens are its own.
 */
export function forkRefusal(chat: ChatInfo): string | null {
  if (chat.kind === "design") return "A design chat cannot be forked: its screens are its own";
  if (chat.status === "working" || chat.status === "starting" || chat.status === "needs_input") {
    return "Its agent is working: fork it once its turn ends, fork from a turn it finished, or interrupt it first";
  }
  return null;
}

/**
 * The tab shown once `leaving` is closed or deleted: the one on its right, else on its left; null
 * when it was the only one (the daemon then opens a new chat).
 */
export function neighbourTab(open: ChatInfo[], leaving: string): string | null {
  const index = open.findIndex((c) => c.id === leaving);
  if (index < 0) return null;
  return open[index + 1]?.id ?? open[index - 1]?.id ?? null;
}

/**
 * The closed chat ⇧⌘T reopens: the last one closed from this window, else the most recently
 * active closed one.
 */
export function chatToReopen(closed: ChatInfo[], closedHere: string[]): ChatInfo | null {
  for (let i = closedHere.length - 1; i >= 0; i--) {
    const chat = closed.find((c) => c.id === closedHere[i]);
    if (chat) return chat;
  }
  return closed[0] ?? null;
}

/** The app's chats and selection without a deleted chat. */
export function withoutChat<S extends { chats: Record<string, ChatInfo>; selectedChat: Record<string, string> }>(
  state: S,
  chatId: string,
): Pick<S, "chats" | "selectedChat"> {
  const chat = state.chats[chatId];
  if (!chat) return { chats: state.chats, selectedChat: state.selectedChat };
  const chats = { ...state.chats };
  delete chats[chatId];
  let selectedChat = state.selectedChat;
  if (selectedChat[chat.workspace_id] === chatId) {
    selectedChat = { ...selectedChat };
    delete selectedChat[chat.workspace_id];
  }
  return { chats, selectedChat };
}
