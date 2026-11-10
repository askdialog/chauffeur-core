import { describe, expect, test } from "bun:test";
import { CodexAlwaysAllowed, commandRules, fileRules } from "../src/agents/codex/always.ts";
import type { CommandExecutionRequestApprovalParams, FileChangeRequestApprovalParams, FileUpdateChange } from "../src/agents/codex/protocol.ts";

const COMMAND: CommandExecutionRequestApprovalParams = {
  kind: "command",
  threadId: "t",
  turnId: "turn-1",
  itemId: "exec-1",
  startedAtMs: 1,
  environmentId: "local",
  command: "/bin/zsh -lc 'git commit -m \"x\"'",
  cwd: "/w",
};
const PATCH: FileChangeRequestApprovalParams = { threadId: "t", turnId: "turn-1", itemId: "p", startedAtMs: 1 };
const update = (path: string, move_path: string | null = null): FileUpdateChange => ({ path, kind: { type: "update", move_path }, diff: "" });

describe("commandRules", () => {
  test("a command: the script in its folder, as Codex remembers it", () => {
    expect(commandRules(COMMAND)).toEqual(['Bash(git commit -m "x") in /w']);
    expect(commandRules({ ...COMMAND, command: "cargo test" })).toEqual(["Bash(cargo test) in /w"]);
  });

  test("none for what Codex approves once, nor for a network request whose port is not shown", () => {
    expect(commandRules({ ...COMMAND, kind: "writeStdin" })).toEqual([]);
    expect(commandRules({ ...COMMAND, approvalId: "sub-1" })).toEqual([]);
    expect(commandRules({ ...COMMAND, command: null, cwd: null, networkApprovalContext: { host: "example.com", protocol: "https" } })).toEqual([]);
  });
});

describe("fileRules", () => {
  test("each file the change touches, a move's destination included, once", () => {
    expect(fileRules(PATCH, [update("/w/a.ts"), update("/w/b.ts", "/w/c.ts"), { path: "/w/a.ts", kind: { type: "add" }, diff: "" }])).toEqual([
      "apply_patch(/w/a.ts)",
      "apply_patch(/w/b.ts)",
      "apply_patch(/w/c.ts)",
    ]);
  });

  test("none for a change asked again after the sandbox refused it, or one not seen", () => {
    expect(fileRules({ ...PATCH, reason: "the sandbox refused it" }, [update("/w/a.ts")])).toEqual([]);
    expect(fileRules(PATCH, [])).toEqual([]);
  });
});

test("CodexAlwaysAllowed covers a request when all its rules were allowed, as Codex does", () => {
  const always = new CodexAlwaysAllowed(["apply_patch(/w/a.ts)"]);
  always.add(["apply_patch(/w/b.ts)"]);
  expect(always.covers(["apply_patch(/w/a.ts)", "apply_patch(/w/b.ts)"])).toBe(true);
  expect(always.covers(["apply_patch(/w/a.ts)", "apply_patch(/w/c.ts)"])).toBe(false);
  expect(always.covers([])).toBe(false);
});
