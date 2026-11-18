import { describe, expect, test } from "bun:test";
import type { Settings } from "@anthropic-ai/claude-agent-sdk";
import { buildClaudeOptions } from "../src/agents/claude/options.ts";
import { optionDeps as deps, startParams as start } from "./support/claude-options.ts";

describe("a chat's brands", () => {
  test("are read without asking, and a chat without one gets no rule", () => {
    const branded = buildClaudeOptions(start({ brands_dir: "/home/me/.chauffeur/brands" }), deps);
    expect((branded.settings as Settings | undefined)?.permissions?.allow).toContain("Read(//home/me/.chauffeur/brands/**)");
    const plain = buildClaudeOptions(start({}), deps);
    expect((plain.settings as Settings | undefined)?.permissions?.allow ?? []).not.toContainEqual(expect.stringContaining("brands"));
  });
});

describe("the user's local allow rules", () => {
  test("are granted as flag settings, never in a design chat, which starts outside the workspace", () => {
    const local_allow = ["Bash(make)", "Edit(//ws/web/src/**)"];
    const allow = (params: Parameters<typeof start>[0]) => (buildClaudeOptions(start(params), deps).settings as Settings | undefined)?.permissions?.allow ?? [];
    expect(allow({ local_allow })).toEqual(expect.arrayContaining(local_allow));
    expect(allow({ local_allow, design_dir: "/w/.chauffeur/designs/c" })).not.toContain("Bash(make)");
  });
});
