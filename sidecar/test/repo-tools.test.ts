import { describe, expect, test } from "bun:test";
import { PendingRequests } from "../src/agents/pending.ts";
import { addRepo, listRepos } from "../src/agents/repo-tools.ts";
import type { ChatEvent } from "../src/protocol.ts";
import { FakeHost, flush } from "./support/fakes.ts";

function setup() {
  const host = new FakeHost();
  const events: ChatEvent[] = [];
  let ids = 0;
  const requests = new PendingRequests((event) => events.push(event), () => `req-${++ids}`);
  return { host, events, requests };
}

describe("list_repos", () => {
  test("names the workspace, its repos with their folder, then those it can add", async () => {
    const { host } = setup();
    expect(await listRepos(host)).toEqual({
      text: [
        "In this workspace (w):",
        "- web: The monolith (folder /workspaces/w/web)",
        "",
        "Not in it, can be proposed with add_repo:",
        "- analytics: Dashboards",
      ].join("\n"),
      error: false,
    });
  });

  test("a daemon that cannot list them is an error the agent reads", async () => {
    const { host } = setup();
    host.listRepos = async () => {
      throw new Error("gh is logged out");
    };
    expect(await listRepos(host)).toEqual({ text: "Chauffeur could not list the repos: gh is logged out", error: true });
  });
});

describe("add_repo", () => {
  test("in an exploration, the card says so and the agent learns the checkout is detached", async () => {
    const { host, events, requests } = setup();
    host.repos = { ...host.repos, explore: true };
    host.addRepo = async (repo) => {
      host.added.push(repo);
      return { path: `/workspaces/w/${repo}`, branch: null, setup: false };
    };
    const call = addRepo(host, requests, { repo: "analytics", reason: "Pour lire le schéma" });
    await flush();
    expect(events[0]).toMatchObject({ type: "permission_request", title: "Add analytics to this exploration?" });
    requests.respond("req-1", { type: "allow", always: true });
    const answer = await call;
    expect(answer.error).toBe(false);
    expect(answer.text).toContain("as a detached checkout of its base branch");
    expect(answer.text).not.toContain("setup script");
    expect(host.added).toEqual(["analytics"]);
  });

  test("another owner's repo is named owner/name; a short name only when one owner has it", async () => {
    const { host, events, requests } = setup();
    host.repos = {
      ...host.repos,
      repos: [
        ...host.repos.repos,
        { name: "blog", description: null, path: null },
        { name: "octocat/blog", description: null, path: null },
        { name: "octocat/dotfiles", description: null, path: null },
      ],
    };
    const ask = async (repo: string) => {
      const call = addRepo(host, requests, { repo, reason: "r" });
      await flush();
      const asked = events.at(-1);
      if (asked?.type === "permission_request") requests.respond(asked.request_id, { type: "allow", always: false });
      return call;
    };
    await ask("octocat/blog");
    await ask("dotfiles");
    await ask("acme/blog");
    expect(host.added).toEqual(["octocat/blog", "octocat/dotfiles", "blog"]);
    expect(events.filter((e) => e.type === "permission_request").map((e) => e.type === "permission_request" && e.title)).toEqual([
      "Add octocat/blog to this workspace?",
      "Add octocat/dotfiles to this workspace?",
      "Add blog to this workspace?",
    ]);
  });

  test("Not now with words passes them on", async () => {
    const { host, requests } = setup();
    const call = addRepo(host, requests, { repo: "analytics", reason: "r" });
    await flush();
    requests.respond("req-1", { type: "deny", message: "on le fera dans un autre ticket" });
    expect((await call).text).toStartWith("The user chose not to add analytics now: on le fera dans un autre ticket");
  });

  test("an interrupted call withdraws its card", async () => {
    const { host, events, requests } = setup();
    const abort = new AbortController();
    const call = addRepo(host, requests, { repo: "analytics", reason: "r" }, { signal: abort.signal });
    await flush();
    abort.abort();
    expect(await call).toMatchObject({ error: true });
    expect(events.at(-1)).toEqual({ type: "request_resolved", request_id: "req-1", outcome: "cancelled" });
    expect(host.added).toEqual([]);
  });

  test("a workspace that is not ready refuses without asking", async () => {
    const { host, events, requests } = setup();
    host.repos = { ...host.repos, state: "archiving" };
    expect(await addRepo(host, requests, { repo: "analytics", reason: "r" })).toEqual({
      text: "This workspace is archiving: no repo can be added to it now.",
      error: true,
    });
    expect(events).toEqual([]);
  });

  test("a failed addition, a missing name: errors the agent reads, no card for the latter", async () => {
    const { host, events, requests } = setup();
    host.addRepoError = new Error("analytics could not be fetched");
    const call = addRepo(host, requests, { repo: "analytics", reason: "r" });
    await flush();
    requests.respond("req-1", { type: "allow", always: false });
    expect(await call).toEqual({
      text: "The user said yes, but Chauffeur could not add analytics: analytics could not be fetched",
      error: true,
    });
    expect(await addRepo(host, requests, { reason: "r" })).toMatchObject({ error: true });
    expect(events.filter((e) => e.type === "permission_request")).toHaveLength(1);
  });
});
