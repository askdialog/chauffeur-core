// Strict stopped-source capture, not the SDK's best-effort transcript mirror/import.
// Private plaintext: ownership, authentication and encryption remain caller prerequisites.
import { readdir } from "node:fs/promises";
import { createHash } from "node:crypto";
import { basename, join } from "node:path";
import { getSessionMessages, type SessionStore } from "@anthropic-ai/claude-agent-sdk";
import { canonicalDirectory, readContextFile, restoreContextFiles } from "../context-files.ts";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const MAX_FILE = 64 * 1024 * 1024;
const MAX_TOTAL = 256 * 1024 * 1024;
const MAX_FILES = 512;
const failure = () => new Error("Claude context is incomplete or unsupported; preserve the source and inspect its native history before retrying");
type ContextFile = { path: string; content: string; sha256: string };
export type ClaudeContextBundle = { version: 1; sessionId: string; sourceProjectKey: string; files: ContextFile[] };
function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
function projectKey(key: string): boolean { return /^[a-zA-Z0-9_-]{1,255}$/.test(key); }
function toolResult(path: string, id: string): boolean {
  const parts = path.split("/");
  return parts.length === 3 && parts[0] === id && parts[1] === "tool-results"
    && /^[a-zA-Z0-9_-]+\.txt$/.test(parts[2] ?? "") && path.length < 2048;
}
function validPath(path: string, id: string): boolean {
  if (path === `${id}.jsonl` || toolResult(path, id)) return true;
  const parts = path.split("/");
  return path.length < 2048 && parts.length <= 20 && parts[0] === id && parts[1] === "subagents"
    && parts.every((part) => /^[a-zA-Z0-9_.-]+$/.test(part) && part !== "." && part !== "..")
    && /^(agent-[a-zA-Z0-9_-]+|journal)(\.jsonl|\.meta\.json)$/.test(parts.at(-1) ?? "");
}
function checkRecords(path: string, bytes: Buffer, sessionId: string): void {
  if (!bytes.length || bytes.length > MAX_FILE) throw failure();
  // Persisted tool output is an opaque payload, not a native JSONL record.
  if (toolResult(path, sessionId)) return;
  try {
    const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    if (path.endsWith(".meta.json")) {
      const meta: unknown = JSON.parse(text);
      if (!object(meta)) throw failure();
    } else {
      if (bytes.at(-1) !== 10) throw failure();
      let ownMessage = false;
      for (const line of text.trimEnd().split("\n")) {
        const entry: unknown = JSON.parse(line);
        if (!object(entry) || typeof entry.type !== "string" || !entry.type) throw failure();
        if ((entry.type === "user" || entry.type === "assistant") && entry.sessionId === sessionId && object(entry.message)) ownMessage = true;
      }
      if (path === `${sessionId}.jsonl` && !ownMessage) throw failure();
    }
  } catch { throw failure(); } // Never quote a private native record in JSON diagnostics.
}
const digest = (bytes: Buffer) => createHash("sha256").update(bytes).digest("hex");
function validate(value: unknown): { id: string; files: Array<{ path: string; bytes: Buffer }> } {
  if (!object(value) || value.version !== 1 || typeof value.sessionId !== "string" || !UUID.test(value.sessionId)
    || typeof value.sourceProjectKey !== "string" || !projectKey(value.sourceProjectKey)
    || !Array.isArray(value.files) || !value.files.length || value.files.length > MAX_FILES) throw failure();
  const names = new Set<string>();
  const files: Array<{ path: string; bytes: Buffer }> = [];
  let total = 0;
  for (const entry of value.files) {
    if (!object(entry) || typeof entry.path !== "string" || !validPath(entry.path, value.sessionId) || names.has(entry.path)
      || typeof entry.content !== "string" || entry.content.length > Math.ceil(MAX_FILE / 3) * 4
      || typeof entry.sha256 !== "string" || !/^[a-f0-9]{64}$/.test(entry.sha256)) throw failure();
    const bytes = Buffer.from(entry.content, "base64");
    if (bytes.toString("base64") !== entry.content || digest(bytes) !== entry.sha256 || (total += bytes.length) > MAX_TOTAL) throw failure();
    checkRecords(entry.path, bytes, value.sessionId);
    names.add(entry.path);
    files.push({ path: entry.path, bytes });
  }
  if (!names.has(`${value.sessionId}.jsonl`)) throw failure();
  for (const name of names) {
    if (name.endsWith(".meta.json") && !names.has(name.replace(/\.meta\.json$/, ".jsonl"))) throw failure();
  }
  return { id: value.sessionId, files };
}

/** Resolve the pinned SDK's project-key codec without reading local sessions or invoking a model. */
export async function claudeProjectKey(cwd: string): Promise<string> {
  await canonicalDirectory(cwd);
  let key: string | undefined;
  const store: SessionStore = {
    async append() { throw failure(); },
    async load(selected) { key = selected.projectKey; return []; },
  };
  await getSessionMessages("11111111-1111-4111-8111-111111111111", { dir: cwd, sessionStore: store });
  if (!key || !projectKey(key)) throw failure();
  return key;
}

/** Read one stopped session, its subagents and persisted tool results, never a whole home. */
/** With `live`, the session may be running: each transcript is taken up to its last complete line. */
export async function captureClaudeContext(projects: string, key: string, sessionId: string, live = false): Promise<ClaudeContextBundle> {
  if (!projectKey(key) || !UUID.test(sessionId)) throw failure();
  await canonicalDirectory(projects);
  const root = join(projects, key);
  await canonicalDirectory(root);
  const paths = [`${sessionId}.jsonl`];
  let scanned = 0;
  async function scan(dir: string, depth: number): Promise<void> {
    let entries;
    try { entries = await readdir(join(root, dir), { withFileTypes: true }); }
    catch (error) { if (object(error) && error.code === "ENOENT" && depth === 0) return; throw error; }
    await canonicalDirectory(join(root, dir));
    for (const entry of entries) {
      if (++scanned > 4096 || entry.isSymbolicLink()) throw failure();
      const path = `${dir}/${entry.name}`;
      if (entry.isDirectory()) {
        if (depth >= 16 || dir === `${sessionId}/tool-results`
          || (depth === 0 && entry.name !== "subagents" && entry.name !== "tool-results")) throw failure();
        await scan(path, depth + 1);
      } else {
        if (!entry.isFile() || !validPath(path, sessionId) || paths.length >= MAX_FILES) throw failure();
        paths.push(path);
      }
    }
  }
  await scan(sessionId, 0);
  paths.sort();
  const files: ContextFile[] = [];
  let total = 0;
  for (const path of paths) {
    const bytes = await readContextFile(root, path, live);
    // A subagent's transcript with no complete record yet carries nothing; the main one must.
    if (live && !bytes.length && path !== `${sessionId}.jsonl`) continue;
    checkRecords(path, bytes, sessionId);
    if ((total += bytes.length) > MAX_TOTAL) throw failure();
    files.push({ path, content: bytes.toString("base64"), sha256: digest(bytes) });
  }
  const bundle: ClaudeContextBundle = { version: 1, sessionId, sourceProjectKey: key, files };
  validate(bundle);
  // A stopped trusted source is required. These rechecks detect changes, not arbitrary writers.
  if (!live) for (const file of files) {
    if (digest(await readContextFile(root, file.path)) !== file.sha256) throw failure();
  }
  return bundle;
}

/** Relocate native files into a NEW SDK project namespace; historical record contents stay unchanged. */
export async function restoreClaudeContext(value: unknown, projects: string, targetKey: string): Promise<void> {
  await restoreClaudeContexts([value], projects, targetKey);
}

/** Restore explicitly selected conversations together into one NEW native project namespace. */
export async function restoreClaudeContexts(values: readonly unknown[], projects: string, targetKey: string): Promise<void> {
  if (!projectKey(targetKey)) throw failure();
  await canonicalDirectory(projects);
  await restoreContextFiles(claudeContextFiles(values), join(projects, targetKey));
}

/** The validated files of selected conversations, relative to their project folder. */
export function claudeContextFiles(values: readonly unknown[]): Array<{ path: string; bytes: Buffer }> {
  if (!values.length || values.length > MAX_FILES) throw failure();
  const ids = new Set<string>();
  const names = new Set<string>();
  const files: Array<{ path: string; bytes: Buffer }> = [];
  let total = 0;
  for (const value of values) {
    const selected = validate(value);
    if (ids.has(selected.id)) throw failure();
    ids.add(selected.id);
    for (const file of selected.files) {
      if (names.has(file.path) || files.length >= MAX_FILES || (total += file.bytes.length) > MAX_TOTAL) throw failure();
      names.add(file.path);
      files.push(file);
    }
  }
  return files;
}

export function isClaudeProjectKey(key: string): boolean { return projectKey(key); }

/** Strict records for compaction/integrity checks; not a replacement for native resume. */
export function claudeMainRecords(value: unknown): Array<Record<string, unknown>> {
  const { id, files } = validate(value);
  const file = files.find((entry) => basename(entry.path) === `${id}.jsonl`)!;
  return file.bytes.toString("utf8").trimEnd().split("\n").map((line): Record<string, unknown> => JSON.parse(line));
}
