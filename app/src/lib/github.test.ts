import { expect, test } from "bun:test";
import type { GithubConnecting, GithubStatus } from "@chauffeur/protocol";
import { connectingLabel, githubStep, isConnecting, organizationChoices } from "./github";

const status = (changes: Partial<GithubStatus>): GithubStatus => ({
  git: true,
  git_install: "xcode",
  gh: "/Users/me/.chauffeur/bin/gh",
  own: true,
  version: "2.98.0",
  download_size: 13_938_611,
  account: "octocat",
  organizations: ["acme"],
  organization: "acme",
  connecting: null,
  ...changes,
});
const step = (changes: Partial<GithubConnecting>): GithubConnecting => ({ phase: "starting", completed: 0, total: 0, code: null, url: null, error: null, ...changes });

test("git comes first, then an account, then the organization repos come from", () => {
  expect(githubStep(status({ git: false, account: null }))).toBe("git");
  expect(githubStep(status({ gh: null, account: null, organizations: [] }))).toBe("connect");
  expect(githubStep(status({ organization: "" }))).toBe("organization");
  expect(githubStep(status({}))).toBe("ready");
});

test("repos come from one of the account's organizations or from the account itself", () => {
  expect(organizationChoices(status({ organizations: ["acme", "globex"] }))).toEqual(["acme", "globex", "octocat"]);
  expect(organizationChoices(status({ account: null, organizations: [] }))).toEqual([]);
});

test("a failed connection is over, so another may start", () => {
  expect(isConnecting(status({ connecting: step({}) }))).toBe(true);
  expect(isConnecting(status({ connecting: step({ error: "expired" }) }))).toBe(false);
  expect(isConnecting(status({}))).toBe(false);
});

test("a connection reads without naming the tools behind it", () => {
  expect(connectingLabel(step({ phase: "downloading", completed: 6_969_306, total: 13_938_611 }))).toBe("Getting GitHub's connector, 50% of 14 MB");
  expect(connectingLabel(step({ phase: "unpacking" }))).toBe("Getting GitHub's connector");
  expect(connectingLabel(step({ phase: "waiting", code: "ABCD-1234", url: "https://github.com/login/device" }))).toBe("Enter this code on GitHub, then click Authorize");
  expect(connectingLabel(step({ phase: "configuring" }))).toBe("Almost done: setting up git");
  expect(connectingLabel(step({ error: "the code expired" }))).toBe("Could not connect GitHub: the code expired");
});
