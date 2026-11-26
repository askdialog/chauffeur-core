import { describe, expect, test } from "bun:test";
import type { CodexCliStatus } from "@chauffeur/protocol";
import { codexOutdated, codexStateLabel, olderVersion } from "./codexCli";

const status = (over: Partial<CodexCliStatus> = {}): CodexCliStatus => ({
  path: "/usr/local/bin/codex",
  version: "0.167.0",
  latest: "0.172.0",
  updating: false,
  error: null,
  ...over,
});

describe("codex cli", () => {
  test("compares versions by their numbers", () => {
    expect(olderVersion("0.167.0", "0.172.0")).toBe(true);
    expect(olderVersion("0.99.0", "0.167.0")).toBe(true);
    expect(olderVersion("0.172.0", "0.172.0")).toBe(false);
    expect(olderVersion("0.173.0", "0.172.0")).toBe(false);
    expect(olderVersion("0.172", "0.172.1")).toBe(true);
  });

  test("a Codex behind the latest is outdated; an unknown latest is not", () => {
    expect(codexOutdated(status())).toBe(true);
    expect(codexOutdated(status({ version: "0.172.0" }))).toBe(false);
    expect(codexOutdated(status({ latest: null }))).toBe(false);
  });

  test("says where Codex stands", () => {
    expect(codexStateLabel(status())).toBe("Codex 0.167.0 is installed; 0.172.0 is out.");
    expect(codexStateLabel(status({ version: "0.172.0" }))).toBe("Codex 0.172.0 is installed, the latest.");
    expect(codexStateLabel(status({ updating: true }))).toBe("Updating Codex 0.167.0…");
    expect(codexStateLabel(status({ path: null, version: null }))).toContain("not installed");
  });
});
