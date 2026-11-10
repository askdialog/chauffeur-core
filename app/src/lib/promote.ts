// "Continue as workspace": what keeps it back.

import type { PromoteLeftovers, RepoLeftovers, WorkspaceInfo } from "@chauffeur/protocol";
import { RpcCallError } from "./rpc";

const CONFLICT = 409;

/**
 * The files an exploration's checkouts hold beyond their commit, when the daemon refused to
 * continue for them; null for any other error. The user then decides: discard them, or keep the
 * exploration.
 */
export function promoteLeftovers(error: unknown): RepoLeftovers[] | null {
  if (!(error instanceof RpcCallError) || error.code !== CONFLICT) return null;
  const repos = (error.data as Partial<PromoteLeftovers> | null | undefined)?.repos;
  return Array.isArray(repos) && repos.length > 0 ? repos : null;
}

/** The first files of a repo, and how many more. */
export function leftoverLines(files: string[], shown = 8): { shown: string[]; more: number } {
  return { shown: files.slice(0, shown), more: Math.max(0, files.length - shown) };
}

/**
 * Why an exploration cannot continue as a workspace now, as the daemon would refuse it; null when
 * it can. Files left in its checkouts are not known here: the dialog asks about them.
 */
export function promoteUnavailable(workspace: Pick<WorkspaceInfo, "state" | "activity">): string | null {
  if (workspace.state !== "ready") return "Wait until the exploration is ready";
  if (workspace.activity === "working" || workspace.activity === "needs_input") {
    return "An agent is working: wait for the end of its turn, or stop it";
  }
  return null;
}
