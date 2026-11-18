import { afterEach, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Settings } from "@anthropic-ai/claude-agent-sdk";
import { indexHead, readCloneMemory } from "../src/agents/claude/memory.ts";
import { buildClaudeOptions } from "../src/agents/claude/options.ts";
import { appended, optionDeps as deps, startParams as start } from "./support/claude-options.ts";

const made: string[] = [];
function folder(): string {
  const dir = realpathSync(mkdtempSync(join(tmpdir(), "clone-memory-")));
  made.push(dir);
  return dir;
}
afterEach(() => {
  for (const dir of made.splice(0)) rmSync(dir, { recursive: true, force: true });
});

/** Where Claude Code keeps a short path's memory: every character but a letter or digit as `-`. */
function memoryDir(config: string, clone: string): string {
  return join(config, "projects", clone.replace(/[^a-zA-Z0-9]/g, "-"), "memory");
}

describe("the head of a clone's MEMORY.md", () => {
  test("is cut as Claude Code loads it: 200 lines, then 25,000 characters at a line's end", () => {
    expect(indexHead("\n  - one\n- two\n\n")).toEqual({ head: "- one\n- two", cut: false });

    const lines = Array.from({ length: 250 }, (_, i) => `- line ${i + 1}`);
    const byLines = indexHead(lines.join("\n"));
    expect(byLines.cut).toBe(true);
    expect(byLines.head.split("\n")).toEqual(lines.slice(0, 200));

    const long = Array.from({ length: 150 }, (_, i) => `${i} ${"x".repeat(250)}`);
    const byChars = indexHead(long.join("\n"));
    expect(byChars.cut).toBe(true);
    expect(byChars.head.length).toBeLessThanOrEqual(25_000);
    expect(long.join("\n").startsWith(`${byChars.head}\n`)).toBe(true);

    const oneLine = indexHead("y".repeat(30_000));
    expect(oneLine).toEqual({ head: "y".repeat(25_000), cut: true });
  });
});

describe("a clone's memory", () => {
  test("is read where Claude Code keeps it, the clone named by any path", async () => {
    const clone = folder();
    const config = folder();
    const dir = memoryDir(config, clone);
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, "MEMORY.md"), "- [Deploys](deploys.md) — staging first\n");
    const link = join(folder(), "web");
    symlinkSync(clone, link);

    for (const named of [clone, link]) {
      expect(await readCloneMemory(named, { CLAUDE_CONFIG_DIR: config })).toEqual({
        clone: named,
        dir,
        index: "- [Deploys](deploys.md) — staging first",
        cut: false,
      });
    }
    expect(await readCloneMemory(clone, { HOME: config })).toBeUndefined();
  });

  test("of any size is cut as a short one is, from its head", async () => {
    const clone = folder();
    const config = folder();
    mkdirSync(memoryDir(config, clone), { recursive: true });
    writeFileSync(join(memoryDir(config, clone), "MEMORY.md"), "é".repeat(4_000_000));
    expect(await readCloneMemory(clone, { CLAUDE_CONFIG_DIR: config })).toMatchObject({ index: "é".repeat(25_000), cut: true });
  });

  test("is none without the file, with an empty one, or without the clone", async () => {
    const clone = folder();
    const config = folder();
    expect(await readCloneMemory(clone, { CLAUDE_CONFIG_DIR: config })).toBeUndefined();
    mkdirSync(memoryDir(config, clone), { recursive: true });
    writeFileSync(join(memoryDir(config, clone), "MEMORY.md"), "\n \n");
    expect(await readCloneMemory(clone, { CLAUDE_CONFIG_DIR: config })).toBeUndefined();
    expect(await readCloneMemory(join(clone, "gone"), { CLAUDE_CONFIG_DIR: config })).toBeUndefined();
  });
});

describe("a session given its clone's memory", () => {
  const memory = {
    clone: "/code/web",
    dir: "/home/me/.claude/projects/-code-web/memory",
    index: "- Deploy <b>staging</b> first",
    cut: true,
  };

  test("is told it after Chauffeur's own instructions, and may read its folder but not write it", () => {
    const options = buildClaudeOptions(start({ append_system_prompt: "Reply in Chauffeur." }), { ...deps, memory });
    const append = appended(options);
    expect(append.startsWith("Reply in Chauffeur.\n\n")).toBe(true);
    expect(append).toContain("`/code/web`");
    expect(append).toContain("read the file for the rest");
    expect(append).toContain(`<user-memory path="${memory.dir}/MEMORY.md">\n- Deploy &lt;b>staging&lt;/b> first\n</user-memory>`);
    const permissions = (options.settings as Settings).permissions;
    expect(permissions?.allow).toContain(`Read(/${memory.dir}/**)`);
    expect(permissions?.deny).toContain(`Edit(/${memory.dir}/**)`);

    const whole = appended(buildClaudeOptions(start({}), { ...deps, memory: { ...memory, cut: false } }));
    expect(whole).toContain("<user-memory");
    expect(whole).not.toContain("read the file for the rest");

    const none = buildClaudeOptions(start({ append_system_prompt: "Reply in Chauffeur." }), deps);
    expect(appended(none)).toBe("Reply in Chauffeur.");
    expect(JSON.stringify(none.settings ?? {})).not.toContain("memory");
  });
});
