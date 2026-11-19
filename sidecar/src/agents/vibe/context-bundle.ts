// A Vibe conversation carried to another machine: its session folder in Vibe's store
// (`logs/session/unified/<id>`: journal, chunks, generations, CURRENT, meta.json) and the plugin
// environment it pins (`logs/session/plugins`: manifests, packages and blobs, named by their
// content digest), relative to the Vibe home (`VIBE_HOME`, else `~/.vibe`). Vibe reloads such a
// copy from another folder, without its session index.
//
// Unlike Claude's and Codex's transcripts, the session folder is rewritten as the conversation
// goes (CURRENT, meta.json; old generations are deleted), so a conversation coming back cannot be
// merged into the copy it left: the restoration sets that folder aside whole and puts the carried
// one in its place (docs/adr/mistral-vibe-third-agent.md); the caller deletes the folder set aside
// once the transfer is done, or puts it back. Plugin files never change for a digest: one already
// there is kept.
//
// Vibe hard-links a plugin package's files to their blobs: those are read whatever their links, and
// carried only when their content is the digest that names them.
//
// Call capture after the source stopped, but for a backup (`live`); hashes are not ownership,
// authentication or encryption: a daemon-owned caller encrypts before transport.

import { createHash, randomBytes } from "node:crypto";
import { lstat, mkdir, readdir, readFile, realpath, rename, rmdir } from "node:fs/promises";
import { dirname, join } from "node:path";
import { canonicalDirectory, readContextFile, type Added } from "../context-files.ts";

const MAX_FILE = 64 * 1024 * 1024;
const MAX_TOTAL = 256 * 1024 * 1024;
const MAX_FILES = 4096;
const MAX_DEPTH = 4;
const SESSION_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const DIGEST = /^[0-9a-f]{64}$/;
const SEGMENT = /^[A-Za-z0-9_.-]+$/;
const SESSIONS = "logs/session/unified";
const PLUGINS = "logs/session/plugins";
/** Where a session folder replaced by a carried one waits, inside the Vibe home. */
export const REPLACED = "logs/session/chauffeur-replaced";
const failure = () => new Error("Vibe context is incomplete or unsafe; preserve the source and inspect its sessions before retrying");

export type VibeContextFile = { path: string; content: string; sha256: string };
export type VibeContextBundle = { version: 1; sessionId: string; files: VibeContextFile[] };

function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
function digest(bytes: Buffer): string {
  return createHash("sha256").update(bytes).digest("hex");
}
function safeSegments(path: string): string[] | undefined {
  if (path.length > 1024 || path.includes("\\") || path.includes("\0")) return undefined;
  const parts = path.split("/");
  return parts.every((part) => SEGMENT.test(part) && part !== "." && part !== "..") ? parts : undefined;
}

/** Whether `path` (relative to the Vibe home) is a file of session `sessionId` or of a plugin. */
export function validVibePath(path: string, sessionId: string): boolean {
  const parts = safeSegments(path);
  if (!parts) return false;
  const session = `${SESSIONS}/${sessionId}/`;
  if (path.startsWith(session)) return parts.length - 4 >= 1 && parts.length - 4 <= MAX_DEPTH + 1;
  if (!path.startsWith(`${PLUGINS}/`)) return false;
  const [kind, prefix, name] = parts.slice(3);
  if (!kind || !prefix || !name || !DIGEST.test(name) || name.slice(0, 2) !== prefix) return false;
  if (kind === "manifests" || kind === "blobs") return parts.length === 6;
  return kind === "packages" && parts.length >= 7 && parts.length <= 6 + MAX_DEPTH;
}

/**
 * What the session's generations pin (`plugin_lock` of each `runtime-state.json`): its plugins'
 * digests, and the blobs holding the lock's snapshots.
 */
function pinnedPlugins(runtimeStates: Buffer[]): { plugins: string[]; snapshots: string[] } {
  const plugins = new Set<string>();
  const snapshots = new Set<string>();
  for (const bytes of runtimeStates) {
    let state: unknown;
    try {
      state = JSON.parse(bytes.toString("utf8"));
    } catch {
      throw failure(); // JSON diagnostics must not quote private session bytes.
    }
    const lock = object(state) ? state.plugin_lock : undefined;
    if (lock === undefined || lock === null) continue;
    if (!object(lock) || !Array.isArray(lock.plugins)) throw failure();
    for (const plugin of lock.plugins) {
      if (!object(plugin) || typeof plugin.content_digest !== "string" || !DIGEST.test(plugin.content_digest)) throw failure();
      plugins.add(plugin.content_digest);
    }
    if (lock.snapshot_digest !== undefined && lock.snapshot_digest !== null) {
      if (typeof lock.snapshot_digest !== "string" || !DIGEST.test(lock.snapshot_digest)) throw failure();
      snapshots.add(lock.snapshot_digest);
    }
  }
  return { plugins: [...plugins], snapshots: [...snapshots] };
}

/** The files of a manifest: each package entry, and the blob holding it. */
function manifestEntries(bytes: Buffer, pluginDigest: string): { path: string; sha256: string }[] {
  let manifest: unknown;
  try {
    manifest = JSON.parse(bytes.toString("utf8"));
  } catch {
    throw failure();
  }
  if (!object(manifest) || manifest.content_digest !== pluginDigest || !Array.isArray(manifest.entries)) throw failure();
  return manifest.entries.flatMap((entry) => {
    if (!object(entry) || entry.kind !== "file") return [];
    if (typeof entry.path !== "string" || !safeSegments(entry.path) || typeof entry.sha256 !== "string" || !DIGEST.test(entry.sha256)) throw failure();
    return [{ path: entry.path, sha256: entry.sha256 }];
  });
}

/** Captures session `sessionId` of the Vibe home `home`, with the plugins it pins. */
export async function captureVibeContext(home: string, sessionId: string, live = false): Promise<VibeContextBundle> {
  await canonicalDirectory(home);
  if (!SESSION_ID.test(sessionId)) throw failure();
  const files: VibeContextFile[] = [];
  let total = 0;
  /** `sha256`: the content a plugin file must have, which allows it to be hard-linked. */
  const take = async (path: string, sha256?: string): Promise<Buffer> => {
    if (!validVibePath(path, sessionId) || files.length >= MAX_FILES) throw failure();
    // A journal is read up to its last complete line: it grows while the session runs, and
    // Vibe leaves empty ones.
    const bytes = await readContextFile(home, path, live || path.endsWith(".jsonl"), sha256 !== undefined);
    if (sha256 !== undefined && digest(bytes) !== sha256) throw failure();
    if ((total += bytes.length) > MAX_TOTAL) throw failure();
    files.push({ path, content: bytes.toString("base64"), sha256: digest(bytes) });
    return bytes;
  };
  const runtimeStates: Buffer[] = [];
  const walk = async (dir: string, depth: number): Promise<void> => {
    const full = join(home, dir);
    if ((await realpath(full)) !== full) throw failure();
    for (const entry of await readdir(full, { withFileTypes: true })) {
      if (entry.isSymbolicLink()) throw failure();
      const child = `${dir}/${entry.name}`;
      if (entry.isDirectory()) {
        if (depth >= MAX_DEPTH) throw failure();
        await walk(child, depth + 1);
      } else if (entry.isFile()) {
        const bytes = await take(child);
        if (entry.name === "runtime-state.json") runtimeStates.push(bytes);
      }
    }
  };
  await walk(`${SESSIONS}/${sessionId}`, 0);
  const pinned = pinnedPlugins(runtimeStates);
  const blobs = new Set(pinned.snapshots);
  for (const plugin of pinned.plugins) {
    const prefix = plugin.slice(0, 2);
    const manifest = await take(`${PLUGINS}/manifests/${prefix}/${plugin}`);
    for (const entry of manifestEntries(manifest, plugin)) {
      await take(`${PLUGINS}/packages/${prefix}/${plugin}/${entry.path}`, entry.sha256);
      blobs.add(entry.sha256);
    }
  }
  for (const blob of blobs) await take(`${PLUGINS}/blobs/${blob.slice(0, 2)}/${blob}`, blob);
  return { version: 1, sessionId, files };
}

/** A carried conversation, checked whole: its paths, sizes, digests, and its blobs' content. */
export function vibeContextFiles(value: unknown): { sessionId: string; files: Array<{ path: string; bytes: Buffer }> } {
  if (!object(value) || value.version !== 1 || typeof value.sessionId !== "string" || !SESSION_ID.test(value.sessionId)
    || !Array.isArray(value.files) || !value.files.length || value.files.length > MAX_FILES) throw failure();
  const sessionId = value.sessionId;
  const files: Array<{ path: string; bytes: Buffer }> = [];
  const paths = new Set<string>();
  let total = 0;
  let inSession = 0;
  for (const entry of value.files) {
    if (!object(entry) || typeof entry.path !== "string" || !validVibePath(entry.path, sessionId) || paths.has(entry.path)
      || typeof entry.content !== "string" || entry.content.length > Math.ceil(MAX_FILE / 3) * 4
      || typeof entry.sha256 !== "string" || !DIGEST.test(entry.sha256)) throw failure();
    const bytes = Buffer.from(entry.content, "base64");
    if (bytes.toString("base64") !== entry.content || digest(bytes) !== entry.sha256 || (total += bytes.length) > MAX_TOTAL) throw failure();
    // A blob is named by its content.
    if (entry.path.startsWith(`${PLUGINS}/blobs/`) && entry.path.slice(entry.path.lastIndexOf("/") + 1) !== entry.sha256) throw failure();
    if (entry.path.startsWith(`${SESSIONS}/`)) inSession++;
    paths.add(entry.path);
    files.push({ path: entry.path, bytes });
  }
  if (!paths.has(`${SESSIONS}/${sessionId}/CURRENT`) || inSession === 0) throw failure();
  return { sessionId, files };
}

/** A session folder set aside for the carried one, to delete once done or to put back. */
export type Replaced = { path: string; aside: string };

/**
 * Sets the session's existing folder aside, if any, for the carried one to take its place.
 * `added` records the folders this makes, for `undoContextFiles`.
 */
export async function setAsideVibeSession(home: string, sessionId: string, added: Added): Promise<Replaced | undefined> {
  if (!SESSION_ID.test(sessionId)) throw failure();
  const path = join(home, SESSIONS, sessionId);
  let stat;
  try {
    stat = await lstat(path);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
    throw error;
  }
  if (!stat.isDirectory() || (await realpath(path)) !== path) throw failure();
  const folder = join(home, REPLACED);
  for (const dir of [join(home, "logs"), join(home, "logs", "session"), folder]) {
    try {
      await mkdir(dir, { mode: 0o700 });
      added.push({ path: dir, folder: true });
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
    }
    if ((await realpath(dir)) !== dir) throw failure();
  }
  const aside = join(folder, `${sessionId}-${randomBytes(6).toString("hex")}`);
  await rename(path, aside);
  return { path, aside };
}

/** Puts folders set aside back, newest first, once what took their place is gone. */
export async function putBackVibeSessions(replaced: readonly Replaced[]): Promise<void> {
  for (const { path, aside } of [...replaced].reverse()) {
    try {
      await rename(aside, path);
      // The folder for them, once empty, goes too.
      await rmdir(dirname(aside)).catch(() => undefined);
    } catch {
      // Left where it is: the error that brought us here is the one reported.
    }
  }
}

/**
 * The carried files still to write: a plugin file already in the home with the same content is
 * left out (Vibe makes its plugin store read-only, which adding to it would trip on); one with
 * other content refuses.
 */
export async function missingVibeFiles(home: string, files: ReadonlyArray<{ path: string; bytes: Buffer }>): Promise<Array<{ path: string; bytes: Buffer }>> {
  const missing: Array<{ path: string; bytes: Buffer }> = [];
  for (const file of files) {
    if (!file.path.startsWith(`${PLUGINS}/`)) {
      missing.push(file);
      continue;
    }
    const full = join(home, file.path);
    let stat;
    try {
      stat = await lstat(full);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
      missing.push(file);
      continue;
    }
    if (!stat.isFile() || (await realpath(full)) !== full || !(await readFile(full)).equals(file.bytes)) throw failure();
  }
  return missing;
}
