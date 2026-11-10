// Selected native rollouts only. Call after source quiescence; hashes are not ownership,
// authentication or encryption. A daemon-owned caller must encrypt before transport.
import { readdir, realpath } from "node:fs/promises";
import { canonicalDirectory, readContextFile, restoreContextFiles } from "../context-files.ts";
import { createHash } from "node:crypto";
import { basename, isAbsolute, join, relative } from "node:path";

const MAX_FILE = 64 * 1024 * 1024;
const MAX_TOTAL = 256 * 1024 * 1024;
const MAX_FILES = 33;
const MAX_SCAN = 100_000;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const failure = () => new Error("Codex context is incomplete or unsafe; preserve the source and inspect its native history before retrying");
export type CodexContextFile = { path: string; content: string; sha256: string };
export type CodexContextBundle = { version: 1; sessionId: string; files: CodexContextFile[] };
type Rollout = { id: string; base?: { id: string; end: number }; bytes: Buffer };

function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
function digest(bytes: Buffer): string { return createHash("sha256").update(bytes).digest("hex"); }
function validPath(path: string): boolean {
  const parts = path.split("/");
  return path.length < 1024 && !path.includes("\\") && !path.includes("\0")
    && (parts[0] === "sessions" || parts[0] === "archived_sessions")
    && parts.length <= 6 && parts.every((part) => /^[a-zA-Z0-9_.-]+$/.test(part) && part !== "." && part !== "..")
    && /^rollout-.+\.jsonl$/.test(parts.at(-1) ?? "");
}
function parse(bytes: Buffer): Rollout {
  if (!bytes.length || bytes.length > MAX_FILE || bytes.at(-1) !== 10) throw failure();
  let lines: unknown[];
  try {
    const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    lines = text.trimEnd().split("\n").map((line): unknown => JSON.parse(line));
  } catch { throw failure(); } // JSON diagnostics must not quote private transcript bytes.
  const first = lines[0];
  if (!lines.every(object) || !object(first) || first.type !== "session_meta" || !object(first.payload)) throw failure();
  const meta = first.payload;
  if (typeof meta.id !== "string" || !UUID.test(meta.id)) throw failure();
  const base = meta.history_base;
  if (base === undefined || base === null) return { id: meta.id, bytes };
  if (!object(base) || typeof base.thread_id !== "string" || !UUID.test(base.thread_id)
    || typeof base.end_byte_offset !== "number" || !Number.isSafeInteger(base.end_byte_offset)
    || base.end_byte_offset <= 0 || base.end_byte_offset > MAX_FILE) throw failure();
  return { id: meta.id, bytes, base: { id: base.thread_id, end: base.end_byte_offset } };
}
/** Capture a selected stopped thread and only the exact prefixes of its ancestor rollouts. */
/** With `live`, the thread may be running: its rollout is taken up to its last complete line. */
export async function captureCodexContext(home: string, path: string, sessionId: string, live = false): Promise<CodexContextBundle> {
  await canonicalDirectory(home);
  if (!UUID.test(sessionId) || !isAbsolute(path)) throw failure();
  const selected = relative(home, path);
  if (!validPath(selected)) throw failure();
  const candidates = new Map<string, string[]>();
  let scanned = 0;
  async function scan(dir: string, depth: number): Promise<void> {
    let entries;
    try { entries = await readdir(join(home, dir), { withFileTypes: true }); }
    catch (error) { if (object(error) && error.code === "ENOENT" && depth === 0) return; throw error; }
    if (await realpath(join(home, dir)) !== join(home, dir)) throw failure();
    for (const entry of entries) {
      if (++scanned > MAX_SCAN || entry.isSymbolicLink()) throw failure();
      const child = `${dir}/${entry.name}`;
      if (entry.isDirectory()) {
        if (depth >= 4) throw failure();
        await scan(child, depth + 1);
      } else if (entry.isFile() && validPath(child)) {
        const id = basename(child).slice(-42, -6);
        if (UUID.test(id)) candidates.set(id, [...(candidates.get(id) ?? []), child]);
      }
    }
  }
  for (const dir of ["sessions", "archived_sessions"]) await scan(dir, 0);
  const files: CodexContextFile[] = [];
  const seen = new Set<string>();
  let total = 0;
  async function collect(file: string, id: string, end?: number): Promise<void> {
    if (seen.has(id) || files.length >= MAX_FILES) throw failure();
    seen.add(id);
    if (candidates.get(id)?.length !== 1 || candidates.get(id)?.[0] !== file) throw failure();
    const full = await readContextFile(home, file, live);
    if (end !== undefined && end > full.length) throw failure();
    const bytes = end === undefined ? full : full.subarray(0, end);
    const rollout = parse(bytes);
    if (rollout.id !== id || (total += bytes.length) > MAX_TOTAL) throw failure();
    files.push({ path: file, content: bytes.toString("base64"), sha256: digest(bytes) });
    if (rollout.base) {
      const parents = candidates.get(rollout.base.id);
      if (parents?.length !== 1) throw failure();
      await collect(parents[0]!, rollout.base.id, rollout.base.end);
    }
    if (!live && !(await readContextFile(home, file)).equals(full)) throw failure();
  }
  await collect(selected, sessionId);
  return { version: 1, sessionId, files };
}

function validate(value: unknown): Array<{ path: string; bytes: Buffer }> {
  if (!object(value) || value.version !== 1 || typeof value.sessionId !== "string" || !UUID.test(value.sessionId)
    || !Array.isArray(value.files) || !value.files.length || value.files.length > MAX_FILES) throw failure();
  const records = new Map<string, Rollout>();
  const files: Array<{ path: string; bytes: Buffer }> = [];
  const paths = new Set<string>();
  let total = 0;
  for (const entry of value.files) {
    if (!object(entry) || typeof entry.path !== "string" || !validPath(entry.path) || paths.has(entry.path)
      || typeof entry.content !== "string" || entry.content.length > Math.ceil(MAX_FILE / 3) * 4
      || typeof entry.sha256 !== "string" || !/^[a-f0-9]{64}$/.test(entry.sha256)) throw failure();
    const bytes = Buffer.from(entry.content, "base64");
    if (bytes.toString("base64") !== entry.content || digest(bytes) !== entry.sha256 || (total += bytes.length) > MAX_TOTAL) throw failure();
    const rollout = parse(bytes);
    if (!basename(entry.path).endsWith(`-${rollout.id}.jsonl`) || records.has(rollout.id)) throw failure();
    records.set(rollout.id, rollout);
    paths.add(entry.path);
    files.push({ path: entry.path, bytes });
  }
  const seen = new Set<string>();
  let id: string | undefined = value.sessionId;
  let end: number | undefined;
  while (id !== undefined) {
    const rollout = records.get(id);
    if (!rollout || seen.has(id) || (end !== undefined && rollout.bytes.length !== end)) throw failure();
    seen.add(id);
    end = rollout.base?.end;
    id = rollout.base?.id;
  }
  if (seen.size !== records.size) throw failure();
  return files;
}

/** Restore into a NEW private home only. Validates all context before creating any files. */
export async function restoreCodexContext(value: unknown, home: string): Promise<void> {
  await restoreContextFiles(validate(value), home);
}

/** Shared ancestor prefixes are merged only when the shorter is an exact prefix of the longer. */
export async function restoreCodexContexts(values: readonly unknown[], home: string): Promise<void> {
  await restoreContextFiles(codexContextFiles(values), home);
}

/** The validated rollouts of selected threads, relative to a Codex home; a rollout several
 * threads share is kept once, the longest when one is an exact prefix of the other. */
export function codexContextFiles(values: readonly unknown[]): Array<{ path: string; bytes: Buffer }> {
  const files = new Map<string, Buffer>();
  const paths = new Map<string, string>();
  let total = 0;
  for (const value of values) {
    for (const file of validate(value)) {
      const id = parse(file.bytes).id;
      const previousPath = paths.get(id);
      if (previousPath !== undefined && previousPath !== file.path) throw failure();
      paths.set(id, file.path);
      const previous = files.get(file.path);
      if (previous) {
        const shorter = previous.length < file.bytes.length ? previous : file.bytes;
        const longer = previous.length < file.bytes.length ? file.bytes : previous;
        if (!longer.subarray(0, shorter.length).equals(shorter)) throw failure();
        total -= previous.length;
        files.set(file.path, longer);
        total += longer.length;
      } else {
        files.set(file.path, file.bytes);
        total += file.bytes.length;
      }
      if (files.size > 4096 || total > MAX_TOTAL) throw failure();
    }
  }
  return [...files].map(([path, bytes]) => ({ path, bytes }));
}
