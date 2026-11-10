import { describe, expect, test } from "bun:test";
import type { PermissionUpdate } from "@anthropic-ai/claude-agent-sdk";
import { AlwaysAllowed, alwaysAllow, parseRule, ruleCovers, ruleText } from "../src/agents/claude/always.ts";
import { permissionResult } from "../src/agents/claude/requests.ts";

// What Claude Code sent in default mode (see the PR): an MCP tool, and a Bash command in the folder.
const MCP_SUGGESTION: PermissionUpdate[] = [
  { type: "addRules", rules: [{ toolName: "mcp__notes__write_note" }], behavior: "allow", destination: "localSettings" },
];
const BASH_SUGGESTIONS: PermissionUpdate[] = [
  { type: "addRules", rules: [{ toolName: "Bash", ruleContent: "touch a.txt" }], behavior: "allow", destination: "localSettings" },
  { type: "addDirectories", directories: ["/tmp/w"], destination: "session" },
  { type: "setMode", mode: "acceptEdits", destination: "session" },
];

// What Claude Code sent in default mode for `mkdir -p /tmp/out/x`, a command writing outside the
// working directories: no rule, a folder and a mode.
const OUTSIDE_SUGGESTIONS: PermissionUpdate[] = [
  { type: "addDirectories", directories: ["/tmp/out"], destination: "session" },
  { type: "setMode", mode: "acceptEdits", destination: "session" },
];

describe("alwaysAllow", () => {
  test("keeps Claude's allow rules as they are", () => {
    expect(alwaysAllow("mcp__notes__write_note", {}, MCP_SUGGESTION)).toEqual({
      rules: [{ toolName: "mcp__notes__write_note" }],
      updatedPermissions: MCP_SUGGESTION,
    });
    expect(alwaysAllow("Bash", { command: "touch a.txt" }, BASH_SUGGESTIONS)).toEqual({
      rules: [{ toolName: "Bash", ruleContent: "touch a.txt" }],
      updatedPermissions: BASH_SUGGESTIONS.slice(0, 1),
    });
  });

  test("never changes the chat's mode nor adds a working directory", () => {
    for (const suggestions of [BASH_SUGGESTIONS, OUTSIDE_SUGGESTIONS]) {
      const { updatedPermissions } = alwaysAllow("Bash", { command: "mkdir -p /tmp/out/x" }, suggestions);
      expect(updatedPermissions.filter((u) => u.type === "setMode" || u.type === "addDirectories")).toEqual([]);
    }
    expect(alwaysAllow("Bash", { command: "mkdir -p /tmp/out/x" }, OUTSIDE_SUGGESTIONS)).toEqual({
      rules: [{ toolName: "Bash", ruleContent: "mkdir -p /tmp/out/x" }],
      updatedPermissions: [
        { type: "addRules", rules: [{ toolName: "Bash", ruleContent: "mkdir -p /tmp/out/x" }], behavior: "allow", destination: "session" },
      ],
    });
  });

  test("keeps Claude's other suggestions (a read rule for a folder)", () => {
    const read: PermissionUpdate[] = [
      { type: "addRules", rules: [{ toolName: "Read", ruleContent: "//tmp/out/**" }], behavior: "allow", destination: "session" },
    ];
    expect(alwaysAllow("Bash", { command: "cat /tmp/out/a" }, read)).toEqual({ rules: [{ toolName: "Read", ruleContent: "//tmp/out/**" }], updatedPermissions: read });
  });

  test("builds a session rule for the tool when Claude suggests none (plan mode)", () => {
    for (const suggestions of [undefined, []]) {
      expect(alwaysAllow("mcp__claude_ai_Linear__save_issue", { id: "X-1" }, suggestions)).toEqual({
        rules: [{ toolName: "mcp__claude_ai_Linear__save_issue" }],
        updatedPermissions: [
          { type: "addRules", rules: [{ toolName: "mcp__claude_ai_Linear__save_issue" }], behavior: "allow", destination: "session" },
        ],
      });
    }
  });

  test("builds its rule when Claude suggests only a mode: an edit in default mode", () => {
    const mode: PermissionUpdate[] = [{ type: "setMode", mode: "acceptEdits", destination: "session" }];
    expect(alwaysAllow("Write", { file_path: "/a" }, mode)).toEqual({
      rules: [{ toolName: "Write" }],
      updatedPermissions: [{ type: "addRules", rules: [{ toolName: "Write" }], behavior: "allow", destination: "session" }],
    });
  });

  test("never remembers Bash as a whole", () => {
    expect(alwaysAllow("Bash", { command: " npm test " }, undefined).rules).toEqual([{ toolName: "Bash", ruleContent: "npm test" }]);
  });
});

describe("permissionResult", () => {
  const remember = alwaysAllow("mcp__notes__write_note", {}, undefined);
  test("always hands Claude Code the rules, with or without suggestions", () => {
    expect(permissionResult({ type: "allow", always: true }, { a: 1 }, remember)).toEqual({
      behavior: "allow",
      updatedInput: { a: 1 },
      updatedPermissions: remember.updatedPermissions,
    });
    const suggested = alwaysAllow("mcp__notes__write_note", {}, MCP_SUGGESTION);
    expect(permissionResult({ type: "allow", always: true }, {}, suggested)).toMatchObject({ updatedPermissions: MCP_SUGGESTION });
  });
  test("once, or nothing to remember, is a plain allow", () => {
    expect(permissionResult({ type: "allow", always: false }, {}, remember)).toEqual({ behavior: "allow", updatedInput: {} });
    expect(permissionResult({ type: "allow", always: true }, {}, undefined)).toEqual({ behavior: "allow", updatedInput: {} });
  });
});

describe("ruleCovers", () => {
  test("a tool rule covers that tool only; a server rule covers its tools", () => {
    expect(ruleCovers({ toolName: "mcp__a__x" }, "mcp__a__x", {})).toBe(true);
    expect(ruleCovers({ toolName: "mcp__a__x" }, "mcp__a__y", {})).toBe(false);
    expect(ruleCovers({ toolName: "mcp__a" }, "mcp__a__y", {})).toBe(true);
    expect(ruleCovers({ toolName: "mcp__a" }, "mcp__ab__y", {})).toBe(false);
  });

  test("Bash: exact text or prefix, never a chained command", () => {
    const exact = { toolName: "Bash", ruleContent: "make test" };
    const prefix = { toolName: "Bash", ruleContent: "npm run:*" };
    expect(ruleCovers(exact, "Bash", { command: "make test" })).toBe(true);
    expect(ruleCovers(exact, "Bash", { command: "make test2" })).toBe(false);
    expect(ruleCovers(prefix, "Bash", { command: "npm run build" })).toBe(true);
    expect(ruleCovers(prefix, "Bash", { command: "npm run" })).toBe(true);
    expect(ruleCovers(prefix, "Bash", { command: "npm runner" })).toBe(false);
    for (const command of ["npm run x && rm -rf /", "npm run x; rm a", "npm run x | sh", "npm run $(rm a)", "npm run `x`", "npm run x > f", "npm run x\nrm a"]) {
      expect(ruleCovers(prefix, "Bash", { command })).toBe(false);
    }
  });

  test("reads of an absolute folder: Read, Glob and Grep under it, never out of it", () => {
    const notes = { toolName: "Read", ruleContent: "//ws/.chauffeur/**" };
    expect(ruleCovers(notes, "Read", { file_path: "/ws/.chauffeur/attachments/a.png" })).toBe(true);
    expect(ruleCovers(notes, "Read", { file_path: "/ws/.chauffeur" })).toBe(true);
    expect(ruleCovers(notes, "Glob", { pattern: "*.png", path: "/ws/.chauffeur/attachments" })).toBe(true);
    expect(ruleCovers(notes, "Grep", { pattern: "x", path: "/ws/.chauffeur/notes.md" })).toBe(true);
    // Out of it: a sibling, `..`, a relative path, no path, a tool that does not read.
    expect(ruleCovers(notes, "Read", { file_path: "/ws/.chauffeur-other/a" })).toBe(false);
    expect(ruleCovers(notes, "Read", { file_path: "/ws/.chauffeur/../repo/.env" })).toBe(false);
    expect(ruleCovers(notes, "Read", { file_path: ".chauffeur/a.png" })).toBe(false);
    expect(ruleCovers(notes, "Glob", { pattern: "*.png" })).toBe(false);
    expect(ruleCovers(notes, "Write", { file_path: "/ws/.chauffeur/a.png" })).toBe(false);
    expect(ruleCovers(notes, "Bash", { command: "cat /ws/.chauffeur/a.png" })).toBe(false);
    // A single file.
    const file = { toolName: "Read", ruleContent: "//ws/a.png" };
    expect(ruleCovers(file, "Read", { file_path: "/ws/a.png" })).toBe(true);
    expect(ruleCovers(file, "Read", { file_path: "/ws/a.png.bak" })).toBe(false);
  });

  test("other rule contents are left to Claude Code", () => {
    expect(ruleCovers({ toolName: "Read", ruleContent: "/.chauffeur/**" }, "Read", { file_path: "/ws/.chauffeur/a" })).toBe(false);
    expect(ruleCovers({ toolName: "Read", ruleContent: "~/notes/**" }, "Read", { file_path: "/Users/me/notes/a" })).toBe(false);
    expect(ruleCovers({ toolName: "Read", ruleContent: "//ws/*.png" }, "Read", { file_path: "/ws/a.png" })).toBe(false);
    expect(ruleCovers({ toolName: "WebFetch", ruleContent: "domain:x.com" }, "WebFetch", { url: "https://x.com" })).toBe(false);
  });

  test("ruleText writes rules as Claude Code's settings do", () => {
    expect(ruleText({ toolName: "mcp__a__x" })).toBe("mcp__a__x");
    expect(ruleText({ toolName: "Bash", ruleContent: "npm run:*" })).toBe("Bash(npm run:*)");
  });
});

test("AlwaysAllowed keeps each rule once and finds the one covering a call", () => {
  const always = new AlwaysAllowed();
  always.add([{ toolName: "mcp__a__x" }, { toolName: "mcp__a__x" }]);
  expect(always.covering("mcp__a__x", {})).toEqual({ toolName: "mcp__a__x" });
  expect(always.covering("mcp__a__y", {})).toBeUndefined();
});

test("rules read back from the daemon seed a new session", () => {
  expect(parseRule("mcp__a__x")).toEqual({ toolName: "mcp__a__x" });
  expect(parseRule("Bash(npm run:*)")).toEqual({ toolName: "Bash", ruleContent: "npm run:*" });
  expect(parseRule(ruleText({ toolName: "Bash", ruleContent: "echo (a)" }))).toEqual({ toolName: "Bash", ruleContent: "echo (a)" });
  const always = new AlwaysAllowed(["mcp__a__x", "Bash(npm run:*)"]);
  expect(always.covering("mcp__a__x", {})).toBeDefined();
  expect(always.covering("Bash", { command: "npm run build" })).toBeDefined();
});
