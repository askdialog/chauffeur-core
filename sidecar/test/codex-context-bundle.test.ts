import { afterEach, expect, test } from "bun:test";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync, realpathSync, existsSync, symlinkSync, statSync, truncateSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { createHash } from "node:crypto";
import { captureCodexContext, restoreCodexContext, restoreCodexContexts, type CodexContextBundle } from "../src/agents/codex/context-bundle.ts";
import { readThreadHistory } from "../src/agents/codex/history.ts";
const roots: string[] = [];
afterEach(() => { for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true }); });
const id = (n: number) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
const path = (n: number, archived = false) => `${archived ? "archived_sessions" : "sessions/2026/10/21"}/rollout-2026-10-21T00-00-00-${id(n)}.jsonl`;
const line = (value: unknown) => JSON.stringify(value) + "\n";
const meta = (n: number, base?: { thread_id: string; end_byte_offset: number }) => line({ type: "session_meta", payload: { id: id(n), cwd: "/historical/source", history_base: base } });
const item = (text: string) => line({ type: "response_item", payload: { type: "message", role: "user", content: text } });
function fixture() {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "chauffeur-context-")));
  roots.push(root);
  const home = join(root, "source"); mkdirSync(home, { mode: 0o700 });
  const write = (name: string, value: string) => { mkdirSync(dirname(join(home, name)), { recursive: true }); writeFileSync(join(home, name), value); };
  return { root, home, write };
}
function replace(bundle: CodexContextBundle, index: number, text: string) {
  const bytes = Buffer.from(text);
  bundle.files[index]!.content = bytes.toString("base64");
  bundle.files[index]!.sha256 = createHash("sha256").update(bytes).digest("hex");
}

test("restores nested parent prefixes and opaque compaction bytes without unrelated history or login files", async () => {
  const { root, home, write } = fixture();
  const grand = meta(1) + line({ type: "compacted", payload: { replacement_history: [{ type: "compaction", encrypted_content: "opaque-été" }] } });
  const parent = meta(2, { thread_id: id(1), end_byte_offset: Buffer.byteLength(grand) }) + item("parent");
  const child = meta(3, { thread_id: id(2), end_byte_offset: Buffer.byteLength(parent) }) + item("selected");
  write(path(1, true), grand + item("unselected grandparent continuation"));
  write(path(2), parent + item("unselected parent continuation"));
  write(path(3), child); write(path(4), meta(4) + item("unrelated private conversation"));
  write("auth.json", "private-token"); write("config.toml", "private-config");
  const bundle = await captureCodexContext(home, join(home, path(3)), id(3));
  expect(bundle.files.map((file) => file.path)).toEqual([path(3), path(2), path(1, true)]);
  expect(bundle.files.map((file) => Buffer.from(file.content, "base64").toString())).toEqual([child, parent, grand]);
  const restored = join(root, "restored"); await restoreCodexContext(JSON.parse(JSON.stringify(bundle)), restored);
  expect(await readThreadHistory(join(restored, path(3)))).toEqual(await readThreadHistory(join(home, path(3))));
  expect(readFileSync(join(restored, path(1, true)), "utf8")).toBe(grand);
  expect(existsSync(join(restored, "auth.json"))).toBe(false);
  expect(existsSync(join(restored, path(4)))).toBe(false);
  expect(statSync(restored).mode & 0o777).toBe(0o700);
  expect(statSync(join(restored, path(3))).mode & 0o777).toBe(0o600);
});
test("rejects damaged content, missing dependencies, extra threads and duplicate paths before creating a destination", async () => {
  const { root, home, write } = fixture();
  const parent = meta(1) + item("parent");
  write(path(1), parent); write(path(2), meta(2, { thread_id: id(1), end_byte_offset: Buffer.byteLength(parent) }));
  const source = await captureCodexContext(home, join(home, path(2)), id(2));
  const variants = [structuredClone(source), structuredClone(source), structuredClone(source), structuredClone(source)];
  variants[0]!.files[0]!.content = Buffer.from("corruption").toString("base64");
  variants[1]!.files.pop();
  variants[2]!.files.push({ path: path(3), content: Buffer.from(meta(3)).toString("base64"), sha256: createHash("sha256").update(meta(3)).digest("hex") });
  variants[3]!.files.push(variants[3]!.files[0]!);
  for (const [i, bundle] of variants.entries()) {
    const destination = join(root, `invalid-${i}`);
    await expect(restoreCodexContext(bundle, destination)).rejects.toThrow();
    expect(existsSync(destination)).toBe(false);
  }
});
test("rejects an overlong parent prefix even with valid component hashes", async () => {
  const { root, home, write } = fixture();
  const parent = meta(1);
  write(path(1), parent); write(path(2), meta(2, { thread_id: id(1), end_byte_offset: Buffer.byteLength(parent) }));
  const bundle = await captureCodexContext(home, join(home, path(2)), id(2));
  replace(bundle, 1, parent + item("must not export this continuation"));
  await expect(restoreCodexContext(bundle, join(root, "invalid"))).rejects.toThrow();
});
test("refuses cycles, missing parents, invalid byte boundaries and mismatched thread identities", async () => {
  const { home, write } = fixture();
  for (const base of [{ thread_id: id(2), end_byte_offset: 1 }, { thread_id: id(9), end_byte_offset: 10 }, { thread_id: id(1), end_byte_offset: 2 }, { thread_id: "../escape", end_byte_offset: 1 }]) {
    write(path(1), meta(1)); write(path(2), meta(2, base));
    await expect(captureCodexContext(home, join(home, path(2)), id(2))).rejects.toThrow();
  }
  write(path(2), meta(1));
  await expect(captureCodexContext(home, join(home, path(2)), id(2))).rejects.toThrow();
});
test("refuses duplicate ancestor candidates instead of choosing by directory order", async () => {
  const { home, write } = fixture(); const parent = meta(1);
  write(path(1), parent); write(path(1, true), parent);
  write(path(2), meta(2, { thread_id: id(1), end_byte_offset: Buffer.byteLength(parent) }));
  await expect(captureCodexContext(home, join(home, path(2)), id(2))).rejects.toThrow();
});
test("rejects symlink sources, unsafe restore paths and oversized native files", async () => {
  const { root, home, write } = fixture(); write(path(1), meta(1));
  const bundle = await captureCodexContext(home, join(home, path(1)), id(1));
  symlinkSync(home, join(root, "alias"));
  await expect(captureCodexContext(join(root, "alias"), join(root, "alias", path(1)), id(1))).rejects.toThrow();
  bundle.files[0]!.path = "sessions/../../auth.json";
  await expect(restoreCodexContext(bundle, join(root, "invalid"))).rejects.toThrow();
  truncateSync(join(home, path(1)), 64 * 1024 * 1024 + 1);
  await expect(captureCodexContext(home, join(home, path(1)), id(1))).rejects.toThrow();
});
test("never overwrites an existing destination or follows a transcript symlink", async () => {
  const { root, home, write } = fixture(); write(path(1), meta(1));
  const bundle = await captureCodexContext(home, join(home, path(1)), id(1));
  await expect(restoreCodexContext(bundle, home)).rejects.toThrow();
  expect(readFileSync(join(home, path(1)), "utf8")).toBe(meta(1));
  write("outside", meta(2)); symlinkSync(join(home, "outside"), join(home, path(2)));
  await expect(captureCodexContext(home, join(home, path(2)), id(2))).rejects.toThrow();
  expect(existsSync(join(root, "invalid"))).toBe(false);
});

test("rejects a structurally valid dependency cycle and suppresses private JSON diagnostics", async () => {
  const { root, home, write } = fixture();
  let cycle = meta(1, { thread_id: id(1), end_byte_offset: 1 });
  for (let i = 0; i < 4; i++) cycle = meta(1, { thread_id: id(1), end_byte_offset: Buffer.byteLength(cycle) });
  write(path(1), cycle);
  await expect(captureCodexContext(home, join(home, path(1)), id(1))).rejects.toThrow();
  const invalid = "private-transcript-secret\n";
  const bundle: CodexContextBundle = { version: 1, sessionId: id(1), files: [{ path: path(1), content: "", sha256: "" }] };
  replace(bundle, 0, invalid);
  await expect(restoreCodexContext(bundle, join(root, "invalid"))).rejects.toThrow("Codex context is incomplete or unsafe");
});


test("restores several forks sharing different exact prefixes without dropping either continuation", async () => {
  const { root, home, write } = fixture();
  const prefix = meta(1) + item("shared");
  const full = prefix + item("later parent message");
  write(path(1), full);
  write(path(2), meta(2, { thread_id: id(1), end_byte_offset: Buffer.byteLength(prefix) }) + item("fork two"));
  write(path(3), meta(3, { thread_id: id(1), end_byte_offset: Buffer.byteLength(full) }) + item("fork three"));
  const bundles = await Promise.all([2, 3].map((n) => captureCodexContext(home, join(home, path(n)), id(n))));
  const restored = join(root, "several");
  await restoreCodexContexts(bundles, restored);
  expect(readFileSync(join(restored, path(1)), "utf8")).toBe(full);
  expect(await readThreadHistory(join(restored, path(2)))).toEqual(await readThreadHistory(join(home, path(2))));
  expect(await readThreadHistory(join(restored, path(3)))).toEqual(await readThreadHistory(join(home, path(3))));
  const conflict = structuredClone(bundles[1]!);
  replace(conflict, 1, meta(1) + item("other") + item("later parent message"));
  // Preserve the valid native parent-byte reference while making its shared prefix disagree.
  const child = meta(3, { thread_id: id(1), end_byte_offset: Buffer.byteLength(Buffer.from(conflict.files[1]!.content, "base64")) }) + item("fork three");
  replace(conflict, 0, child);
  const bad = join(root, "conflicting");
  await expect(restoreCodexContexts([bundles[0], conflict], bad)).rejects.toThrow();
  expect(existsSync(bad)).toBe(false);
  const archived = structuredClone(bundles[0]!);
  archived.files[1]!.path = path(1, true);
  const ambiguous = join(root, "ambiguous");
  await expect(restoreCodexContexts([bundles[0], archived], ambiguous)).rejects.toThrow();
  expect(existsSync(ambiguous)).toBe(false);
});
