import { afterEach, expect, test } from "bun:test";
import { existsSync, linkSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { claudeProjectKey } from "../src/agents/claude/context-bundle.ts";
import { createDispatcher } from "../src/dispatch.ts";
import type { SessionManager } from "../src/sessions.ts";

const SESSION = "11111111-1111-4111-8111-111111111111";
const roots: string[] = [];
afterEach(() => { for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true }); });

/** Claude Code's folder with one session kept for a folder gone since, and the agents' Claude folder. */
function fixture() {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "chauffeur-copy-"))); roots.push(root);
  const source = join(root, "claude-code", "projects", "-clone--wt-removed");
  const write = (path: string, text: string | Buffer) => { mkdirSync(dirname(join(source, path)), { recursive: true }); writeFileSync(join(source, path), text); };
  const exploration = join(root, "exploration"); mkdirSync(exploration);
  const dispatch = createDispatcher({ has: () => false } as unknown as SessionManager, {});
  const copy = () => dispatch("context.copy", {
    session_id: SESSION, source_home: join(root, "claude-code"), project_key: "-clone--wt-removed",
    claude_home: join(root, "agents-claude"), cwd: exploration,
  }) as Promise<{ files: string[]; folders: string[] }>;
  return { root, source, write, exploration, copy };
}

test("copies a session whatever its size, its whole folder with it, beside the sessions already there", async () => {
  const f = fixture();
  // Over capture's 64 MB limit.
  const transcript = Buffer.alloc(65 * 1024 * 1024, "{\"type\":\"user\"}\n");
  f.write(`${SESSION}.jsonl`, transcript);
  const own = {
    [`${SESSION}/subagents/agent-a1.jsonl`]: "{\"type\":\"assistant\"}\n",
    [`${SESSION}/tool-results/mcp-figma-shot.png`]: "png",
    [`${SESSION}/custom-title.json`]: "{\"title\":\"x\"}",
    [`${SESSION}/workflows/scripts/phase1.js`]: "run()",
  };
  for (const [path, text] of Object.entries(own)) f.write(path, text);
  const target = join(f.root, "agents-claude", "projects", await claudeProjectKey(f.exploration));
  mkdirSync(target, { recursive: true });
  writeFileSync(join(target, "22222222-2222-4222-8222-222222222222.jsonl"), "earlier\n");

  const added = await f.copy();
  expect(added.files.sort()).toEqual([`${SESSION}.jsonl`, ...Object.keys(own)].map((path) => join(target, path)).sort());
  expect(readFileSync(join(target, `${SESSION}.jsonl`)).equals(transcript)).toBe(true);
  for (const [path, text] of Object.entries(own)) expect(readFileSync(join(target, path), "utf8")).toBe(text);
  expect(statSync(join(target, `${SESSION}.jsonl`)).mode & 0o777).toBe(0o600);
  expect(readFileSync(join(target, "22222222-2222-4222-8222-222222222222.jsonl"), "utf8")).toBe("earlier\n");
  // Copied, not moved.
  expect(readFileSync(join(f.source, `${SESSION}.jsonl`)).equals(transcript)).toBe(true);

  // Again: what is there is kept, nothing added.
  expect(await f.copy()).toEqual({ files: [], folders: [] });
  // A copy cut short is completed.
  writeFileSync(join(target, `${SESSION}.jsonl`), transcript.subarray(0, 1000));
  expect(await f.copy()).toEqual({ files: [], folders: [] });
  expect(readFileSync(join(target, `${SESSION}.jsonl`)).equals(transcript)).toBe(true);
});

test("never replaces a different file, and a refused copy takes back what it added", async () => {
  const f = fixture();
  f.write(`${SESSION}.jsonl`, "{\"type\":\"user\"}\n");
  f.write(`${SESSION}/subagents/agent-a1.jsonl`, "agent\n");
  const target = join(f.root, "agents-claude", "projects", await claudeProjectKey(f.exploration));
  mkdirSync(target, { recursive: true });
  // Sorted, the transcript comes first: added, then taken back when the subagent's is refused.
  mkdirSync(join(target, SESSION, "subagents"), { recursive: true });
  writeFileSync(join(target, SESSION, "subagents", "agent-a1.jsonl"), "another agent\n");
  await expect(f.copy()).rejects.toThrow(`another file is where ${SESSION}/subagents/agent-a1.jsonl goes`);
  expect(readFileSync(join(target, SESSION, "subagents", "agent-a1.jsonl"), "utf8")).toBe("another agent\n");
  expect(existsSync(join(target, `${SESSION}.jsonl`))).toBe(false);
});

test("refuses a link in the session, a missing transcript and an id that is no session's", async () => {
  const f = fixture();
  await expect(f.copy()).rejects.toThrow("-clone--wt-removed is missing");
  mkdirSync(f.source, { recursive: true });
  await expect(f.copy()).rejects.toThrow("its transcript is missing");
  f.write(`${SESSION}.jsonl`, "{}\n");
  f.write("elsewhere.txt", "secret");
  mkdirSync(join(f.source, SESSION), { recursive: true });
  symlinkSync(join(f.source, "elsewhere.txt"), join(f.source, SESSION, "link.txt"));
  await expect(f.copy()).rejects.toThrow("is a link or not a file");
  rmSync(join(f.source, SESSION, "link.txt"));
  linkSync(join(f.source, "elsewhere.txt"), join(f.source, SESSION, "hard.txt"));
  await expect(f.copy()).rejects.toThrow("hard.txt is not a file of its own");
  rmSync(join(f.source, SESSION, "hard.txt"));
  // The session's folder itself a link.
  rmSync(join(f.source, SESSION), { recursive: true });
  mkdirSync(join(f.root, "elsewhere"));
  writeFileSync(join(f.root, "elsewhere", "secret.txt"), "secret");
  symlinkSync(join(f.root, "elsewhere"), join(f.source, SESSION));
  await expect(f.copy()).rejects.toThrow("is a link or not a folder");
  const dispatch = createDispatcher({ has: () => false } as unknown as SessionManager, {});
  await expect(dispatch("context.copy", {
    session_id: "../x", source_home: join(f.root, "claude-code"), project_key: "-clone--wt-removed",
    claude_home: join(f.root, "agents-claude"), cwd: f.exploration,
  }) as Promise<unknown>).rejects.toThrow("not a Claude Code session id");
});
