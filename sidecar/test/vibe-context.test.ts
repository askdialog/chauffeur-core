import { afterEach, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { chmodSync, existsSync, linkSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { restoreContexts } from "../src/agents/context-restore.ts";
import { captureVibeContext, validVibePath, vibeContextFiles } from "../src/agents/vibe/context-bundle.ts";

const SESSION = "a9234162-819d-74ba-e87f-2dd667550c20";
const PLUGIN = "7383f7b653209070505a2a1c03b62b99feb5d3934a3f2b121077f8546f7e2e84";
const sha = (text: string) => createHash("sha256").update(text).digest("hex");
const SKILL = "# Worktree skill\n";

const roots: string[] = [];
afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function folder(): string {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "vibe-context-")));
  roots.push(root);
  return root;
}

function write(home: string, path: string, content: string): void {
  mkdirSync(dirname(join(home, path)), { recursive: true });
  writeFileSync(join(home, path), content);
}

/** A Vibe home with one session pinning one plugin, as Vibe 3.1.3 lays them out. */
function vibeHome(turn = "first"): string {
  const home = join(folder(), ".vibe");
  const session = `logs/session/unified/${SESSION}`;
  write(home, `${session}/CURRENT`, `{"generation":"${turn}"}\n`);
  write(home, `${session}/meta.json`, `{"session_id":"${SESSION}","turn":"${turn}"}`);
  write(home, `${session}/journal/0000000000000051.jsonl`, `{"turn":"${turn}"}\n`);
  write(home, `${session}/journal/0000000000000052.jsonl`, "");
  write(home, `${session}/generations/0000000000000050/runtime-state.json`, JSON.stringify({ plugin_lock: { lock_version: 1, plugins: [{ name: "vibe", content_digest: PLUGIN }], snapshot_digest: sha("snapshot") } }));
  write(home, `logs/session/plugins/blobs/${sha("snapshot").slice(0, 2)}/${sha("snapshot")}`, "snapshot");
  write(home, `${session}/chunks/${sha("chunk")}.json`, "chunk");
  write(home, `logs/session/plugins/manifests/73/${PLUGIN}`, JSON.stringify({ content_digest: PLUGIN, entries: [{ kind: "file", path: "skills/worktree/SKILL.md", sha256: sha(SKILL) }] }));
  // As Vibe stores them: the package's file is a hard link to its blob.
  write(home, `logs/session/plugins/blobs/${sha(SKILL).slice(0, 2)}/${sha(SKILL)}`, SKILL);
  mkdirSync(join(home, `logs/session/plugins/packages/73/${PLUGIN}/skills/worktree`), { recursive: true });
  linkSync(join(home, `logs/session/plugins/blobs/${sha(SKILL).slice(0, 2)}/${sha(SKILL)}`), join(home, `logs/session/plugins/packages/73/${PLUGIN}/skills/worktree/SKILL.md`));
  // Another session and another plugin: not carried.
  write(home, "logs/session/unified/00dd7895-6924-125c-6f96-939ec507d785/CURRENT", "{}\n");
  write(home, `logs/session/plugins/manifests/aa/${"a".repeat(64)}`, "{}");
  return home;
}

/** A package entry as the daemon hands it over: the capture written to a file. */
function packaged(bundle: unknown): { path: string; sha256: string; bytes: number } {
  const bytes = Buffer.from(JSON.stringify(bundle));
  const path = join(folder(), "context.json");
  writeFileSync(path, bytes);
  return { path, sha256: createHash("sha256").update(bytes).digest("hex"), bytes: bytes.length };
}

function restoreParams(destination: string, bundle: unknown) {
  const homes = folder();
  return {
    claude_home: join(homes, ".claude"),
    codex_home: join(homes, ".codex"),
    vibe_home: destination,
    contexts: [{ agent: "vibe" as const, session_id: SESSION, cwd: homes, ...packaged(bundle) }],
  };
}

test("a capture holds the session's folder and the plugin it pins, nothing else", async () => {
  const bundle = await captureVibeContext(vibeHome(), SESSION);
  const paths = bundle.files.map((file) => file.path).sort();
  expect(paths).toEqual(
    [
      `logs/session/plugins/blobs/${sha(SKILL).slice(0, 2)}/${sha(SKILL)}`,
      `logs/session/plugins/blobs/${sha("snapshot").slice(0, 2)}/${sha("snapshot")}`,
      `logs/session/plugins/manifests/73/${PLUGIN}`,
      `logs/session/plugins/packages/73/${PLUGIN}/skills/worktree/SKILL.md`,
      `logs/session/unified/${SESSION}/CURRENT`,
      `logs/session/unified/${SESSION}/chunks/${sha("chunk")}.json`,
      `logs/session/unified/${SESSION}/generations/0000000000000050/runtime-state.json`,
      `logs/session/unified/${SESSION}/journal/0000000000000051.jsonl`,
      `logs/session/unified/${SESSION}/journal/0000000000000052.jsonl`,
      `logs/session/unified/${SESSION}/meta.json`,
    ].sort(),
  );
  expect(vibeContextFiles(bundle).files).toHaveLength(10);
});

test("a hard-linked plugin file whose content is not its digest is not carried", async () => {
  const home = vibeHome();
  const elsewhere = join(folder(), "secret");
  writeFileSync(elsewhere, "not the skill");
  const blob = join(home, `logs/session/plugins/blobs/${sha(SKILL).slice(0, 2)}/${sha(SKILL)}`);
  rmSync(blob);
  linkSync(elsewhere, blob);
  await expect(captureVibeContext(home, SESSION)).rejects.toThrow("unsafe");
});

test("a carried conversation is checked whole: paths, digests, blobs' content", async () => {
  const bundle = await captureVibeContext(vibeHome(), SESSION);
  expect(validVibePath(`logs/session/unified/${SESSION}/../x`, SESSION)).toBe(false);
  expect(validVibePath("logs/session/unified/00dd7895-6924-125c-6f96-939ec507d785/CURRENT", SESSION)).toBe(false);
  expect(validVibePath("config.toml", SESSION)).toBe(false);
  const tampered = { ...bundle, files: bundle.files.map((file) => (file.path.includes("/blobs/") ? { ...file, content: Buffer.from("other").toString("base64"), sha256: sha("other") } : file)) };
  expect(() => vibeContextFiles(tampered)).toThrow("Vibe context is incomplete or unsafe");
  expect(() => vibeContextFiles({ ...bundle, files: bundle.files.filter((file) => !file.path.endsWith("/CURRENT")) })).toThrow();
});

test("restored into a new home, then back over an older copy, which is set aside whole", async () => {
  const destination = join(folder(), ".vibe");
  const first = await restoreContexts(restoreParams(destination, await captureVibeContext(vibeHome("first"), SESSION)));
  const current = join(destination, `logs/session/unified/${SESSION}/CURRENT`);
  expect(readFileSync(current, "utf8")).toBe('{"generation":"first"}\n');
  expect(first.replaced ?? []).toEqual([]);
  expect(first.files).toContain(current);

  const back = await restoreContexts(restoreParams(destination, await captureVibeContext(vibeHome("later"), SESSION)));
  expect(readFileSync(current, "utf8")).toBe('{"generation":"later"}\n');
  const [replaced] = back.replaced ?? [];
  expect(replaced?.path).toBe(join(destination, `logs/session/unified/${SESSION}`));
  expect(replaced?.aside.startsWith(join(destination, "logs/session/chauffeur-replaced", `${SESSION}-`))).toBe(true);
  expect(readFileSync(join(replaced?.aside ?? "", "CURRENT"), "utf8")).toBe('{"generation":"first"}\n');
  // The plugin's files were there already, the same: kept, not listed as added.
  expect(back.files.some((file) => file.includes("/plugins/"))).toBe(false);
});

test("a plugin store Vibe made read-only takes a conversation coming back", async () => {
  const destination = join(folder(), ".vibe");
  await restoreContexts(restoreParams(destination, await captureVibeContext(vibeHome("first"), SESSION)));
  const skill = join(destination, `logs/session/plugins/packages/73/${PLUGIN}/skills/worktree/SKILL.md`);
  chmodSync(skill, 0o444);
  chmodSync(dirname(skill), 0o555);
  try {
    await restoreContexts(restoreParams(destination, await captureVibeContext(vibeHome("later"), SESSION)));
    expect(readFileSync(join(destination, `logs/session/unified/${SESSION}/CURRENT`), "utf8")).toBe('{"generation":"later"}\n');
  } finally {
    chmodSync(dirname(skill), 0o755);
  }
});

test("a restoration that fails takes back what it wrote and puts the copy it set aside back", async () => {
  const destination = join(folder(), ".vibe");
  await restoreContexts(restoreParams(destination, await captureVibeContext(vibeHome("first"), SESSION)));
  const later = await captureVibeContext(vibeHome("later"), SESSION);
  // The same plugin path with other bytes: refused once the session folder was set aside.
  const clash = {
    ...later,
    files: later.files.map((file) =>
      file.path.includes("/packages/") ? { ...file, content: Buffer.from("changed").toString("base64"), sha256: sha("changed") } : file,
    ),
  };
  await expect(restoreContexts(restoreParams(destination, clash))).rejects.toThrow();
  expect(readFileSync(join(destination, `logs/session/unified/${SESSION}/CURRENT`), "utf8")).toBe('{"generation":"first"}\n');
  expect(readFileSync(join(destination, `logs/session/unified/${SESSION}/meta.json`), "utf8")).toContain('"turn":"first"');
  expect(existsSync(join(destination, "logs/session/chauffeur-replaced"))).toBe(false);
});
