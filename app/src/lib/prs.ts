// The workspace's pull requests, as the app offers to merge them, one at a time.

import type { MergeMethod, PrInfo } from "@chauffeur/protocol";

export const MERGE_METHODS: Array<{ id: MergeMethod; label: string; detail: string }> = [
  { id: "squash", label: "Squash and merge", detail: "one commit per PR" },
  { id: "merge", label: "Create a merge commit", detail: "every commit kept" },
  { id: "rebase", label: "Rebase and merge", detail: "the commits replayed" },
];

/** The open PR among `prs` that `pr` is stacked on: the one of its repo whose branch it merges into. */
export function stackedOn(pr: PrInfo, prs: PrInfo[]): PrInfo | undefined {
  return prs.find((other) => other.state === "OPEN" && other.repo === pr.repo && other.branch === pr.pr_base_branch);
}

/** The open PRs among `prs` stacked on `pr`. */
export function stackedAbove(pr: PrInfo, prs: PrInfo[]): PrInfo[] {
  return prs.filter((other) => stackedOn(other, prs) === pr);
}

/** The methods every PR's repo allows (all of them while unknown). */
export function commonMethods(prs: PrInfo[]): MergeMethod[] {
  return MERGE_METHODS.map((m) => m.id).filter((id) => prs.every((pr) => pr.merge_methods.length === 0 || pr.merge_methods.includes(id)));
}

/** Why `pr` cannot be merged on its own now, or null when it can: it is open and not a draft, it is
 * not stacked on another open PR of `prs` (merged first), its checks pass (or it has none), no
 * review blocks it, it has no conflict and the user may merge into the repo. */
export function mergeBlocker(pr: PrInfo, prs: PrInfo[]): string | null {
  if (pr.state !== "OPEN") return "is not open";
  if (pr.is_draft) return "is a draft";
  const below = stackedOn(pr, prs);
  if (below) return `stacked on #${below.number}, to merge first`;
  if (pr.can_merge === false) return "you cannot merge into this repo";
  if (pr.checks === "FAILURE") return "checks failed";
  if (pr.checks === "PENDING") return "checks are running";
  if (pr.review_decision === "CHANGES_REQUESTED") return "changes requested";
  if (pr.review_decision === "REVIEW_REQUIRED") return "review required";
  if (pr.mergeable === "CONFLICTING") return `conflicts with ${pr.pr_base_branch ?? pr.base_branch}`;
  if (pr.mergeable !== "MERGEABLE") return "GitHub is still checking for conflicts";
  return null;
}

/** `web`, or `abc-3214 · web` for a PR of an agent's worktree. */
export function prLabel(pr: PrInfo): string {
  return pr.group ? `${pr.group} · ${pr.repo}` : pr.repo;
}
