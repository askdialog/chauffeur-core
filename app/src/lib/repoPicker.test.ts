import { describe, expect, test } from "bun:test";
import type { RepoInfo } from "@chauffeur/protocol";
import { githubRepoRef, mainOwner, ownerAvatar, ownerFilters, repoSections, shortAge, showsOwner } from "./repoPicker";

const repo = (fullName: string, extra: Partial<RepoInfo> = {}): RepoInfo => {
  const [owner, name] = fullName.split("/");
  return {
    name,
    key: owner === "acme" ? name : fullName,
    full_name: fullName,
    owner,
    owner_is_org: owner !== "octocat",
    private: false,
    local: false,
    clone_path: null,
    default_branch: "main",
    description: null,
    pushed_at: null,
    last_used: null,
    is_default: false,
    ...extra,
  };
};
const local = (name: string): RepoInfo => ({ ...repo(`x/${name}`), key: name, full_name: name, owner: "", local: true });

const ecom = repo("acme/web", { last_used: "2026-11-04T10:00:00Z" });
const api = repo("acme/api", { last_used: "2026-11-03T10:00:00Z", description: "Core API" });
const adblog = repo("acme/blog");
const blog = repo("octocat/blog", { last_used: "2026-11-04T11:00:00Z" });
const dotfiles = repo("octocat/dotfiles");
const kanso = repo("kanso-studio/kanso-web");
const notes = local("chauffeur-notes");
const all = [ecom, api, adblog, blog, dotfiles, kanso, notes];

describe("repoSections", () => {
  test("Recent first, then each owner with the main organization and the account first, then this Mac", () => {
    const sections = repoSections(all, { account: "octocat" });
    expect(sections.map((s) => [s.title, s.count, s.repos.map((r) => r.key)])).toEqual([
      ["Recent", null, ["octocat/blog", "web", "api"]],
      ["acme", "3", ["blog"]],
      ["octocat", "2", ["octocat/dotfiles"]],
      ["kanso-studio", "1", ["kanso-studio/kanso-web"]],
      ["On this Mac", "1", ["chauffeur-notes"]],
    ]);
  });

  test("an owner filter or a search hides Recent and counts the matches", () => {
    expect(repoSections(all, { owner: "octocat" }).map((s) => [s.title, s.repos.length])).toEqual([["octocat", 2]]);
    const searched = repoSections(all, { query: "API" });
    expect(searched.map((s) => [s.title, s.count, s.repos.map((r) => r.key)])).toEqual([["acme", "1 of 3", ["api"]]]);
    expect(repoSections(all, { query: "kanso-studio/" })[0].repos).toEqual([kanso]);
  });

  test("the chosen repos come first, under Selected, and are not repeated below", () => {
    const sections = repoSections(all, { account: "octocat", selected: ["api", "kanso-studio/kanso-web"] });
    expect(sections.map((s) => [s.title, s.repos.map((r) => r.key)])).toEqual([
      ["Selected", ["api", "kanso-studio/kanso-web"]],
      ["Recent", ["octocat/blog", "web"]],
      ["acme", ["blog"]],
      ["octocat", ["octocat/dotfiles"]],
      ["On this Mac", ["chauffeur-notes"]],
    ]);
    const searched = repoSections(all, { query: "ap", selected: ["api"] });
    expect(searched.map((s) => [s.title, s.repos.map((r) => r.key)])).toEqual([["Selected", ["api"]]]);
  });
});

test("the owner shows outside the main organization or when two owners share a name", () => {
  expect(mainOwner(all)).toBe("acme");
  expect(showsOwner(ecom, all)).toBe(false);
  expect(showsOwner(adblog, all)).toBe(true);
  expect(showsOwner(dotfiles, all)).toBe(true);
  expect(showsOwner(notes, all)).toBe(false);
  expect(ownerFilters(all, "octocat")).toEqual([
    { owner: "acme", count: 3 },
    { owner: "octocat", count: 2 },
    { owner: "kanso-studio", count: 1 },
  ]);
});

test("a pasted GitHub URL or a typed owner/name names a repo", () => {
  expect(githubRepoRef("https://github.com/vercel/next.js")).toBe("vercel/next.js");
  expect(githubRepoRef("github.com/vercel/next.js.git")).toBe("vercel/next.js");
  expect(githubRepoRef("https://github.com/vercel/next.js/pull/12")).toBe("vercel/next.js");
  expect(githubRepoRef("git@github.com:acme-labs/pricing-engine.git")).toBe("acme-labs/pricing-engine");
  expect(githubRepoRef(" octocat/blog ")).toBe("octocat/blog");
  expect(githubRepoRef("web")).toBeNull();
  expect(githubRepoRef("https://gitlab.com/a/b")).toBeNull();
  expect(githubRepoRef("a/b/c")).toBeNull();
  expect(githubRepoRef("../x")).toBeNull();
});

test("ages and avatars", () => {
  const now = Date.parse("2026-11-04T12:00:00Z");
  expect(shortAge("2026-11-04T10:00:00Z", now)).toBe("2h");
  expect(shortAge("2026-11-01T12:00:00Z", now)).toBe("3d");
  expect(shortAge("2026-10-14T12:00:00Z", now)).toBe("3w");
  expect(shortAge("2026-09-01T12:00:00Z", now)).toBe("2mo");
  expect(shortAge("2024-11-01T12:00:00Z", now)).toBe("2y");
  expect(shortAge(null, now)).toBe("");
  expect(ownerAvatar("kanso-studio").initials).toBe("KS");
  expect(ownerAvatar("acme").initials).toBe("AC");
  expect(ownerAvatar("acme").color).toBe(ownerAvatar("acme").color);
});
