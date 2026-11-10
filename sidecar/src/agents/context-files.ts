// Native context files are private plaintext. Call only for a trusted stopped source.
import { constants } from "node:fs";
import { lstat, mkdir, open, realpath, rm, rmdir, truncate } from "node:fs/promises";
import { dirname, isAbsolute, join, resolve } from "node:path";
const MAX_FILE = 64 * 1024 * 1024;
const failure = () => new Error("Native context files are unsafe or incomplete; preserve the source and inspect its history before retrying");
function safePath(path: string): boolean {
  return path.length > 0 && path.length < 8192 && !path.includes("\\") && !path.includes("\0")
    && !isAbsolute(path) && path.split("/").every((p) => p !== "" && p !== "." && p !== "..");
}
export async function canonicalDirectory(path: string): Promise<void> {
  if (!isAbsolute(path) || resolve(path) !== path || await realpath(path) !== path || !(await lstat(path)).isDirectory()) throw failure();
}
/** With `live`, a transcript still being appended (a running chat's, for a backup) is read as it
 * stood when opened, up to its last complete line (empty when it has none yet); any other file
 * must hold still as before. */
export async function readContextFile(home: string, path: string, live = false): Promise<Buffer> {
  if (!safePath(path)) throw failure();
  const appended = live && path.endsWith(".jsonl");
  // The stopped source is trusted. Check ancestors plus O_NOFOLLOW on the file, then
  // recheck its name and identity; this is not containment against a concurrent writer.
  const full = join(home, path);
  if (await realpath(full) !== full) throw failure();
  const file = await open(full, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    const before = await file.stat();
    if (!before.isFile() || before.size > MAX_FILE || before.nlink !== 1) throw failure();
    if (before.size < 1 && !appended) throw failure();
    const bytes = Buffer.alloc(before.size + 1);
    let size = 0;
    while (size < bytes.length) {
      const result = await file.read(bytes, size, bytes.length - size, size);
      if (!result.bytesRead) break;
      size += result.bytesRead;
    }
    const after = await file.stat();
    const named = await lstat(full);
    if (named.ino !== before.ino || named.dev !== before.dev || await realpath(full) !== full) throw failure();
    if (appended) {
      return bytes.subarray(0, bytes.subarray(0, Math.min(size, before.size)).lastIndexOf(10) + 1);
    }
    if (size !== before.size || after.size !== before.size || after.mtimeMs !== before.mtimeMs
      || after.ctimeMs !== before.ctimeMs) throw failure();
    return bytes.subarray(0, size);
  } finally { await file.close(); }
}

/** Write validated context only into a new private root, never over an existing home. */
export async function restoreContextFiles(files: ReadonlyArray<{ path: string; bytes: Buffer }>, home: string): Promise<void> {
  if (!files.length || files.some((file) => !safePath(file.path))) throw failure();
  if (!isAbsolute(home) || resolve(home) !== home) throw failure();
  await canonicalDirectory(dirname(home));
  await mkdir(home, { mode: 0o700 }); // EEXIST never authorizes removal/overwrite.
  try {
    const dirs = new Set([home]);
    for (const file of files) {
      const destination = join(home, file.path);
      await mkdir(dirname(destination), { recursive: true, mode: 0o700 });
      let dir = dirname(destination);
      while (dir !== home) { dirs.add(dir); dir = dirname(dir); }
      const output = await open(destination, "wx", 0o600);
      try { await output.writeFile(file.bytes); await output.sync(); }
      finally { await output.close(); }
    }
    for (const dir of [...dirs].sort((a, b) => b.length - a.length).concat(dirname(home))) {
      const handle = await open(dir, constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW);
      try { await handle.sync(); } finally { await handle.close(); }
    }
  } catch (error) { await rm(home, { recursive: true, force: true }); throw error; }
}

/** What `addContextFiles` changed, in order, so a failed restoration can take it back: a folder
 * or file it created, or a file it extended (with its size before). A folder that is a native
 * home itself, made because the machine had none, is marked so. */
export type Added = Array<{ path: string; extendedFrom?: number; folder?: true; home?: true }>;

/** Add validated context to an existing native home (the user's own). Each file is new, or
 * already there: kept when it starts with the carried bytes (a retry, or a conversation this
 * machine continued since), extended when it is the start of them (a conversation that came back
 * longer). Any other file in the way refuses. Never truncates or replaces what was there. */
export async function addContextFiles(files: ReadonlyArray<{ path: string; bytes: Buffer }>, home: string, added: Added): Promise<void> {
  if (files.some((file) => !safePath(file.path))) throw failure();
  await canonicalDirectory(home);
  for (const file of files) {
    const destination = join(home, file.path);
    const missing: string[] = [];
    for (let dir = dirname(destination); dir !== home; dir = dirname(dir)) {
      try { if (!(await lstat(dir)).isDirectory()) throw failure(); break; }
      catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error; missing.unshift(dir); }
    }
    for (const dir of missing) { await mkdir(dir, { mode: 0o700 }); added.push({ path: dir, folder: true }); }
    if (await realpath(dirname(destination)) !== dirname(destination)) throw failure();
    let output;
    try { output = await open(destination, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600); }
    catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
      await joinExisting(destination, file.bytes, added);
      continue;
    }
    added.push({ path: destination });
    try { await output.writeFile(file.bytes); await output.sync(); }
    finally { await output.close(); }
  }
}

/** A file already where carried bytes go: one must start the other. Only their common length is
 * read, so a long continuation is never loaded whole. */
async function joinExisting(destination: string, bytes: Buffer, added: Added): Promise<void> {
  const handle = await open(destination, constants.O_RDWR | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    const stat = await handle.stat();
    if (!stat.isFile() || stat.nlink !== 1) throw failure();
    const common = Math.min(stat.size, bytes.length);
    const existing = Buffer.alloc(common);
    let read = 0;
    while (read < common) {
      const { bytesRead } = await handle.read(existing, read, common - read, read);
      if (!bytesRead) throw failure();
      read += bytesRead;
    }
    if (!existing.equals(bytes.subarray(0, common))) throw failure();
    if (stat.size < bytes.length) {
      added.push({ path: destination, extendedFrom: stat.size });
      let written = stat.size;
      while (written < bytes.length) {
        const { bytesWritten } = await handle.write(bytes, written, bytes.length - written, written);
        if (!bytesWritten) throw failure();
        written += bytesWritten;
      }
      await handle.sync();
    }
  } finally { await handle.close(); }
}

/** Takes back what `addContextFiles` did, newest first: an extended file is cut back to its size
 * before, a created one removed, a created folder removed when empty. */
export async function undoContextFiles(added: Added): Promise<void> {
  for (const entry of [...added].reverse()) {
    if (entry.extendedFrom !== undefined) {
      await truncate(entry.path, entry.extendedFrom).catch(() => undefined);
      continue;
    }
    await rm(entry.path, { force: true }).catch(() => rmdir(entry.path).catch(() => undefined));
  }
}
