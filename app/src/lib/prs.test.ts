import { describe, expect, test } from "bun:test";
import type { PrInfo } from "@chauffeur/protocol";
import { commonMethods, mergeBlocker, stackedAbove, stackedOn } from "./prs";

const pr = (repo: string, state: string, methods: PrInfo["merge_methods"] = []): PrInfo => ({
  repo,
  group: null,
  branch: `${repo}-branch`,
  base_branch: "main",
  ahead: 1,
  dirty: false,
  number: 1n,
  pr_base_branch: "main",
  url: null,
  state,
  title: null,
  is_draft: false,
  checks: null,
  review_decision: null,
  mergeable: null,
  merge_methods: methods,
  can_merge: null,
});

describe("merging", () => {
  test("a PR is stacked on the open PR whose branch it merges into", () => {
    const stacked = (name: string, base: string): PrInfo => ({ ...pr("web", "OPEN"), group: name, branch: name, pr_base_branch: base });
    const prs = [stacked("abc-2", "abc-1"), stacked("abc-1", "main")];
    expect(stackedOn(prs[0], prs)?.branch).toBe("abc-1");
    expect(stackedOn(prs[1], prs)).toBeUndefined();
  });

  test("only the methods every repo allows are offered", () => {
    const prs = [pr("a", "OPEN", ["squash", "merge", "rebase"]), pr("b", "OPEN", ["squash", "merge"])];
    expect(commonMethods(prs)).toEqual(["squash", "merge"]);
    expect(commonMethods([pr("c", "OPEN")])).toEqual(["squash", "merge", "rebase"]);
  });

  test("a PR can be merged once its checks pass, it has no conflict and the user may merge", () => {
    const ready: PrInfo = { ...pr("a", "OPEN"), checks: "SUCCESS", mergeable: "MERGEABLE", can_merge: true };
    const blocker = (p: PrInfo) => mergeBlocker(p, [p]);
    expect(blocker(ready)).toBeNull();
    expect(blocker({ ...ready, checks: null, can_merge: null })).toBeNull();
    expect(blocker({ ...ready, checks: "FAILURE" })).toBe("checks failed");
    expect(blocker({ ...ready, checks: "PENDING" })).toBe("checks are running");
    expect(blocker({ ...ready, mergeable: "CONFLICTING" })).toBe("conflicts with main");
    expect(blocker({ ...ready, mergeable: "UNKNOWN" })).toBe("GitHub is still checking for conflicts");
    expect(blocker({ ...ready, can_merge: false })).toBe("you cannot merge into this repo");
    expect(blocker({ ...ready, is_draft: true })).toBe("is a draft");
    expect(blocker({ ...ready, review_decision: "CHANGES_REQUESTED" })).toBe("changes requested");
    expect(blocker({ ...ready, review_decision: "REVIEW_REQUIRED" })).toBe("review required");
    expect(blocker({ ...ready, review_decision: "APPROVED" })).toBeNull();
    expect(blocker({ ...ready, state: "MERGED" })).toBe("is not open");
  });

  test("a PR stacked on an open one waits for it", () => {
    const ready: PrInfo = { ...pr("a", "OPEN"), checks: "SUCCESS", mergeable: "MERGEABLE", can_merge: true };
    const below: PrInfo = { ...ready, branch: "abc-1", number: 6n };
    const above: PrInfo = { ...ready, branch: "abc-2", pr_base_branch: "abc-1" };
    expect(mergeBlocker(above, [below, above])).toBe("stacked on #6, to merge first");
    expect(mergeBlocker(above, [{ ...below, state: "MERGED" }, above])).toBeNull();
    expect(mergeBlocker(below, [below, above])).toBeNull();
    expect(stackedAbove(below, [below, above])).toEqual([above]);
  });

  test("a PR is stacked only on a PR of its own repo", () => {
    // Every repo on the workspace's branch; an agent's PR in alpha targets it.
    const ready: PrInfo = { ...pr("alpha", "OPEN"), checks: "SUCCESS", mergeable: "MERGEABLE", can_merge: true };
    const beta: PrInfo = { ...ready, repo: "beta", branch: "jc/dec" };
    const sub: PrInfo = { ...ready, group: "abc-1", branch: "jc/dec--abc-1", pr_base_branch: "jc/dec" };
    expect(stackedOn(sub, [beta, sub])).toBeUndefined();
    expect(mergeBlocker(sub, [beta, sub])).toBeNull();
    const alpha: PrInfo = { ...ready, branch: "jc/dec" };
    expect(stackedOn(sub, [beta, alpha, sub])).toBe(alpha);
  });
});
