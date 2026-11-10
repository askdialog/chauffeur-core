// Workspace shares (story 53) in the app: each workspace's share as the daemon tells it, and the
// suggestions and comments of its guests by chat. Every workspace is followed live (its `share`,
// `suggestions` and `share_comments` events, which registry subscriptions get) whether it is open
// or not, so that a new suggestion or comment is told (and a suggestion badged) wherever the user
// is; which workspaces are shared is asked once per connection. The owner comments and replies
// too, mentioning the people the share is open to, or someone they invite at once.

import type { ChatInfo, Mention, ShareComment, ShareCommentAddParams, Suggestion, WorkspaceShareState } from "@chauffeur/protocol";
import { create } from "zustand";
import { appendDraft } from "@/lib/designHandoff";
import { drafts } from "@/lib/drafts";
import { notify, windowFocused } from "@/lib/native";
import type { Subscription } from "@/lib/rpc";
import { errorMessage } from "@/lib/rpc";
import { isShared } from "@/lib/share";
import { commentNotice, newComments } from "@/lib/shareComments";
import { newPending, suggestionNotice } from "@/lib/workspaceShare";
import { client, toast, useApp } from "./app";

type SharesState = {
  /** By workspace, as the daemon last told it. */
  shares: Record<string, WorkspaceShareState>;
  /** By chat, the decided ones too; absent until read. */
  suggestions: Record<string, Suggestion[]>;
  /** The guests' comments on the agents' messages, by chat; absent until read. */
  comments: Record<string, ShareComment[]>;
  /** The suggestion a chat's composer holds after Edit, by chat: its next message sends it. */
  editing: Record<string, string>;
};

export const useWorkspaceShares = create<SharesState>(() => ({ shares: {}, suggestions: {}, comments: {}, editing: {} }));

const set = useWorkspaceShares.setState;
const get = useWorkspaceShares.getState;

export function setWorkspaceShare(state: WorkspaceShareState) {
  set((s) => ({ shares: { ...s.shares, [state.workspace]: state } }));
  follow();
}

/** When the app started: a chat's suggestions told before it was read are new only if made since. */
const startedAt = Date.now();

/**
 * A chat's suggestions, read (`tell` false) or told by the daemon: a pending one not known before
 * is told to the user, a toast while they look at its workspace, a notification otherwise.
 */
export function setSuggestions(chatId: string, suggestions: Suggestion[], tell: boolean) {
  const before = get().suggestions[chatId];
  set((s) => ({ suggestions: { ...s.suggestions, [chatId]: suggestions } }));
  const editing = get().editing[chatId];
  if (editing && !suggestions.some((s) => s.id === editing && s.state === "pending")) forgetEditing(chatId);
  if (!tell) return;
  const title = useApp.getState().chats[chatId]?.title ?? null;
  const fresh = newPending(before, suggestions).filter((s) => before || s.at >= startedAt);
  for (const suggestion of fresh) announce(chatId, suggestionNotice(suggestion, title));
}

/** A toast while the user looks at the chat's workspace, a notification otherwise. */
function announce(chatId: string, what: string) {
  const app = useApp.getState();
  const chat = app.chats[chatId];
  if (windowFocused() && chat && app.selectedId === chat.workspace_id) toast(what);
  else void notify(chat ? (app.workspaces[chat.workspace_id]?.slug ?? "Chauffeur") : "Chauffeur", what);
}

/** The share's owner: the user, as the share names them. */
function ownerOf(chatId: string): string | null {
  const app = useApp.getState();
  const workspace = app.chats[chatId]?.workspace_id;
  return (workspace ? get().shares[workspace]?.share?.owner.email : undefined) ?? app.identity?.email ?? null;
}

/**
 * A chat's comments, read (`tell` false) or told by the daemon: a new one is told to the user, but
 * their own, and one that mentions them (the mention's notice tells it).
 */
export function setComments(chatId: string, comments: ShareComment[], tell: boolean) {
  const before = get().comments[chatId];
  set((s) => ({ comments: { ...s.comments, [chatId]: comments } }));
  if (!tell) return;
  const title = useApp.getState().chats[chatId]?.title ?? null;
  const fresh = newComments(before, comments, ownerOf(chatId)).filter((c) => before || c.at >= startedAt);
  for (const comment of fresh) announce(chatId, commentNotice(comment, title));
}

/** The owner comments on a passage, or replies: true once the share service took it. */
export async function addShareComment(params: ShareCommentAddParams): Promise<boolean> {
  try {
    const result = await client.call("chat.share_comment_add", params);
    setComments(params.chat, result.comments, false);
    return true;
  } catch (error) {
    toast(`Could not ${params.reply_to ? "reply" : "comment"}: ${errorMessage(error)}`, "error");
    return false;
  }
}

export async function deleteShareComment(chatId: string, comment: ShareComment) {
  try {
    const result = await client.call("chat.share_comment_delete", { chat: chatId, id: comment.id });
    setComments(chatId, result.comments, false);
  } catch (error) {
    toast(`Could not delete the comment: ${errorMessage(error)}`, "error");
  }
}

/**
 * "Invite and mention": the addresses mentioned that the share is not open to become its viewers
 * (reading is enough to be mentioned) before the comment names them; false when that failed.
 */
export async function inviteMentioned(workspaceId: string, mentions: readonly Mention[], teamDomain: string | null): Promise<boolean> {
  const share = get().shares[workspaceId]?.share;
  if (!share) return true;
  const newcomers = mentions
    .map((m) => m.email)
    .filter((email) => email !== share.owner.email && !share.guests.some((g) => g.email === email) && !(teamDomain && email.endsWith(`@${teamDomain}`)));
  if (newcomers.length === 0) return true;
  try {
    const guests = [...share.guests, ...newcomers.map((email) => ({ email, role: "viewer" as const }))];
    setWorkspaceShare(await client.call("workspace.share_update", { workspace: workspaceId, guests, team: share.team }));
    toast(`Invited ${newcomers.join(", ")}`);
    return true;
  } catch (error) {
    toast(`Could not invite ${newcomers.join(", ")}: ${errorMessage(error)}`, "error");
    return false;
  }
}

export async function loadComments(chatId: string) {
  try {
    const result = await client.call("chat.share_comments", { chat: chatId });
    setComments(chatId, result.comments, false);
  } catch (error) {
    // A daemon without comments, or a chat gone: nothing to show under its messages.
    console.warn("the chat's comments", errorMessage(error));
  }
}

export async function loadSuggestions(chatId: string) {
  try {
    const result = await client.call("chat.suggestions", { chat: chatId });
    setSuggestions(chatId, result.suggestions, false);
  } catch (error) {
    // A daemon without workspace shares, or a chat gone: no card to show.
    console.warn("the chat's suggestions", errorMessage(error));
  }
}

/** Send: the suggestion's text goes to the agent as the user's own message. */
export async function sendSuggestion(chat: ChatInfo, suggestion: Suggestion): Promise<boolean> {
  try {
    await client.call("chat.send", { chat: chat.id, text: suggestion.text, suggestion: suggestion.id });
    return true;
  } catch (error) {
    toast(`Could not send ${suggestion.author.name}'s suggestion: ${errorMessage(error)}`, "error");
    return false;
  }
}

/** Edit: the text goes below the composer's draft, and the next message sent carries it. */
export function editSuggestion(chatId: string, suggestion: Suggestion) {
  drafts.setText(chatId, appendDraft(drafts.text(chatId), suggestion.text));
  set((s) => ({ editing: { ...s.editing, [chatId]: suggestion.id } }));
}

export function forgetEditing(chatId: string) {
  if (!get().editing[chatId]) return;
  set((s) => {
    const editing = { ...s.editing };
    delete editing[chatId];
    return { editing };
  });
}

export async function dismissSuggestion(chatId: string, suggestion: Suggestion) {
  try {
    const result = await client.call("chat.suggestion_dismiss", { chat: chatId, id: suggestion.id });
    setSuggestions(chatId, result.suggestions, false);
  } catch (error) {
    toast(`Could not dismiss ${suggestion.author.name}'s suggestion: ${errorMessage(error)}`, "error");
  }
}

// --- following the shared workspaces ---

let following: Subscription | null = null;

/**
 * Every workspace's share and suggestions events, open or not: the daemon gives them to registry
 * subscriptions, without the rest of a workspace's live events.
 */
function follow() {
  following ??= client.subscribe({ all: false, registry: true, workspaces: [], chats: [], ephemeral: false }, (envelope) => {
    const event = envelope.event;
    if (event.scope === "workspace" && event.event.type === "share") setWorkspaceShare(event.event.state);
    if (event.scope === "chat" && event.event.type === "suggestions") setSuggestions(event.chat_id, event.event.suggestions, true);
    if (event.scope === "chat" && event.event.type === "share_comments") setComments(event.chat_id, event.event.comments, true);
  });
}

/** Which workspaces are shared, asked of the daemon at each connection. */
async function probe() {
  const workspaces = Object.values(useApp.getState().workspaces).filter((w) => w.state === "ready");
  const states = await Promise.all(
    workspaces.map((w) => client.call("workspace.share_state", { workspace: w.id }).catch(() => null)),
  );
  const shares: Record<string, WorkspaceShareState> = { ...get().shares };
  for (const state of states) if (state) shares[state.workspace] = state;
  set({ shares });
  follow();
  // The suggestions waiting in shared workspaces badge them from the start.
  const chats = Object.values(useApp.getState().chats);
  for (const state of Object.values(shares).filter(isShared)) {
    for (const chat of chats.filter((c) => c.workspace_id === state.workspace && !c.closed)) void loadSuggestions(chat.id);
  }
}

let probed = false;
useApp.subscribe((s) => {
  if (s.connection.status !== "open") {
    probed = false;
    return;
  }
  if (probed || !s.workspacesLoaded) return;
  probed = true;
  void probe();
});
