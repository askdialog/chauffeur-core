import { afterEach, expect, test } from "bun:test";
import { createCipheriv, createHash, randomBytes } from "node:crypto";
import { lstat, mkdir, mkdtemp, readFile, realpath, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { openPackage, sealPackage } from "../src/package-crypto.ts";
import type { PackageCryptParams, PackageTransferContext } from "../src/protocol.ts";

const roots: string[] = [];
afterEach(async () => { for (const root of roots.splice(0)) await rm(root, { recursive: true, force: true }); });
const context: PackageTransferContext = {
  workspace_id: "11111111-1111-4111-8111-111111111111", operation_id: "22222222-2222-4222-8222-222222222222",
  owner_peer_id: `12D3KooW${"a".repeat(44)}`, controller_peer_id: `12D3KooW${"b".repeat(44)}`,
  source_peer_id: `12D3KooW${"a".repeat(44)}`, target_peer_id: `12D3KooW${"c".repeat(44)}`,
  source_epoch: 1, snapshot_digest: Buffer.alloc(32, 1).toString("base64url"),
};
async function fixture() {
  const root = await realpath(await mkdtemp(join(tmpdir(), "chauffeur-package-crypto-"))); roots.push(root);
  const input = join(root, "source"), output = join(root, "archive.bin"), key = randomBytes(32).toString("base64url");
  await mkdir(join(input, "notes", "sous-dossier-é"), { recursive: true });
  const binary = randomBytes(3 * 1024 * 1024 + 17);
  await writeFile(join(input, "manifest.json"), "synthetic secret conversation");
  await writeFile(join(input, "notes", "sous-dossier-é", "binary"), binary);
  await writeFile(join(input, "empty"), "");
  const params = { input, output, key, context: { ...context } };
  return { root, input, output, key, binary, params };
}
function received(f: Awaited<ReturnType<typeof fixture>>, digest: string, bytes: number, name: string): PackageCryptParams {
  return { input: f.output, output: join(f.root, name), key: f.key, context: { ...context }, ciphertext_digest: digest, ciphertext_bytes: bytes };
}

test("an opaque randomized archive restores exact names and bytes across streaming boundaries", async () => {
  const f = await fixture(), sealed = await sealPackage(f.params);
  const archive = await readFile(f.output);
  expect(sealed.bytes).toBe(archive.length);
  expect(sealed.digest).toBe(createHash("sha256").update(archive).digest("base64url"));
  for (const cleartext of ["manifest.json", "synthetic secret conversation", "sous-dossier-é", context.workspace_id]) {
    expect(archive.includes(Buffer.from(cleartext))).toBeFalse();
  }
  const second = await sealPackage({ ...f.params, output: join(f.root, "second.bin") });
  expect(second.digest).not.toBe(sealed.digest);
  const p = received(f, sealed.digest, sealed.bytes, "destination");
  expect(await openPackage(p)).toEqual(sealed);
  expect(await readFile(join(p.output, "notes", "sous-dossier-é", "binary"))).toEqual(f.binary);
  expect(await readFile(join(p.output, "manifest.json"), "utf8")).toBe("synthetic secret conversation");
  expect((await readFile(join(p.output, "empty"))).length).toBe(0);
  expect((await lstat(p.output)).mode & 0o777).toBe(0o700);
  expect((await lstat(join(p.output, "manifest.json"))).mode & 0o777).toBe(0o600);
  expect(await lstat(join(p.output, ".unauthenticated-payload")).catch(() => null)).toBeNull();
  expect(await readFile(join(f.input, "notes", "sous-dossier-é", "binary"))).toEqual(f.binary);
});

test("wrong operation evidence or key never materializes any package file", async () => {
  const f = await fixture(), sealed = await sealPackage(f.params);
  const variants: PackageTransferContext[] = [
    { ...context, workspace_id: "33333333-3333-4333-8333-333333333333" },
    { ...context, operation_id: "33333333-3333-4333-8333-333333333333" },
    { ...context, owner_peer_id: `12D3KooW${"d".repeat(44)}` },
    { ...context, controller_peer_id: `12D3KooW${"d".repeat(44)}` },
    { ...context, source_peer_id: `12D3KooW${"d".repeat(44)}` },
    { ...context, target_peer_id: `12D3KooW${"d".repeat(44)}` },
    { ...context, source_epoch: 2 },
    { ...context, snapshot_digest: randomBytes(32).toString("base64url") },
  ];
  for (const [index, altered] of variants.entries()) {
    const p = { ...received(f, sealed.digest, sealed.bytes, `wrong-${index}`), context: altered };
    await expect(openPackage(p)).rejects.toThrow("unauthenticated");
    expect(await lstat(join(p.output, "manifest.json")).catch(() => null)).toBeNull();
  }
  const p = { ...received(f, sealed.digest, sealed.bytes, "wrong-key"), key: randomBytes(32).toString("base64url") };
  await expect(openPackage(p)).rejects.toThrow();
  expect(await lstat(join(p.output, "manifest.json")).catch(() => null)).toBeNull();
});

test("corruption cannot bypass the authentication tag even with a replacement ciphertext digest", async () => {
  const f = await fixture(), sealed = await sealPackage(f.params), original = await readFile(f.output);
  for (const [index, at] of [9, 25, original.length - 1].entries()) {
    const altered = Buffer.from(original); altered[at] = (altered[at] ?? 0) ^ 1;
    await writeFile(f.output, altered);
    const p = received(f, createHash("sha256").update(altered).digest("base64url"), sealed.bytes, `corrupt-${index}`);
    await expect(openPackage(p)).rejects.toThrow();
    expect(await lstat(join(p.output, "manifest.json")).catch(() => null)).toBeNull();
  }
});

test("existing files and folders and source links are never overwritten or removed", async () => {
  const f = await fixture(), sealed = await sealPackage(f.params), original = await readFile(f.output);
  await expect(sealPackage(f.params)).rejects.toThrow();
  expect(await readFile(f.output)).toEqual(original);
  const p = received(f, sealed.digest, sealed.bytes, "existing");
  await mkdir(p.output); await writeFile(join(p.output, "marker"), "keep");
  await expect(openPackage(p)).rejects.toThrow();
  expect(await readFile(join(p.output, "marker"), "utf8")).toBe("keep");
  const link = join(f.root, "link"); await symlink(p.output, link);
  await expect(openPackage({ ...p, output: link })).rejects.toThrow();
  expect((await lstat(link)).isSymbolicLink()).toBeTrue();
  await symlink(join(f.root, "missing"), join(f.input, "dangling"));
  await expect(sealPackage({ ...f.params, output: join(f.root, "bad-source") })).rejects.toThrow();
  expect(await readFile(join(p.output, "marker"), "utf8")).toBe("keep");
});

function frame(path: Buffer, bytes = Buffer.from("content")): Buffer {
  const header = Buffer.alloc(12); header.writeUInt32BE(path.length); header.writeBigUInt64BE(BigInt(bytes.length), 4);
  return Buffer.concat([header, path, bytes]);
}
async function rawArchive(f: Awaited<ReturnType<typeof fixture>>, payload: Buffer) {
  const iv = randomBytes(12), cipher = createCipheriv("aes-256-gcm", Buffer.from(f.key, "base64url"), iv);
  cipher.setAAD(Buffer.from(JSON.stringify(["chauffeur-workspace-archive-v1", context.workspace_id, context.owner_peer_id, context.controller_peer_id,
    context.operation_id, context.source_peer_id, context.target_peer_id, context.source_epoch, context.snapshot_digest])));
  const bytes = Buffer.concat([Buffer.from("CHWSPK01"), iv, cipher.update(payload), cipher.final(), cipher.getAuthTag()]);
  await writeFile(f.output, bytes);
  return { digest: createHash("sha256").update(bytes).digest("base64url"), bytes: bytes.length };
}

test("authenticated malicious entry paths, collisions, truncation and trailing bytes refuse extraction", async () => {
  const f = await fixture();
  const payloads = [
    ...["../escaped", "/escaped", "notes/../../escaped", "notes\\escaped", "repos/.git/config", "notes//escaped"].map(p => Buffer.concat([frame(Buffer.from(p)), Buffer.alloc(4)])),
    Buffer.concat([frame(Buffer.from("same")), frame(Buffer.from("same")), Buffer.alloc(4)]),
    Buffer.concat([frame(Buffer.from("parent")), frame(Buffer.from("parent/child")), Buffer.alloc(4)]),
    Buffer.concat([frame(Buffer.from([0xff])), Buffer.alloc(4)]),
    Buffer.concat([frame(Buffer.alloc(8193, 97)), Buffer.alloc(4)]),
    frame(Buffer.from("no-end-marker")),
    Buffer.concat([Buffer.alloc(4), Buffer.from("trailing")]),
    Buffer.from([0xff, 0xff, 0xff, 0xff]),
  ];
  for (const [index, payload] of payloads.entries()) {
    const sealed = await rawArchive(f, payload), p = received(f, sealed.digest, sealed.bytes, `malicious-${index}`);
    await expect(openPackage(p)).rejects.toThrow();
    expect(await lstat(join(f.root, "escaped")).catch(() => null)).toBeNull();
  }
});

test("unsupported sizes refuse before allocating or creating a destination", async () => {
  const f = await fixture(), sealed = await sealPackage(f.params);
  for (const [index, bytes] of [-1, 0, Number.NaN, 2 ** 53, 16 * 1024 * 1024 * 1024 + 37].entries()) {
    const p = { ...received(f, sealed.digest, bytes, `oversized-${index}`) };
    await expect(openPackage(p)).rejects.toThrow();
    expect(await lstat(p.output).catch(() => null)).toBeNull();
  }
});

test("trusted context is copied before asynchronous validation yields", async () => {
  const f = await fixture(), sealed = await sealPackage(f.params), p = received(f, sealed.digest, sealed.bytes, "snapshot");
  const opening = openPackage(p);
  p.context.source_epoch = 9; p.ciphertext_digest = randomBytes(32).toString("base64url"); p.key = randomBytes(32).toString("base64url");
  expect(await opening).toEqual(sealed);
});

test("a backup authenticates under its own label: it never opens as a transfer's archive, nor the reverse", async () => {
  const backup: PackageTransferContext = { ...context, target_peer_id: "", snapshot_digest: "", backup: true };
  const f = await fixture();
  const sealed = await sealPackage({ ...f.params, context: backup });
  const asTransfer = received(f, sealed.digest, sealed.bytes, "as-transfer");
  await expect(openPackage({ ...asTransfer, context: { ...context } })).rejects.toThrow();
  const opened = { ...received(f, sealed.digest, sealed.bytes, "as-backup"), context: backup };
  expect(await openPackage(opened)).toEqual(sealed);
  // A backup names no target, and is taken by a worker other than the controller.
  await expect(sealPackage({ ...f.params, output: join(f.root, "targeted.bin"), context: { ...backup, target_peer_id: context.target_peer_id } })).rejects.toThrow();
  await expect(sealPackage({ ...f.params, output: join(f.root, "controller.bin"), context: { ...backup, source_peer_id: context.controller_peer_id } })).rejects.toThrow();
  // It binds no manifest digest: one named is refused rather than ignored.
  await expect(sealPackage({ ...f.params, output: join(f.root, "digest.bin"), context: { ...backup, snapshot_digest: context.snapshot_digest } })).rejects.toThrow();
  const transfer = await sealPackage({ ...f.params, output: join(f.root, "transfer.bin") });
  const asBackup = { input: join(f.root, "transfer.bin"), output: join(f.root, "transfer-as-backup"), key: f.key, context: backup, ciphertext_digest: transfer.digest, ciphertext_bytes: transfer.bytes };
  await expect(openPackage(asBackup)).rejects.toThrow();
});
