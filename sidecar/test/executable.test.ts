import { afterEach, describe, expect, test } from "bun:test";
import { resetExtractedExecutable, resolveClaudeExecutable, type ExecutableEnvironment } from "../src/agents/claude/executable.ts";
import { ErrorCode } from "../src/protocol.ts";

function environment(overrides: Partial<ExecutableEnvironment> = {}): ExecutableEnvironment & { extracted: string[] } {
  const extracted: string[] = [];
  const files = new Set(["/opt/claude", "/tmp/claude-501/claude-agent-sdk-abc/claude"]);
  return {
    env: {},
    standalone: true,
    embeddedNames: () => ["claude"],
    exists: (path) => files.has(path),
    extract: (path) => {
      extracted.push(path);
      return "/tmp/claude-501/claude-agent-sdk-abc/claude";
    },
    extracted,
    ...overrides,
  };
}

describe("resolveClaudeExecutable", () => {
  afterEach(resetExtractedExecutable);

  test("CHAUFFEUR_CLAUDE_PATH wins", () => {
    expect(resolveClaudeExecutable(environment({ env: { CHAUFFEUR_CLAUDE_PATH: "/opt/claude" } }))).toBe("/opt/claude");
  });

  test("a missing CHAUFFEUR_CLAUDE_PATH is an error", () => {
    expect(() => resolveClaudeExecutable(environment({ env: { CHAUFFEUR_CLAUDE_PATH: "/nope" } }))).toThrow(
      expect.objectContaining({ code: ErrorCode.UNAVAILABLE }),
    );
  });

  test("under bun run, lets the SDK resolve its own binary", () => {
    expect(resolveClaudeExecutable(environment({ standalone: false }))).toBeUndefined();
  });

  test("in a compiled binary, extracts the embedded binary once", () => {
    const env = environment();
    expect(resolveClaudeExecutable(env)).toBe("/tmp/claude-501/claude-agent-sdk-abc/claude");
    expect(resolveClaudeExecutable(env)).toBe("/tmp/claude-501/claude-agent-sdk-abc/claude");
    expect(env.extracted).toEqual(["/$bunfs/root/claude"]);
  });

  test("a compiled binary without the embedded CLI is an error", () => {
    expect(() => resolveClaudeExecutable(environment({ embeddedNames: () => [] }))).toThrow("embeds no Claude Code executable");
  });

  test("a failed extraction (the path stays in $bunfs) is an error", () => {
    expect(() => resolveClaudeExecutable(environment({ extract: (path) => path }))).toThrow("could not extract");
  });
});
