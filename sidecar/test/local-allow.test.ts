import { describe, expect, test } from "bun:test";
import type { Settings } from "@anthropic-ai/claude-agent-sdk";
import { buildClaudeOptions } from "../src/agents/claude/options.ts";
import { optionDeps as deps, startParams as start } from "./support/claude-options.ts";

describe("the user's local allow rules", () => {
  test("are granted as flag settings, which Claude Code applies in a folder never trusted", () => {
    const local_allow = ["Bash(make)", "Edit(//ws/web/src/**)"];
    const allow = (buildClaudeOptions(start({ local_allow }), deps).settings as Settings).permissions?.allow;
    expect(allow).toEqual(expect.arrayContaining(local_allow));
  });
});
