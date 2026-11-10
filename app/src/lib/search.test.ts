import { expect, test } from "bun:test";
import type { SearchStatus, WorkspaceInfo } from "@chauffeur/protocol";
import { filterOptions, NO_CHOICES, searchExcerpt, searchFilters, semanticLine, workspaceKept } from "./search";

test("an excerpt highlights several matched words while keeping markup as text", () => {
  expect(searchExcerpt('…la \u0001connexion\u0002 <script> à \u0001Shopify\u0002…')).toEqual([
    { text: "…la ", highlight: false },
    { text: "connexion", highlight: true },
    { text: " <script> à ", highlight: false },
    { text: "Shopify", highlight: true },
    { text: "…", highlight: false },
  ]);
  expect(searchExcerpt("")).toEqual([]);
});

const now = new Date("2026-11-08T12:00:00.000Z");

test("choices left at any narrow nothing, and a date range counts back from now", () => {
  expect(searchFilters(NO_CHOICES, now)).toEqual({ sources: [], repos: [], tickets: [] });
  expect(searchFilters({ scope: "archived", where: "notes", repo: "api", ticket: "XYZ-1", date: "week", pr: "with" }, now)).toEqual({
    state: "archived",
    sources: ["notes"],
    repos: ["api"],
    tickets: ["XYZ-1"],
    since: "2026-11-01T12:00:00.000Z",
    pr: "any",
  });
  expect(searchFilters({ ...NO_CHOICES, pr: "merged" }, now).pr).toBe("merged");
});

function workspace(state: WorkspaceInfo["state"], repos: string[], tickets: string[]): WorkspaceInfo {
  return {
    state,
    worktrees: repos.map((repo) => ({ repo, origin: "workspace" })),
    tickets: tickets.map((identifier) => ({ identifier, title: null, url: null })),
  } as unknown as WorkspaceInfo;
}

test("a workspace matched by name stays listed only under the filters its own info answers", () => {
  const archived = workspace("archived", ["api"], ["XYZ-1"]);
  expect(workspaceKept(archived, NO_CHOICES)).toBe(true);
  expect(workspaceKept(archived, { ...NO_CHOICES, scope: "archived", repo: "api", ticket: "xyz-1", where: "titles" })).toBe(true);
  expect(workspaceKept(archived, { ...NO_CHOICES, scope: "active" })).toBe(false);
  expect(workspaceKept(archived, { ...NO_CHOICES, repo: "web" })).toBe(false);
  expect(workspaceKept(archived, { ...NO_CHOICES, where: "messages" })).toBe(false);
  // The daemon knows its dates and pull requests: its own results list it then.
  expect(workspaceKept(archived, { ...NO_CHOICES, pr: "merged" })).toBe(false);
  expect(workspaceKept(archived, { ...NO_CHOICES, date: "month" })).toBe(false);
});

test("the filters offer the workspaces' repos and tickets once each, sorted", () => {
  expect(filterOptions([workspace("ready", ["web", "api"], ["XYZ-10", "XYZ-9"]), workspace("archived", ["api"], ["XYZ-9"])])).toEqual({
    repos: ["api", "web"],
    tickets: ["XYZ-9", "XYZ-10"],
  });
});

test("the footer says why search is by keywords only, and how far the history is indexed", () => {
  const status = (semantic: SearchStatus["semantic"], embedded = 10, total = 10): SearchStatus => ({ semantic, model: "embeddinggemma", embedded, total });
  expect(semanticLine(status("ready"))).toBeNull();
  expect(semanticLine(status("ready", 0, 0))).toBeNull();
  expect(semanticLine(status("ready", 1, 3))).toEqual({ text: "Indexing by meaning: 33% done.", action: null });
  expect(semanticLine(status("no_model"))).toEqual({ text: "Keywords only: search by meaning needs embeddinggemma (621 MB).", action: "download" });
  expect(semanticLine(status("no_ollama"))?.action).toBe("settings");
  expect(semanticLine(status("disabled"))?.action).toBe("settings");
  expect(semanticLine(status("on_battery", 5, 10))?.text).toContain("50%");
  expect(semanticLine(status("starting"))?.action).toBeNull();
});
