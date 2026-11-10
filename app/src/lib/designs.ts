// What the Office's Design window lists (docs/office.md): the user's design chats and the
// workspaces a new one can start in; and the designs a new workspace can start with.

import type { ChatInfo, SharedDesign, WorkspaceDesign, WorkspaceInfo } from "@chauffeur/protocol";
import { workspaceName } from "./format";
import { sharedChangedAt, sharedSections } from "./share";

/** The workspaces a design chat can live in: those ready, or being created. */
export function designWorkspaces(workspaces: WorkspaceInfo[]): WorkspaceInfo[] {
  return workspaces.filter((w) => w.state === "ready" || w.state === "creating");
}

/** The open design chats of those workspaces, newest first, each with its workspace. */
export function designsOf(chats: ChatInfo[], workspaces: WorkspaceInfo[]): { chat: ChatInfo; workspace: WorkspaceInfo }[] {
  const live = new Map(designWorkspaces(workspaces).map((w) => [w.id, w]));
  return chats
    .filter((c) => c.kind === "design" && !c.closed)
    .flatMap((chat) => {
      const workspace = live.get(chat.workspace_id);
      return workspace ? [{ chat, workspace }] : [];
    })
    .sort((a, b) => b.chat.created_at.localeCompare(a.chat.created_at));
}

/** A design the create dialog offers: what `workspace.create` takes, and what the list shows. */
export type DesignOption = {
  key: string;
  design: WorkspaceDesign;
  title: string;
  /** The workspace of one of the user's, the author of a colleague's. */
  detail: string;
  changedAt: string;
};

/** The create dialog's design sections: the user's own, then those shared with them, then the team's. */
export function designOptions(
  own: { chat: ChatInfo; workspace: WorkspaceInfo }[],
  shared: readonly SharedDesign[],
): { yours: DesignOption[]; named: DesignOption[]; team: DesignOption[] } {
  const ofShare = (design: SharedDesign): DesignOption => ({
    key: `share:${design.id}`,
    design: { kind: "share", url: design.url },
    title: design.title,
    detail: design.owner.name,
    changedAt: sharedChangedAt(design),
  });
  const sections = sharedSections(shared);
  return {
    yours: own.map(({ chat, workspace }) => ({
      key: `chat:${chat.id}`,
      design: { kind: "chat", chat: chat.id },
      title: chat.title ?? "Design",
      detail: workspaceName(workspace),
      changedAt: chat.created_at,
    })),
    named: sections.named.map(ofShare),
    team: sections.team.map(ofShare),
  };
}
