import type { Activity, ChatStatus, WorkspaceInfo } from "@chauffeur/protocol";

export type Attention = "finished" | "failed" | "needs_input";

const ACTIVE: ChatStatus[] = ["working", "starting", "needs_input"];

/**
 * Whether a chat's status change deserves the user's attention (an unread marker, a notification
 * when the window is not focused): its turn ended, it failed, or it waits for an answer.
 * `before` is unknown for a chat never seen; the workspace's activity stands in for it.
 */
export function chatAttention(
  before: ChatStatus | undefined,
  after: ChatStatus,
  workspaceActivity?: Activity,
): Attention | null {
  const previous =
    before ??
    (workspaceActivity === "working" ? "working" : workspaceActivity === "needs_input" ? "needs_input" : undefined);
  if (previous === undefined || previous === after) return null;
  if (after === "needs_input") return "needs_input";
  if (!ACTIVE.includes(previous)) return null;
  if (after === "idle") return "finished";
  if (after === "error") return "failed";
  return null;
}

export function attentionText(attention: Attention, agent: string): string {
  switch (attention) {
    case "needs_input":
      return `${agent} needs your input`;
    case "failed":
      return `${agent} stopped with an error`;
    case "finished":
      return `${agent} finished`;
  }
}

/**
 * How many workspaces wait for the user, for the tray icon: those the sidebar badges "needs input"
 * (a chat asks a question or a permission). A finished turn only marks its workspace unread.
 */
export function needingYou(workspaces: Record<string, WorkspaceInfo>): number {
  return Object.values(workspaces).filter((w) => w.state === "ready" && w.activity === "needs_input").length;
}
