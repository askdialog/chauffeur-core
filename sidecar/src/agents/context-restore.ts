// Restores carried conversations into the native homes the daemon names (the destination's own),
// adding files only: an existing file is never replaced. Executes no session.
import { createHash } from "node:crypto";
import { constants } from "node:fs";
import { mkdir, open } from "node:fs/promises";
import { dirname, isAbsolute, resolve } from "node:path";
import type { ContextRestoreParams, ContextRestoreResult } from "../protocol.ts";
import { addContextFiles, canonicalDirectory, undoContextFiles, type Added } from "./context-files.ts";
import { claudeContextFiles, claudeProjectKey, isClaudeProjectKey } from "./claude/context-bundle.ts";
import { codexContextFiles } from "./codex/context-bundle.ts";

const MAX_BYTES = 512 * 1024 * 1024;
const failure = () => new Error("Native restoration is incomplete or unsafe; retain the package and inspect the native homes before retrying");

/** Returns what it created, so that discarding the copy later removes exactly that. */
export async function restoreContexts(params: ContextRestoreParams): Promise<ContextRestoreResult> {
  for (const home of [params.claude_home, params.codex_home]) {
    if (!isAbsolute(home) || resolve(home) !== home) throw failure();
  }
  const claude = new Map<string, Map<string, { serialized: string; value: unknown }>>();
  const codex: unknown[] = [];
  let total = 0;
  for (const entry of params.contexts) {
    if (!Number.isSafeInteger(entry.bytes) || entry.bytes <= 0 || (total += entry.bytes) > MAX_BYTES
      || !/^[a-f0-9]{64}$/.test(entry.sha256)) throw failure();
    await canonicalDirectory(dirname(entry.path));
    const file = await open(entry.path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
    let bytes: Buffer;
    try {
      const stat = await file.stat();
      if (!stat.isFile() || stat.size !== entry.bytes || stat.nlink !== 1) throw failure();
      bytes = Buffer.alloc(entry.bytes + 1);
      let offset = 0;
      while (offset < bytes.length) {
        const read = await file.read(bytes, offset, bytes.length - offset, offset);
        if (!read.bytesRead) break;
        offset += read.bytesRead;
      }
      if (offset !== entry.bytes) throw failure();
      bytes = bytes.subarray(0, offset);
    } finally { await file.close(); }
    if (createHash("sha256").update(bytes).digest("hex") !== entry.sha256) throw failure();
    let value: unknown;
    try { value = JSON.parse(bytes.toString("utf8")); } catch { throw failure(); }
    if (value === null || typeof value !== "object" || !("sessionId" in value) || value.sessionId !== entry.session_id) throw failure();
    if (entry.agent === "claude") {
      const key = await claudeProjectKey(entry.cwd);
      const sessions = claude.get(key) ?? new Map<string, { serialized: string; value: unknown }>();
      const serialized = bytes.toString("utf8");
      const previous = sessions.get(entry.session_id);
      if (previous && previous.serialized !== serialized) throw failure();
      sessions.set(entry.session_id, { serialized, value });
      claude.set(key, sessions);
    } else if (entry.agent === "codex") codex.push(value);
    else throw failure();
  }
  // Everything is validated before the first file is written; a failure takes back what it added.
  const added: Added = [];
  try {
    if (claude.size) await home(params.claude_home, added);
    for (const [key, sessions] of claude) {
      if (!isClaudeProjectKey(key)) throw failure();
      const files = claudeContextFiles([...sessions.values()].map((session) => session.value));
      await addContextFiles(files.map((file) => ({ ...file, path: `projects/${key}/${file.path}` })), params.claude_home, added);
    }
    if (codex.length) await addContextFiles(codexContextFiles(codex), await home(params.codex_home, added), added);
  } catch (error) {
    await undoContextFiles(added);
    throw error;
  }
  return {
    files: added.filter((entry) => !entry.folder && entry.extendedFrom === undefined).map((entry) => entry.path),
    folders: added.filter((entry) => entry.folder && !entry.home).map((entry) => entry.path),
  };
}

/** A native home, created private when the machine has none yet. */
async function home(path: string, added: Added): Promise<string> {
  const missing: string[] = [];
  for (let dir = path; ; dir = dirname(dir)) {
    try { await canonicalDirectory(dir); break; }
    catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT" || dirname(dir) === dir) throw failure();
      missing.unshift(dir);
    }
  }
  for (const dir of missing) { await mkdir(dir, { mode: 0o700 }); added.push({ path: dir, folder: true, home: true }); }
  return path;
}
