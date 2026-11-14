// A child workspace's open requests, shown in its parent's chats too (see
// docs/adr/child-questions-reach-the-parent.md): what `workspace.wait` says the child waits on,
// as the cards the child's own chat shows.

import type { WaitResult, WorkspaceInfo } from "@chauffeur/protocol";
import { type ElicitationItem, emptyChatView, openRequests, type PermissionItem, type QuestionItem, reduceChat } from "./chat";

export type ChildRequest = {
  workspace: WorkspaceInfo;
  chatId: string;
  item: PermissionItem | QuestionItem | ElicitationItem;
};

/** The requests a child's `workspace.wait` lists, oldest first in each chat. */
export function childRequests(result: Pick<WaitResult, "workspace" | "open_requests">): ChildRequest[] {
  return result.open_requests.flatMap((open) => {
    const view = reduceChat(emptyChatView, {
      seq: null,
      ts: "",
      event: { scope: "chat", workspace_id: result.workspace.id, chat_id: open.chat_id, event: open.request },
    });
    return openRequests(view).map((item) => ({ workspace: result.workspace, chatId: open.chat_id, item }));
  });
}

/** The children of `parentId` waiting on the user, oldest first. */
export function childrenNeedingInput(workspaces: Record<string, WorkspaceInfo>, parentId: string): WorkspaceInfo[] {
  return Object.values(workspaces)
    .filter((w) => w.parent_id === parentId && w.state === "ready" && w.activity === "needs_input")
    .sort((a, b) => a.created_at.localeCompare(b.created_at));
}
