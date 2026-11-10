import { describe, expect, test } from "bun:test";
import type { RepoInfo } from "@chauffeur/protocol";
import {
  branchErrorOf,
  branchPrefix,
  branchPreview,
  checkoutBranch,
  isDesignShareLink,
  isClaudeArtifactLink,
  isWorkspaceShareLink,
  slugify,
  startsTicket,
  toggledRepo,
  withoutTicketRepos,
  withTicketRepos,
} from "./newWorkspace";
import { RpcCallError } from "./rpc";

const base = { kind: "workspace" as const, prefix: "jc", branch: "", name: "", child: false, tickets: [], linked: {}, shareTitle: null };
const cart = { identifier: "ABC-123", title: "Checkout loses or randomly resets the cart" };

describe("slugify", () => {
  test("keeps letters and digits, lower-cased, other runs as one dash, as the daemon does", () => {
    expect(slugify("Fix the Price  filter!", 60)).toBe("fix-the-price-filter");
    expect(slugify("équipe café", 60)).toBe("equipe-cafe");
    expect(slugify("  --a--  ", 60)).toBe("a");
  });

  test("a cut never ends on a dash", () => {
    expect(slugify("retry bedrock read", 6)).toBe("retry");
  });
});

describe("branchPreview", () => {
  test("an exploration and a design have no branch", () => {
    expect(branchPreview({ ...base, kind: "explore" })).toEqual({ kind: "none" });
    expect(branchPreview({ ...base, kind: "design" })).toEqual({ kind: "none" });
  });

  test("without name nor ticket, the prefix is known and the rest comes from the first prompt", () => {
    expect(branchPreview(base)).toEqual({ kind: "later", prefix: "jc/", note: "named from the first prompt" });
  });

  test("a typed branch wins over everything", () => {
    expect(branchPreview({ ...base, branch: " jc/mine ", tickets: [cart] })).toEqual({ kind: "known", branch: "jc/mine", from: null });
  });

  test("one ticket with Linear's branch takes it", () => {
    const linked = { "ABC-123": { identifier: "ABC-123", branch_name: "jc/abc-123-checkout-loses-the-cart", state: "In Progress", state_type: "started", repos: [] } };
    expect(branchPreview({ ...base, tickets: [cart], linked })).toEqual({
      kind: "known",
      branch: "jc/abc-123-checkout-loses-the-cart",
      from: "from ABC-123 in Linear",
    });
  });

  test("one ticket without Linear's branch takes its id and title after the prefix", () => {
    expect(branchPreview({ ...base, tickets: [cart] })).toMatchObject({
      branch: "jc/abc-123-checkout-loses-or-randomly-resets-the-ca",
    });
  });

  test("a child's branch is the ticket id and title", () => {
    expect(branchPreview({ ...base, child: true, tickets: [cart] })).toMatchObject({
      branch: "ABC-123/checkout-loses-or-randomly-resets-the-cart",
    });
  });

  test("a typed name names the branch; two tickets do not", () => {
    expect(branchPreview({ ...base, name: "Retry Bedrock read" })).toMatchObject({ branch: "jc/retry-bedrock-read" });
    expect(branchPreview({ ...base, tickets: [cart, { identifier: "XYZ-1", title: "x" }] }).kind).toBe("later");
  });

  test("an imported share is named after its title, as a fork", () => {
    expect(branchPreview({ ...base, shareTitle: "Checkout redesign" })).toMatchObject({ branch: "jc/checkout-redesign-fork" });
  });
});

test("the prefix is the setting, else the GitHub login, else chauffeur", () => {
  expect(branchPrefix({ branch_prefix: "jc", github_login: "justin" })).toBe("jc");
  expect(branchPrefix({ branch_prefix: "", github_login: "justin" })).toBe("justin");
  expect(branchPrefix(null)).toBe("chauffeur");
});

describe("the repos a ticket brings", () => {
  test("are marked as its own, and go when it is unlinked", () => {
    let choice = { selection: ["web"], sources: {} };
    choice = withTicketRepos(choice, "ABC-123", ["api", "web", "analytics"]);
    expect(choice.selection).toEqual(["web", "api", "analytics"]);
    expect(choice.sources).toEqual({ "api": "ABC-123", "analytics": "ABC-123" });
    expect(withoutTicketRepos(choice, "ABC-123")).toEqual({ selection: ["web"], sources: {} });
  });

  test("a repo removed and picked again by hand is the user's own", () => {
    let choice = withTicketRepos({ selection: [], sources: {} }, "ABC-123", ["analytics"]);
    choice = toggledRepo(toggledRepo(choice, "analytics"), "analytics");
    expect(choice).toEqual({ selection: ["analytics"], sources: {} });
    expect(withoutTicketRepos(choice, "ABC-123").selection).toEqual(["analytics"]);
  });
});

test("a ticket moves to In Progress only from a workspace, with the setting on, when not started", () => {
  const ticket = (state_type: string | null) => ({ identifier: "XYZ-1", branch_name: null, state: null, state_type, repos: [] });
  expect(startsTicket(ticket("unstarted"), true, "workspace")).toBe(true);
  expect(startsTicket(ticket("backlog"), true, "workspace")).toBe(true);
  expect(startsTicket(ticket("started"), true, "workspace")).toBe(false);
  expect(startsTicket(ticket("unstarted"), false, "workspace")).toBe(false);
  expect(startsTicket(ticket("unstarted"), true, "explore")).toBe(false);
  expect(startsTicket(undefined, true, "workspace")).toBe(false);
});

test("an exploration names the base branch its repos share", () => {
  const repo = (key: string, default_branch: string | null) => ({ key, default_branch }) as RepoInfo;
  const repos = [repo("a", "main"), repo("b", "main"), repo("c", "staging")];
  expect(checkoutBranch(repos, ["a", "b"])).toBe("origin/main");
  expect(checkoutBranch(repos, ["a", "c"])).toBeNull();
  expect(checkoutBranch(repos, [])).toBeNull();
});

test("a refusal naming the branch gives its suggestion; others are not the branch's", () => {
  const held = new RpcCallError("workspace.create", { code: 409, message: "held", data: { field: "branch", suggestion: "jc/x-2" } });
  expect(branchErrorOf(held)).toEqual({ suggestion: "jc/x-2" });
  const invalid = new RpcCallError("workspace.create", { code: 400, message: "bad", data: { field: "branch" } });
  expect(branchErrorOf(invalid)).toEqual({ suggestion: null });
  expect(branchErrorOf(new RpcCallError("workspace.create", { code: 400, message: "no repo" }))).toBeNull();
  expect(branchErrorOf(new Error("x"))).toBeNull();
});

test("share links are told apart: a workspace's /w/, a design's /s/", () => {
  expect(isWorkspaceShareLink("https://share.example.com/w/7a2f9c01")).toBe(true);
  expect(isWorkspaceShareLink("https://share.example.com/s/7a2f9c01")).toBe(false);
  expect(isDesignShareLink("https://share.example.com/s/7a2f9c01")).toBe(true);
  expect(isDesignShareLink("https://share.example.com/s/7a2f9c01?x=1")).toBe(true);
  expect(isDesignShareLink("not a link")).toBe(false);
});

test("Claude imports accept public artifact links and reject other hosts and credentials", () => {
  expect(isClaudeArtifactLink("https://claude.ai/artifact/AbCdEf1234567890#page-example")).toBe(true);
  for (const link of ["https://example.com/artifact/a", "http://claude.ai/artifact/a", "https://user@claude.ai/artifact/a", "https://claude.ai:123/artifact/a", "https://claude.ai/chat/a"]) {
    expect(isClaudeArtifactLink(link)).toBe(false);
  }
});
