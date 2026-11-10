// Endpoint-only package encryption. Failed outputs stay private for explicit reconciliation.
import { createCipheriv, createDecipheriv, createHash, randomBytes } from "node:crypto";
import { constants } from "node:fs";
import { lstat, mkdir, open, readdir, unlink, type FileHandle } from "node:fs/promises";
import { dirname, isAbsolute, join, resolve } from "node:path";
import type { PackageCryptParams, PackageCryptResult, PackageTransferContext } from "./protocol.ts";
import { canonicalDirectory } from "./agents/context-files.ts";

const MAGIC = Buffer.from("CHWSPK01");
const MAX_BYTES = 16 * 1024 * 1024 * 1024;
const MAX_FILES = 200_000;
const MAX_PATH_BYTES = 64 * 1024 * 1024;
const CHUNK = 64 * 1024;
const SCRATCH = ".unauthenticated-payload";
const fail = () => new Error("Workspace archive is incomplete or unauthenticated; retain its source and inspect the private output before retrying");

function encoded(value: string, size: number): Buffer {
  if (typeof value !== "string" || value.length !== Math.ceil(size * 4 / 3)) throw fail();
  const bytes = Buffer.from(value, "base64url");
  if (bytes.length !== size || bytes.toString("base64url") !== value) throw fail();
  return bytes;
}
function aad(c: PackageTransferContext): Buffer {
  const uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;
  const peer = /^12D3KooW[1-9A-HJ-NP-Za-km-z]{44}$/;
  const epoch = Number.isSafeInteger(c.source_epoch) && c.source_epoch >= 1 && c.source_epoch < Number.MAX_SAFE_INTEGER;
  // A backup: taken by the worker running the workspace, for no target. Its own label keeps a
  // backup from opening as a transfer's archive, and the reverse. It binds only what the Mac
  // keeps of the transfer and the object's name gives, so any backup in storage opens without
  // a reference the Mac may never have received: no manifest digest (the package checks itself).
  if (c.backup) {
    if (!uuid.test(c.workspace_id) || !uuid.test(c.operation_id) || c.target_peer_id !== "" || c.snapshot_digest !== ""
      || ![c.owner_peer_id, c.controller_peer_id, c.source_peer_id].every(p => peer.test(p))
      || c.owner_peer_id === c.controller_peer_id || c.controller_peer_id === c.source_peer_id || !epoch) throw fail();
    return Buffer.from(JSON.stringify(["chauffeur-workspace-backup-v2", c.workspace_id, c.owner_peer_id, c.controller_peer_id,
      c.operation_id, c.source_peer_id, c.source_epoch]));
  }
  if (!uuid.test(c.workspace_id) || !uuid.test(c.operation_id)
    || ![c.owner_peer_id, c.controller_peer_id, c.source_peer_id, c.target_peer_id].every(p => peer.test(p))
    || c.owner_peer_id === c.controller_peer_id || c.source_peer_id === c.target_peer_id
    || c.controller_peer_id === c.source_peer_id || c.controller_peer_id === c.target_peer_id
    || !Number.isSafeInteger(c.source_epoch) || c.source_epoch < 1 || c.source_epoch >= Number.MAX_SAFE_INTEGER) throw fail();
  encoded(c.snapshot_digest, 32);
  return Buffer.from(JSON.stringify(["chauffeur-workspace-archive-v1", c.workspace_id, c.owner_peer_id, c.controller_peer_id,
    c.operation_id, c.source_peer_id, c.target_peer_id, c.source_epoch, c.snapshot_digest]));
}
function portable(path: string): void {
  if (!path || Buffer.byteLength(path) > 8192 || /[\\:\0]/.test(path)
    || path.split("/").some(p => !p || p === "." || p === ".." || p.toLowerCase() === ".git")) throw fail();
}
async function newOutput(path: string): Promise<void> {
  if (!isAbsolute(path) || resolve(path) !== path) throw fail();
  await canonicalDirectory(dirname(path));
}
async function regular(path: string): Promise<FileHandle> {
  if (!isAbsolute(path) || resolve(path) !== path) throw fail();
  await canonicalDirectory(dirname(path));
  const file = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    const stat = await file.stat();
    if (!stat.isFile() || stat.nlink !== 1) throw fail();
    return file;
  } catch (error) { await file.close(); throw error; }
}
async function write(file: FileHandle, bytes: Uint8Array): Promise<void> {
  let offset = 0;
  while (offset < bytes.length) {
    const result = await file.write(bytes, offset, bytes.length - offset);
    if (!result.bytesWritten) throw fail();
    offset += result.bytesWritten;
  }
}
async function exact(file: FileHandle, length: number, position: number): Promise<Buffer> {
  const buffer = Buffer.alloc(length);
  let offset = 0;
  while (offset < length) {
    const result = await file.read(buffer, offset, length - offset, position + offset);
    if (!result.bytesRead) throw fail();
    offset += result.bytesRead;
  }
  return buffer;
}
async function syncDir(path: string): Promise<void> {
  const directory = await open(path, constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW);
  try { await directory.sync(); } finally { await directory.close(); }
}
async function* files(root: string, prefix = "", budget = { directories: 0, bytes: 0 }): AsyncGenerator<string> {
  if (++budget.directories > MAX_FILES || (budget.bytes += Buffer.byteLength(prefix)) > MAX_PATH_BYTES) throw fail();
  await canonicalDirectory(prefix ? join(root, prefix) : root);
  const entries = await readdir(prefix ? join(root, prefix) : root, { withFileTypes: true });
  for (const entry of entries.sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : 0)) {
    const path = prefix ? `${prefix}/${entry.name}` : entry.name;
    portable(path);
    if (entry.isDirectory()) yield* files(root, path, budget);
    else if (entry.isFile()) yield path;
    else throw fail();
  }
}

/** Only ciphertext leaves this call. Neither names nor operation metadata are cleartext headers. */
export async function sealPackage(params: PackageCryptParams): Promise<PackageCryptResult> {
  const { input, output, context, ciphertext_digest, ciphertext_bytes } = { ...params, context: { ...params.context } };
  const key = encoded(params.key, 32);
  try {
    const header = aad(context);
    if (ciphertext_digest !== undefined || ciphertext_bytes !== undefined) throw fail();
    await canonicalDirectory(input); await newOutput(output);
    // Reject output within input, including an ancestor reached through an alias.
    if (output.startsWith(`${input}/`)) throw fail();
    const iv = randomBytes(12), cipher = createCipheriv("aes-256-gcm", key, iv);
    cipher.setAAD(header);
    const file = await open(output, "wx", 0o600);
    const hash = createHash("sha256");
    let bytes = 0, plaintext = 0, count = 0, pathBytes = 0;
    const emit = async (value: Buffer) => { hash.update(value); bytes += value.length; await write(file, value); };
    const encrypt = async (value: Buffer) => {
      if ((plaintext += value.length) > MAX_BYTES) throw fail();
      await emit(cipher.update(value));
    };
    try {
      await emit(Buffer.concat([MAGIC, iv]));
      const buffer = Buffer.alloc(CHUNK);
      try {
        for await (const path of files(input)) {
          if (++count > MAX_FILES || (pathBytes += Buffer.byteLength(path)) > MAX_PATH_BYTES) throw fail();
          const source = await regular(join(input, path));
          try {
            const stat = await source.stat();
            if (!Number.isSafeInteger(stat.size) || stat.size > MAX_BYTES - plaintext) throw fail();
            const name = Buffer.from(path), frame = Buffer.alloc(12);
            frame.writeUInt32BE(name.length); frame.writeBigUInt64BE(BigInt(stat.size), 4);
            await encrypt(frame); await encrypt(name);
            let position = 0;
            while (position < stat.size) {
              const result = await source.read(buffer, 0, Math.min(buffer.length, stat.size - position), position);
              if (!result.bytesRead) throw fail();
              position += result.bytesRead;
              await encrypt(buffer.subarray(0, result.bytesRead));
            }
            if ((await source.read(buffer, 0, 1, position)).bytesRead) throw fail();
          } finally { await source.close(); }
        }
        await encrypt(Buffer.alloc(4));
        await emit(cipher.final()); await emit(cipher.getAuthTag());
        await file.sync(); await syncDir(dirname(output));
        return { digest: hash.digest("base64url"), bytes };
      } finally { buffer.fill(0); }
    } finally { await file.close(); }
  } catch { throw fail(); }
  finally { key.fill(0); }
}

/** Authentication completes in a private scratch file before any package entry is materialized. */
export async function openPackage(params: PackageCryptParams): Promise<PackageCryptResult> {
  const { input, output, context, ciphertext_digest, ciphertext_bytes } = { ...params, context: { ...params.context } };
  const key = encoded(params.key, 32);
  try {
    const header = aad(context);
    if (!ciphertext_digest || !Number.isSafeInteger(ciphertext_bytes) || ciphertext_bytes === undefined
      || ciphertext_bytes < 40 || ciphertext_bytes > MAX_BYTES + 36) throw fail();
    encoded(ciphertext_digest, 32);
    await newOutput(output);
    const source = await regular(input);
    try {
      if ((await source.stat()).size !== ciphertext_bytes) throw fail();
      const prefix = await exact(source, 20, 0);
      if (!prefix.subarray(0, 8).equals(MAGIC)) throw fail();
      const tag = await exact(source, 16, ciphertext_bytes - 16);
      const decipher = createDecipheriv("aes-256-gcm", key, prefix.subarray(8));
      decipher.setAAD(header); decipher.setAuthTag(tag);
      await mkdir(output, { mode: 0o700 });
      const scratch = await open(join(output, SCRATCH), "wx+", 0o600);
      try {
        const hash = createHash("sha256").update(prefix), buffer = Buffer.alloc(CHUNK);
        try {
          let position = 20;
          while (position < ciphertext_bytes - 16) {
            const result = await source.read(buffer, 0, Math.min(buffer.length, ciphertext_bytes - 16 - position), position);
            if (!result.bytesRead) throw fail();
            position += result.bytesRead;
            const ciphertext = buffer.subarray(0, result.bytesRead);
            hash.update(ciphertext);
            const plaintext = decipher.update(ciphertext);
            try { await write(scratch, plaintext); } finally { plaintext.fill(0); }
          }
          hash.update(tag);
          if (hash.digest("base64url") !== ciphertext_digest || (await source.read(buffer, 0, 1, ciphertext_bytes)).bytesRead) throw fail();
          const final = decipher.final();
          try { await write(scratch, final); } finally { final.fill(0); }
        } finally { buffer.fill(0); }
        await scratch.sync();
        await extract(scratch, output);
      } finally { await scratch.close(); }
      // No caller receives a package before authentication and strict extraction have completed.
      await unlink(join(output, SCRATCH));
      await syncDir(output); await syncDir(dirname(output));
      return { digest: ciphertext_digest, bytes: ciphertext_bytes };
    } finally { await source.close(); }
  } catch { throw fail(); }
  finally { key.fill(0); }
}

async function extract(source: FileHandle, output: string): Promise<void> {
  const size = (await source.stat()).size, seen = new Set<string>(), directories = new Set([output]);
  let position = 0, pathBytes = 0, directoryBytes = 0;
  const buffer = Buffer.alloc(CHUNK);
  try {
    while (true) {
      const length = (await exact(source, 4, position)).readUInt32BE(); position += 4;
      if (length === 0) { if (position !== size) throw fail(); break; }
      if (length > 8192 || seen.size >= MAX_FILES || (pathBytes += length) > MAX_PATH_BYTES) throw fail();
      const entrySize = (await exact(source, 8, position)).readBigUInt64BE(); position += 8;
      const path = new TextDecoder("utf-8", { fatal: true }).decode(await exact(source, length, position)); position += length;
      portable(path);
      if (seen.has(path) || entrySize > BigInt(size - position) || entrySize > BigInt(MAX_BYTES)) throw fail();
      seen.add(path);
      const parts = path.split("/"); parts.pop();
      let parent = output;
      for (const part of parts) {
        parent = join(parent, part);
        if (!directories.has(parent) && (directories.size >= MAX_FILES
          || (directoryBytes += Buffer.byteLength(parent.slice(output.length + 1))) > MAX_PATH_BYTES)) throw fail();
        try { await mkdir(parent, { mode: 0o700 }); }
        catch (error) { if (!(error instanceof Error && "code" in error && error.code === "EEXIST")) throw error; }
        if (!(await lstat(parent)).isDirectory()) throw fail();
        await canonicalDirectory(parent); directories.add(parent);
      }
      const file = await open(join(output, path), "wx", 0o600);
      try {
        let remaining = Number(entrySize);
        while (remaining > 0) {
          const result = await source.read(buffer, 0, Math.min(buffer.length, remaining), position);
          if (!result.bytesRead) throw fail();
          await write(file, buffer.subarray(0, result.bytesRead));
          remaining -= result.bytesRead; position += result.bytesRead;
        }
        await file.sync();
      } finally { await file.close(); }
    }
    for (const directory of [...directories].sort((a, b) => b.length - a.length)) await syncDir(directory);
  } finally { buffer.fill(0); }
}
