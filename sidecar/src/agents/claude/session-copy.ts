// A Claude Code session copied, never moved, into the project folder of another folder of a Claude
// home: its transcript and everything in its own folder (subagents, tool outputs, title…),
// streamed whatever their size (an import from Claude Code). Unlike a capture, nothing is held in
// memory, so no size limit. The source must hold still and hold no link; a file already where a
// copy goes is kept when it starts with the copy's bytes (extended when shorter), any other file
// in the way refuses, and a refused copy takes back what it added.
import { constants } from "node:fs";
import { lstat, mkdir, open, readdir, realpath, type FileHandle } from "node:fs/promises";
import { dirname, isAbsolute, join, resolve } from "node:path";
import type { ContextCopyParams, ContextRestoreResult } from "../../protocol.ts";
import { undoContextFiles, type Added } from "../context-files.ts";
import { claudeProjectKey, isClaudeProjectKey } from "./context-bundle.ts";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const CHUNK = 1024 * 1024;
const refused = (why: string) => new Error(`The session could not be copied: ${why}`);

export async function copyClaudeSession(params: ContextCopyParams): Promise<ContextRestoreResult> {
  if (!isAbsolute(params.source_home) || !isAbsolute(params.claude_home) || resolve(params.claude_home) !== params.claude_home) {
    throw refused("a Claude folder is not named from the root");
  }
  if (!UUID.test(params.session_id)) throw refused("its id is not a Claude Code session id");
  if (!isClaudeProjectKey(params.project_key)) throw refused("its project folder's name is not one Claude Code writes");
  // The user's own folder, read only: wherever its links lead (a `projects` kept on another disk).
  const named = join(params.source_home, "projects", params.project_key);
  const source = await realpath(named).catch(() => { throw refused(`${named} is missing`); });
  if (!(await lstat(source)).isDirectory()) throw refused(`${named} is not a folder`);
  const files = await sessionFiles(source, params.session_id);
  const target = join(params.claude_home, "projects", await claudeProjectKey(params.cwd));
  const added: Added = [];
  try {
    for (const path of files) await copyFile(join(source, path), target, path, added);
  } catch (error) {
    await undoContextFiles(added);
    throw error;
  }
  return {
    files: added.filter((entry) => !entry.folder && entry.extendedFrom === undefined).map((entry) => entry.path),
    folders: added.filter((entry) => entry.folder).map((entry) => entry.path),
  };
}

/** The session's files, relative to its project folder: `<id>.jsonl` and all of `<id>/`. */
async function sessionFiles(source: string, sessionId: string): Promise<string[]> {
  const main = `${sessionId}.jsonl`;
  const stat = await lstat(join(source, main)).catch(() => null);
  if (!stat?.isFile()) throw refused("its transcript is missing");
  const paths = [main];
  async function scan(dir: string): Promise<void> {
    // The session's folder itself may be a link: refused as any inside it.
    const stat = await lstat(join(source, dir)).catch((error: NodeJS.ErrnoException) => { if (error.code === "ENOENT") return null; throw error; });
    if (!stat && dir === sessionId) return;
    if (!stat?.isDirectory()) throw refused(`${dir} is a link or not a folder`);
    const entries = await readdir(join(source, dir), { withFileTypes: true });
    for (const entry of entries) {
      const path = `${dir}/${entry.name}`;
      if (entry.isDirectory()) await scan(path);
      else if (entry.isFile()) paths.push(path);
      else throw refused(`${path} is a link or not a file`);
    }
  }
  await scan(sessionId);
  return paths.sort();
}

/** One file streamed from `from` to `<target>/<path>`, its missing folders made. */
async function copyFile(from: string, target: string, path: string, added: Added): Promise<void> {
  const destination = join(target, path);
  const missing: string[] = [];
  for (let dir = dirname(destination); ; dir = dirname(dir)) {
    const stat = await lstat(dir).catch((error: NodeJS.ErrnoException) => { if (error.code === "ENOENT") return null; throw error; });
    if (stat) { if (!stat.isDirectory()) throw refused(`${dir} is not a folder`); break; }
    missing.unshift(dir);
  }
  for (const dir of missing) { await mkdir(dir, { mode: 0o700 }); added.push({ path: dir, folder: true }); }
  if (await realpath(dirname(destination)) !== dirname(destination)) throw refused(`${dirname(destination)} goes through a link`);
  const input = await open(from, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    const before = await input.stat();
    // A file hard-linked in is another's too: the capture refuses it alike.
    if (!before.isFile() || before.nlink !== 1) throw refused(`${path} is not a file of its own`);
    let output: FileHandle;
    let created = false;
    try {
      output = await open(destination, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
      created = true;
      added.push({ path: destination });
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
      output = await open(destination, constants.O_RDWR | constants.O_NOFOLLOW | constants.O_NONBLOCK);
    }
    try {
      const start = created ? 0 : await sameStart(input, output, before.size, path);
      if (!created && start < before.size) added.push({ path: destination, extendedFrom: start });
      const buffer = Buffer.alloc(CHUNK);
      for (let at = start; at < before.size;) {
        const { bytesRead } = await input.read(buffer, 0, Math.min(CHUNK, before.size - at), at);
        if (!bytesRead) throw refused(`${path} changed while it was copied: is the session still running?`);
        for (let written = 0; written < bytesRead;) {
          const { bytesWritten } = await output.write(buffer, written, bytesRead - written, at + written);
          if (!bytesWritten) throw refused(`${path} could not be written`);
          written += bytesWritten;
        }
        at += bytesRead;
      }
      await output.sync();
    } finally { await output.close(); }
    const after = await input.stat();
    if (after.size !== before.size || after.mtimeMs !== before.mtimeMs) {
      throw refused(`${path} changed while it was copied: is the session still running?`);
    }
  } finally { await input.close(); }
}

/** How much of the copy a file already there holds: all of it, or its start (a copy cut short, a
 * session continued since). Any other file refuses. */
async function sameStart(input: FileHandle, output: FileHandle, size: number, path: string): Promise<number> {
  const stat = await output.stat();
  if (!stat.isFile() || stat.nlink !== 1) throw refused(`another file is where ${path} goes`);
  const common = Math.min(stat.size, size);
  const theirs = Buffer.alloc(Math.min(CHUNK, common));
  const ours = Buffer.alloc(theirs.length);
  for (let at = 0; at < common;) {
    const length = Math.min(theirs.length, common - at);
    const [a, b] = await Promise.all([output.read(theirs, 0, length, at), input.read(ours, 0, length, at)]);
    if (a.bytesRead !== length || b.bytesRead !== length || !theirs.subarray(0, length).equals(ours.subarray(0, length))) {
      throw refused(`another file is where ${path} goes`);
    }
    at += length;
  }
  // A longer file there is the session continued from the copy: kept as it is.
  return stat.size >= size ? size : stat.size;
}
