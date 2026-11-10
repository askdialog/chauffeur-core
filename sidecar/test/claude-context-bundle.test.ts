import { afterEach, expect, test } from "bun:test";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync, realpathSync, existsSync, symlinkSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { createHash } from "node:crypto";
import { captureClaudeContext, restoreClaudeContext, restoreClaudeContexts, claudeMainRecords, claudeProjectKey, type ClaudeContextBundle } from "../src/agents/claude/context-bundle.ts";
const roots: string[] = [];
afterEach(() => { for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true }); });
const id = "11111111-1111-4111-8111-111111111111";
const key = "-source-project";
const line = (entry: unknown) => JSON.stringify(entry) + "\n";
const main = line({ type: "user", uuid: "message-a", sessionId: id, message: { content: "source marker été" } })
  + line({ type: "system", subtype: "compact_boundary", opaque: "do not rewrite this" });
function fixture() {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "chauffeur-claude-context-"))); roots.push(root);
  const projects = join(root, "source-projects"); mkdirSync(projects, { mode: 0o700 });
  const write = (path: string, bytes: string | Buffer) => { const full = join(projects, key, path); mkdirSync(dirname(full), { recursive: true }); writeFileSync(full, bytes); };
  write(`${id}.jsonl`, main);
  return { root, projects, write };
}
function replace(bundle: ClaudeContextBundle, index: number, bytes: Buffer) {
  bundle.files[index]!.content = bytes.toString("base64");
  bundle.files[index]!.sha256 = createHash("sha256").update(bytes).digest("hex");
}

test("relocates only the selected native session and nested subagent metadata, preserving exact bytes", async () => {
  const { root, projects, write } = fixture();
  const sub = `${id}/subagents/agent-first/subagents/agent-second.jsonl`;
  const metadata = sub.replace(/\.jsonl$/, ".meta.json");
  const subBytes = line({ type: "assistant", message: { content: [{ type: "tool_result", content: "opaque result" }] } });
  write(sub, subBytes); write(metadata, '{"parentAgentId":"first","toolUseId":"tool-id"}');
  write("22222222-2222-4222-8222-222222222222.jsonl", line({ type: "user", text: "unrelated secret" }));
  write("auth.json", "private-token");
  const bundle = await captureClaudeContext(projects, key, id);
  expect(bundle.files.length).toBe(3);
  expect(claudeMainRecords(bundle)).toEqual(main.trimEnd().split("\n").map((entry) => JSON.parse(entry)));
  const target = join(root, "target-projects"); mkdirSync(target);
  await restoreClaudeContext(JSON.parse(JSON.stringify(bundle)), target, "-new-project");
  expect(readFileSync(join(target, "-new-project", `${id}.jsonl`), "utf8")).toBe(main);
  expect(readFileSync(join(target, "-new-project", sub), "utf8")).toBe(subBytes);
  expect(readFileSync(join(target, "-new-project", metadata), "utf8")).toBe('{"parentAgentId":"first","toolUseId":"tool-id"}');
  expect(existsSync(join(target, "-new-project", "auth.json"))).toBe(false);
  expect(statSync(join(target, "-new-project")).mode & 0o777).toBe(0o700);
  expect(statSync(join(target, "-new-project", sub)).mode & 0o777).toBe(0o600);
});
test("refuses malformed, truncated and invalid UTF-8 source records instead of skipping them", async () => {
  const { projects, write } = fixture();
  for (const bytes of [Buffer.from(main + 'private-transcript-secret\n'), Buffer.from(main.trimEnd()), Buffer.from([0xff, 10]), Buffer.from('{}\n')]) {
    write(`${id}.jsonl`, bytes);
    await expect(captureClaudeContext(projects, key, id)).rejects.toThrow("Claude context is incomplete or unsupported");
  }
});
test("carries persisted tool results with the selected session without interpreting their bytes", async () => {
  const { root, projects, write } = fixture();
  const path = `${id}/tool-results/toolu_result.txt`;
  const bytes = Buffer.from([0xff, 0, 10, 123, 34]);
  write(path, bytes);
  write(`${id}/tool-results/second.txt`, "a large tool result without a trailing newline".repeat(2000));
  const bundle = await captureClaudeContext(projects, key, id);
  expect(bundle.files.length).toBe(3);
  const target = join(root, "target-projects"); mkdirSync(target);
  await restoreClaudeContext(bundle, target, "-new-project");
  expect(readFileSync(join(target, "-new-project", path))).toEqual(bytes);
  expect(readFileSync(join(target, "-new-project", `${id}.jsonl`), "utf8")).toBe(main);
  expect(await captureClaudeContext(projects, key, id, true)).toEqual(bundle);
  expect(await captureClaudeContext(target, "-new-project", id)).toEqual({ ...bundle, sourceProjectKey: "-new-project" });
});
test("refuses unsupported selected-session files and metadata without a transcript", async () => {
  const first = fixture(); first.write(`${id}/unknown/result.txt`, "unknown session data");
  await expect(captureClaudeContext(first.projects, key, id)).rejects.toThrow();
  const second = fixture(); second.write(`${id}/subagents/agent-orphan.meta.json`, "{}");
  await expect(captureClaudeContext(second.projects, key, id)).rejects.toThrow();
});
test("refuses tool result symlinks, nested paths and corrupted bytes", async () => {
  const { root, projects, write } = fixture();
  const path = `${id}/tool-results/result.txt`;
  write(path, "persisted output");
  const bundle = await captureClaudeContext(projects, key, id);
  bundle.files.find((file) => file.path === path)!.content = Buffer.from("changed output").toString("base64");
  const target = join(root, "target"); mkdirSync(target);
  await expect(restoreClaudeContext(bundle, target, "-corrupt")).rejects.toThrow();
  expect(existsSync(join(target, "-corrupt"))).toBe(false);
  symlinkSync(join(projects, key, `${id}.jsonl`), join(projects, key, id, "tool-results", "link.txt"));
  await expect(captureClaudeContext(projects, key, id)).rejects.toThrow();
  const nested = fixture(); nested.write(`${id}/tool-results/nested/result.txt`, "output");
  await expect(captureClaudeContext(nested.projects, key, id)).rejects.toThrow();
});
test("validates all integrity and paths before creating the new project namespace", async () => {
  const { root, projects } = fixture(); const source = await captureClaudeContext(projects, key, id);
  const target = join(root, "target"); mkdirSync(target);
  const variants = [structuredClone(source), structuredClone(source), structuredClone(source), structuredClone(source), structuredClone(source)];
  variants[0]!.files[0]!.content = Buffer.from("corruption").toString("base64");
  variants[1]!.files[0]!.path = "../auth.json";
  variants[2]!.files.push(variants[2]!.files[0]!);
  variants[3]!.sessionId = "22222222-2222-4222-8222-222222222222";
  replace(variants[4]!, 0, Buffer.from("private-transcript-secret\n"));
  for (const [index, bundle] of variants.entries()) {
    await expect(restoreClaudeContext(bundle, target, `-invalid-${index}`)).rejects.toThrow();
    expect(existsSync(join(target, `-invalid-${index}`))).toBe(false);
  }
});
test("never overwrites a native project namespace, even with another session inside it", async () => {
  const { projects } = fixture(); const bundle = await captureClaudeContext(projects, key, id);
  await expect(restoreClaudeContext(bundle, projects, key)).rejects.toThrow();
  expect(readFileSync(join(projects, key, `${id}.jsonl`), "utf8")).toBe(main);
});
test("does not follow selected transcript, project or subagent symlinks", async () => {
  const { root, projects, write } = fixture();
  write(`${id}/subagents/agent-real.jsonl`, line({ type: "assistant" }));
  symlinkSync(join(projects, key, `${id}.jsonl`), join(projects, key, id, "subagents", "agent-link.jsonl"));
  await expect(captureClaudeContext(projects, key, id)).rejects.toThrow();
  symlinkSync(projects, join(root, "project-alias"));
  await expect(captureClaudeContext(join(root, "project-alias"), key, id)).rejects.toThrow();
});
test("asks the installed SDK for its project-key mapping, including a long cwd", async () => {
  const { root } = fixture();
  const cwd = join(root, "long-path-".repeat(15), "nested-".repeat(20)); mkdirSync(cwd, { recursive: true });
  const actual = await claudeProjectKey(cwd);
  expect(actual).toMatch(/^[a-zA-Z0-9_-]+$/);
  expect(actual.length).toBeLessThanOrEqual(255);
  expect(await claudeProjectKey(cwd)).toBe(actual);
  expect(await claudeProjectKey(root)).not.toBe(actual);
});

test("restores several explicitly selected conversations together without admitting duplicates", async () => {
  const { root, projects, write } = fixture();
  const other = "22222222-2222-4222-8222-222222222222";
  write(`${other}.jsonl`, line({ type: "user", sessionId: other, message: { content: "second conversation" } }));
  const first = await captureClaudeContext(projects, key, id);
  const second = await captureClaudeContext(projects, key, other);
  const target = join(root, "target"); mkdirSync(target);
  await expect(restoreClaudeContexts([first, first], target, "-duplicate")).rejects.toThrow();
  expect(existsSync(join(target, "-duplicate"))).toBe(false);
  await restoreClaudeContexts([first, second], target, "-batch");
  expect(readFileSync(join(target, "-batch", `${id}.jsonl`), "utf8")).toBe(main);
  expect(readFileSync(join(target, "-batch", `${other}.jsonl`), "utf8")).toContain("second conversation");
});

test("refuses a main transcript without a message belonging to the selected native session", async () => {
  const { projects, write } = fixture();
  write(`${id}.jsonl`, line({ type: "user", sessionId: "22222222-2222-4222-8222-222222222222", message: { content: "another session" } }));
  await expect(captureClaudeContext(projects, key, id)).rejects.toThrow();
  write(`${id}.jsonl`, line({ type: "system", sessionId: id }));
  await expect(captureClaudeContext(projects, key, id)).rejects.toThrow();
});
