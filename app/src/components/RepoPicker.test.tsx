import { expect, spyOn } from "bun:test";
import type { GithubStatus, ReposListResult, ReposPageParams, ReposPageResult } from "@chauffeur/protocol";
import { act, StrictMode } from "react";
import { deferred, domTest } from "@/test/dom";
import { repo, repoPage as page } from "@/test/repos";

domTest("new workspace uses cached defaults, loads only requested pages and keeps selections through search races and retries", import.meta.path, async ({ render, click, fill, until }) => {
  const { CreateWorkspaceDialog } = await import("./CreateWorkspaceDialog");
  const { client, loadAllRepos, loadRepos, useApp } = await import("@/state/app");
  const before = useApp.getState();
  useApp.setState({ repos: null, reposError: null, reposRevision: 0, reposSync: { refreshedAt: null, githubError: null }, createOpen: true, createRetry: null, models: [], loadout: [] });
  const cached = deferred<ReposListResult>();
  const first = deferred<ReposPageResult>();
  const oldSearch = deferred<ReposPageResult>();
  const recent = Array.from({ length: 50 }, (_, index) => repo(`repo-${String(index + 1).padStart(5, "0")}`));
  const calls: string[] = [];
  const pages: ReposPageParams[] = [];
  let fail = false;
  const rpc = spyOn(client, "call").mockImplementation(async (method, params) => {
    calls.push(method);
    if (method === "repos.cached") return cached.promise;
    if (method === "repos.list") return { repos: [repo("new-default", true), repo("default-outside-page")] };
    if (method === "settings.get") return { workspaces: { github_org: "acme", branch_prefix: "test", github_login: "tester" }, linear: { enabled: false, start_tickets: false } };
    if (method === "github.status") return {
      git: true, git_install: "xcode", gh: "/fake/gh", own: false, version: "test", download_size: null,
      account: "tester", organizations: ["acme"], organization: "acme", connecting: null,
    } satisfies GithubStatus;
    if (method === "repos.page" && "query" in params && "refresh" in params) {
      pages.push(params);
      if (fail) return { ...page([]), github_error: "GitHub rate limit exceeded" };
      if (params.query === "repo-19999") return oldSearch.promise;
      if (params.query === "repo-18000") return page([repo("repo-18000")]);
      if (params.cursor === "next") return page([repo("repo-00051")]);
      return first.promise;
    }
    throw new Error(`Unexpected RPC: ${method}`);
  });
  const type = (text: string) => fill('[data-testid="repo-filter"]', text);
  try {
    const startup = loadRepos();
    await render(<StrictMode><CreateWorkspaceDialog /></StrictMode>);
    expect(calls.filter((method) => method === "repos.cached")).toHaveLength(1);
    expect(calls).not.toContain("repos.list");
    await act(async () => {
      cached.resolve({ repos: [repo("default-outside-page", true)] });
      await startup;
    });
    expect(document.querySelector<HTMLButtonElement>('[data-testid="create-workspace"]')?.disabled).toBe(false);
    expect(pages).toHaveLength(0);

    await click('[data-testid="add-repo"]');
    await until(() => pages.length === 1);
    expect(document.querySelector('[data-repo="default-outside-page"]')?.getAttribute("aria-selected")).toBe("true");
    await act(async () => { first.resolve(page(recent, "next")); });
    await until(() => !!document.querySelector('[data-testid="repos-load-more"]'));
    expect(pages).toHaveLength(1);
    await click('[data-testid="repos-load-more"]');
    await until(() => !!document.querySelector('[data-repo="repo-00051"]'));
    expect(pages).toHaveLength(2);
    expect(pages[1].cursor).toBe("next");
    expect(document.querySelector('[data-repo="default-outside-page"]')?.getAttribute("aria-selected")).toBe("true");

    await click('[data-owner="acme"]');
    await until(() => pages.some((params) => params.owner === "acme"));
    await type("repo-1");
    await type("repo-19999");
    await until(() => pages.some((params) => params.query === "repo-19999"));
    expect(pages.some((params) => params.query === "repo-1")).toBe(false);
    await type("repo-18000");
    await until(() => !!document.querySelector('[data-repo="repo-18000"]'));
    await act(async () => { oldSearch.resolve(page([repo("repo-19999")])); });
    expect(document.querySelector('[data-repo="repo-19999"]')).toBeNull();
    expect(useApp.getState().repos?.some((entry) => entry.key === "repo-19999")).toBe(false);

    fail = true;
    await click('button[aria-label="Refresh from GitHub"]');
    await until(() => !!document.querySelector('[data-testid="repos-github-error"]'));
    expect(document.querySelector('[data-repo="repo-18000"]')).not.toBeNull();
    expect(document.querySelector('[data-testid="create-workspace"]')?.closest('[role="dialog"]')?.textContent).toContain("default-outside-page");
    fail = false;
    const retry = [...document.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent?.trim() === "Retry");
    if (!retry) throw new Error("Missing Retry");
    await act(async () => { retry.click(); });
    await until(() => !document.querySelector('[data-testid="repos-github-error"]'));
    expect(pages.at(-1)?.refresh).toBe(true);

    await click('[data-repo="repo-18000"]');
    await until(() => document.querySelector('[data-testid="repo-filter"]')?.getAttribute("value") === "");
    const dialog = document.querySelector('[role="dialog"]');
    expect(dialog?.textContent).toContain("default-outside-page");
    expect(dialog?.textContent).toContain("repo-18000");
    await click('[data-owner="All"]');
    await type("not-loaded");
    expect(document.querySelector('[data-testid="repos-search-scope"]')?.textContent).toContain("Choose an owner");
    expect(pages.some((params) => params.query === "not-loaded")).toBe(false);
    expect(calls).not.toContain("repos.list");

    await act(async () => {
      useApp.setState({ createOpen: false });
      await loadAllRepos();
    });
    expect(useApp.getState().repos?.map((entry) => entry.key)).toEqual(["new-default", "default-outside-page"]);
    expect(useApp.getState().repos?.filter((entry) => entry.is_default).map((entry) => entry.key)).toEqual(["new-default"]);
    await act(async () => { useApp.setState({ createOpen: true }); });
    expect(document.querySelector('[role="dialog"]')?.textContent).toContain("new-default");
    expect(document.querySelector('[role="dialog"]')?.textContent).not.toContain("default-outside-page");
  } finally {
    await render(null);
    rpc.mockRestore();
    useApp.setState(before, true);
  }
});

domTest("a click toggles a repo in place: rows keep their order until the filter changes, and hovering never scrolls the list", import.meta.path, async ({ render, click, fill, until }) => {
  const { CreateWorkspaceDialog } = await import("./CreateWorkspaceDialog");
  const { client, loadRepos, useApp } = await import("@/state/app");
  const before = useApp.getState();
  useApp.setState({ repos: null, reposError: null, reposRevision: 0, reposSync: { refreshedAt: null, githubError: null }, createOpen: true, createRetry: null, models: [], loadout: [] });
  const used = (name: string, at: string) => ({ ...repo(name), last_used: at });
  const repos = [repo("chosen", true), used("first", "2026-11-22T10:00:00Z"), used("second", "2026-11-22T09:00:00Z"), used("third", "2026-11-22T08:00:00Z")];
  const rpc = spyOn(client, "call").mockImplementation(async (method) => {
    if (method === "repos.cached") return { repos };
    if (method === "settings.get") return { workspaces: { github_org: "acme", branch_prefix: "test", github_login: "tester" }, linear: { enabled: false, start_tickets: false } };
    if (method === "github.status") return {
      git: true, git_install: "xcode", gh: "/fake/gh", own: false, version: "test", download_size: null,
      account: "tester", organizations: ["acme"], organization: "acme", connecting: null,
    } satisfies GithubStatus;
    if (method === "repos.page") return page(repos);
    throw new Error(`Unexpected RPC: ${method}`);
  });
  const order = () => [...document.querySelectorAll("[data-repo]")].map((row) => row.getAttribute("data-repo"));
  const checked = (key: string) => document.querySelector(`[data-repo="${key}"]`)?.getAttribute("aria-selected");
  try {
    await act(async () => { await loadRepos(); });
    await render(<CreateWorkspaceDialog />);
    await click('[data-testid="add-repo"]');
    await until(() => !!document.querySelector('[data-repo="third"]'));
    expect(order()).toEqual(["chosen", "first", "second", "third"]);

    const scrolled: string[] = [];
    const scroll = spyOn(window.HTMLElement.prototype, "scrollIntoView").mockImplementation(function (this: HTMLElement) {
      scrolled.push(this.getAttribute("data-repo") ?? "");
    });
    await act(async () => {
      document.querySelector('[data-repo="third"]')?.dispatchEvent(new window.MouseEvent("mousemove", { bubbles: true }));
    });
    expect(scrolled).toEqual([]);
    scroll.mockRestore();

    await click('[data-repo="second"]');
    expect(checked("second")).toBe("true");
    expect(order()).toEqual(["chosen", "first", "second", "third"]);
    await click('[data-repo="chosen"]');
    expect(checked("chosen")).toBe("false");
    expect(order()).toEqual(["chosen", "first", "second", "third"]);

    await fill('[data-testid="repo-filter"]', "x");
    await fill('[data-testid="repo-filter"]', "");
    expect(order()).toEqual(["second", "first", "third", "chosen"]);
  } finally {
    await render(null);
    rpc.mockRestore();
    useApp.setState(before, true);
  }
});
