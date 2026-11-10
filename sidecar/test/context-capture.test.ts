import { afterEach, expect, test } from "bun:test";
import { existsSync, mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync, realpathSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { createHash } from "node:crypto";
import { createDispatcher } from "../src/dispatch.ts";
import type { SessionManager } from "../src/sessions.ts";
import { claudeProjectKey } from "../src/agents/claude/context-bundle.ts";
import { findCodexRollout, nativeHome } from "../src/agents/context-capture.ts";

const roots: string[] = [];
afterEach(() => { for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true }); });
const line = (value: unknown) => JSON.stringify(value) + "\n";
const claudeId = "11111111-1111-4111-8111-111111111111";
const codexId = "00000000-0000-4000-8000-000000000003";
const rollout = (id: string) => `sessions/2026/10/21/rollout-2026-10-21T00-00-00-${id}.jsonl`;

function fixture() {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "chauffeur-capture-"))); roots.push(root);
  const write = (path: string, text: string) => { mkdirSync(dirname(join(root, path)), { recursive: true }); writeFileSync(join(root, path), text); };
  const cwd = join(root, "workspace"); mkdirSync(cwd);
  const out = join(root, "package"); mkdirSync(out, { mode: 0o700 });
  // Only `has` is reached by context.capture: a chat is live or it is not.
  const sessions = { has: (chat: string) => chat === "live-chat" } as unknown as SessionManager;
  const dispatch = createDispatcher(sessions, { HOME: join(root, "home") });
  return { root, write, cwd, out, dispatch };
}
const params = (f: ReturnType<typeof fixture>, extra: Record<string, unknown>) =>
  ({ chat_id: "stopped-chat", cwd: f.cwd, env: {}, ...extra });

test("writes a stopped Claude chat's session from the chat's own config folder into a new private file", async () => {
  const f = fixture();
  const key = await claudeProjectKey(f.cwd);
  const main = line({ type: "user", sessionId: claudeId, message: { content: "marker" } });
  f.write(`claude-config/projects/${key}/${claudeId}.jsonl`, main);
  f.write(`home/.claude/projects/${key}/${claudeId}.jsonl`, line({ type: "user", sessionId: claudeId, message: { content: "the wrong home" } }));
  const output = join(f.out, "claude.json");
  const result = await f.dispatch("context.capture", params(f, {
    agent: "claude", session_id: claudeId, output, env: { CLAUDE_CONFIG_DIR: join(f.root, "claude-config") },
  }));
  const bytes = readFileSync(output);
  expect(result).toEqual({ outcome: "captured", sha256: createHash("sha256").update(bytes).digest("hex"), bytes: bytes.length });
  const bundle = JSON.parse(bytes.toString());
  expect(bundle.sessionId).toBe(claudeId);
  expect(Buffer.from(bundle.files[0].content, "base64").toString()).toBe(main);
  expect(statSync(output).mode & 0o777).toBe(0o600);
});

test("finds a Codex thread's rollout by its id, archived or not, and refuses a missing or duplicated one", async () => {
  const f = fixture();
  const home = join(f.root, "home", ".codex");
  expect(nativeHome("codex", { HOME: join(f.root, "home") })).toBe(home);
  await expect(findCodexRollout(home, codexId)).rejects.toThrow("not found exactly once");
  f.write(`home/.codex/${rollout(codexId)}`, line({ type: "session_meta", payload: { id: codexId } }));
  expect(await findCodexRollout(home, codexId)).toBe(join(home, rollout(codexId)));
  const output = join(f.out, "codex.json");
  await f.dispatch("context.capture", params(f, { agent: "codex", session_id: codexId, output }));
  expect(JSON.parse(readFileSync(output, "utf8")).files.map((file: { path: string }) => file.path)).toEqual([rollout(codexId)]);
  f.write(`home/.codex/archived_sessions/rollout-2026-10-22T00-00-00-${codexId}.jsonl`, "{}\n");
  await expect(findCodexRollout(home, codexId)).rejects.toThrow("not found exactly once");
});

test("refuses a chat with a running session, and never replaces an existing file", async () => {
  const f = fixture();
  f.write(`home/.codex/${rollout(codexId)}`, line({ type: "session_meta", payload: { id: codexId } }));
  const output = join(f.out, "codex.json");
  expect(() => f.dispatch("context.capture", params(f, { chat_id: "live-chat", agent: "codex", session_id: codexId, output })))
    .toThrow("has a running session");
  writeFileSync(output, "earlier");
  await expect(f.dispatch("context.capture", params(f, { agent: "codex", session_id: codexId, output }))).rejects.toThrow();
  expect(readFileSync(output, "utf8")).toBe("earlier");
});

test("a conversation whose history is missing is blocked with nothing written, not an error", async () => {
  const f = fixture();
  const output = join(f.out, "codex.json");
  expect(await f.dispatch("context.capture", params(f, { agent: "codex", session_id: codexId, output })))
    .toEqual({ outcome: "blocked", reason: expect.stringContaining("not found exactly once") });
  expect(existsSync(output)).toBe(false);
  await expect(f.dispatch("context.capture", params(f, { agent: "codex", session_id: codexId, output: "relative.json" })))
    .rejects.toThrow("new absolute path");
});

test("restores captured sessions into the destination's own homes, adding files and never replacing one", async () => {
  const f = fixture();
  const sourceKey = await claudeProjectKey(f.cwd);
  const main = line({ type: "user", sessionId: claudeId, message: { content: "native marker" } });
  f.write(`home/.claude/projects/${sourceKey}/${claudeId}.jsonl`, main);
  const toolPath = `${claudeId}/tool-results/result.txt`;
  const toolOutput = "persisted tool output\n".repeat(4000);
  f.write(`home/.claude/projects/${sourceKey}/${toolPath}`, toolOutput);
  const codex = line({ type: "session_meta", payload: { id: codexId } });
  f.write(`home/.codex/${rollout(codexId)}`, codex);
  const destination = join(f.root, "destination"); mkdirSync(destination);
  const entries = [];
  for (const [agent, session_id] of [["claude", claudeId], ["codex", codexId]] as const) {
    const path = join(f.out, `${agent}.json`);
    await f.dispatch("context.capture", params(f, { agent, session_id, output: path }));
    const bytes = readFileSync(path);
    entries.push({ agent, session_id, cwd: destination, path, sha256: createHash("sha256").update(bytes).digest("hex"), bytes: bytes.length });
  }
  // The destination machine: no Claude home yet, a Codex home with its own history.
  const claude_home = join(f.root, "machine", ".claude");
  const codex_home = join(f.root, "machine", ".codex");
  f.write("machine/.codex/sessions/2026/10/20/rollout-own.jsonl", "the machine's own\n");
  const homes = { claude_home, codex_home };
  // A fork can carry the same Claude ancestor; it must be installed once.
  const added = await f.dispatch("context.restore", { ...homes, contexts: [...entries, entries[0]] });
  const targetKey = await claudeProjectKey(destination);
  const restored = join(claude_home, "projects", targetKey, `${claudeId}.jsonl`);
  const restoredTool = join(claude_home, "projects", targetKey, toolPath);
  // What it created is named, so that discarding the copy removes exactly that: not the Claude
  // home it had to make, nor the Codex home's own folders.
  expect(added).toEqual({
    files: [restored, restoredTool, join(codex_home, rollout(codexId))],
    folders: [join(claude_home, "projects"), join(claude_home, "projects", targetKey),
      join(claude_home, "projects", targetKey, claudeId), join(claude_home, "projects", targetKey, claudeId, "tool-results"),
      join(codex_home, "sessions/2026/10/21")],
  });
  expect(readFileSync(restored, "utf8")).toBe(main);
  expect(readFileSync(restoredTool, "utf8")).toBe(toolOutput);
  expect(readFileSync(join(codex_home, rollout(codexId)), "utf8")).toBe(codex);
  expect(readFileSync(join(codex_home, "sessions/2026/10/20/rollout-own.jsonl"), "utf8")).toBe("the machine's own\n");
  expect(statSync(claude_home).mode & 0o777).toBe(0o700);
  // A retry finds the same files and succeeds without touching them, adding nothing.
  expect(await f.dispatch("context.restore", { ...homes, contexts: entries })).toEqual({ files: [], folders: [] });
  // A Codex thread the machine continued since is kept as it is.
  writeFileSync(join(codex_home, rollout(codexId)), codex + line({ type: "event_msg", payload: { later: true } }));
  await f.dispatch("context.restore", { ...homes, contexts: entries });
  expect(readFileSync(join(codex_home, rollout(codexId)), "utf8").startsWith(codex + "{")).toBe(true);
  // A conversation that comes back longer than this machine's copy (it went on elsewhere) is
  // extended; a later refusal in the same restoration cuts it back to what it was.
  const head = main.slice(0, 20);
  writeFileSync(restored, head);
  expect(await f.dispatch("context.restore", { ...homes, contexts: entries })).toEqual({ files: [], folders: [] });
  expect(readFileSync(restored, "utf8")).toBe(main);
  writeFileSync(restored, head);
  const rolloutPath = join(codex_home, rollout(codexId));
  const kept = readFileSync(rolloutPath);
  writeFileSync(rolloutPath, "a different thread\n");
  await expect(f.dispatch("context.restore", { ...homes, contexts: entries })).rejects.toThrow();
  expect(readFileSync(restored, "utf8")).toBe(head);
  writeFileSync(rolloutPath, kept);
  writeFileSync(restored, main);
  // A Codex-only restoration never touches the Claude home, even an unusable one.
  writeFileSync(join(f.root, "not-a-folder"), "");
  await f.dispatch("context.restore", { claude_home: join(f.root, "not-a-folder"), codex_home, contexts: [entries[1]] });
  // A different file where a conversation goes is refused, and what the attempt added is taken back.
  rmSync(join(claude_home, "projects"), { recursive: true });
  writeFileSync(join(codex_home, rollout(codexId)), "another thread\n");
  await expect(f.dispatch("context.restore", { ...homes, contexts: entries })).rejects.toThrow();
  expect(readFileSync(join(codex_home, rollout(codexId)), "utf8")).toBe("another thread\n");
  expect(existsSync(join(claude_home, "projects"))).toBe(false);
  expect(readFileSync(join(codex_home, "sessions/2026/10/20/rollout-own.jsonl"), "utf8")).toBe("the machine's own\n");
  const bad = { claude_home: join(f.root, "bad", ".claude"), codex_home: join(f.root, "bad", ".codex") };
  await expect(f.dispatch("context.restore", { ...bad, contexts: [{ ...entries[0], sha256: "0".repeat(64) }] })).rejects.toThrow();
  expect(existsSync(join(f.root, "bad"))).toBe(false);
  await expect(f.dispatch("context.restore", { claude_home: "relative", codex_home, contexts: entries })).rejects.toThrow();
});

test("a backup takes a running chat's transcript up to its last complete line", async () => {
  const f = fixture();
  const key = await claudeProjectKey(f.cwd);
  const done = line({ type: "user", sessionId: claudeId, message: { content: "marker" } });
  // The agent is writing its next record: half of it is on disk.
  f.write(`home/.claude/projects/${key}/${claudeId}.jsonl`, done + '{"type":"assistant","sessionId":"');
  const output = join(f.out, "claude.json");
  const ask = { chat_id: "live-chat", agent: "claude", session_id: claudeId, output };
  expect(() => f.dispatch("context.capture", params(f, ask))).toThrow("has a running session");
  expect(await f.dispatch("context.capture", params(f, { ...ask, live: true })))
    .toEqual(expect.objectContaining({ outcome: "captured" }));
  const bundle = JSON.parse(readFileSync(output, "utf8"));
  expect(Buffer.from(bundle.files[0].content, "base64").toString()).toBe(done);
});

test("a backup carries a subagent transcript with no complete line yet as nothing, and a main one as blocked", async () => {
  const f = fixture();
  const key = await claudeProjectKey(f.cwd);
  const done = line({ type: "user", sessionId: claudeId, message: { content: "marker" } });
  f.write(`home/.claude/projects/${key}/${claudeId}.jsonl`, done);
  f.write(`home/.claude/projects/${key}/${claudeId}/subagents/agent-a1.jsonl`, '{"type":"assistant"');
  const output = join(f.out, "claude.json");
  const ask = { chat_id: "live-chat", agent: "claude", session_id: claudeId, output, live: true };
  expect(await f.dispatch("context.capture", params(f, ask))).toEqual(expect.objectContaining({ outcome: "captured" }));
  expect(JSON.parse(readFileSync(output, "utf8")).files.map((file: { path: string }) => file.path)).toEqual([`${claudeId}.jsonl`]);
  f.write(`home/.claude/projects/${key}/${claudeId}.jsonl`, '{"type":"user"');
  expect(await f.dispatch("context.capture", params(f, { ...ask, output: join(f.out, "again.json") })))
    .toEqual(expect.objectContaining({ outcome: "blocked" }));
});
