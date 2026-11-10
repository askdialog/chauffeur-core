// Compaction through Jev (TypeSafe), for the Claude chats of a user who turned it on: Chauffeur's
// copy of the fast-jev-compaction plugin (plugins/jev) replaces Claude Code's summary, and the
// session sends `/compact` itself after a turn that leaves the context past the threshold, since
// a plugin cannot start a compaction in an SDK session.

import { createHash, randomBytes } from "node:crypto";
import { existsSync, lstatSync, mkdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { JEV_PLUGIN_FILES } from "./jev-plugin.generated.ts";

/** The user's own install of the plugin, left out of the sessions that load Chauffeur's. */
export const USER_JEV_PLUGIN = "fast-jev-compaction@fast-jev-compaction";

const COMPLETE = ".complete";

/**
 * Where the plugin's folder goes: the user's runtime dir when there is one (per user, 0700), else
 * the temp dir. Claude Code runs the plugin's hooks as code in its own process, so the folder must
 * be one no other local user can prepare for us.
 */
export function jevPluginRoot(): string {
  const runtime = process.env.XDG_RUNTIME_DIR;
  if (runtime && ownDir(runtime)) return runtime;
  return tmpdir();
}

/** A real directory (no symlink) of our own that nobody else can write into. */
function ownDir(path: string): boolean {
  let stat;
  try {
    stat = lstatSync(path);
  } catch {
    return false;
  }
  const uid = process.getuid?.();
  return stat.isDirectory() && (uid === undefined || stat.uid === uid) && (stat.mode & 0o022) === 0;
}

function exists(path: string): boolean {
  try {
    lstatSync(path);
    return true;
  } catch {
    return false;
  }
}

/**
 * The plugin's folder, written once per version under the root: the sidecar is a single binary,
 * and Claude Code loads a plugin from a folder. An existing folder is only reused when it is ours
 * (owner, mode, no symlink); one somebody else put at the predictable path is left alone and the
 * plugin goes to a fresh name next to it.
 */
export function jevPluginDir(files: Record<string, string> = JEV_PLUGIN_FILES, root = jevPluginRoot()): string {
  const hash = createHash("sha256");
  for (const path of Object.keys(files).sort()) hash.update(path).update("\0").update(files[path] ?? "").update("\0");
  const dir = join(root, `chauffeur-jev-${hash.digest("hex").slice(0, 16)}`);
  if (ownDir(dir) && existsSync(join(dir, COMPLETE))) return dir;
  const staging = `${dir}.${process.pid}.${randomBytes(8).toString("hex")}`;
  mkdirSync(staging, { recursive: true, mode: 0o700 });
  for (const [path, content] of Object.entries(files)) {
    const target = join(staging, path);
    mkdirSync(dirname(target), { recursive: true, mode: 0o700 });
    writeFileSync(target, content);
  }
  writeFileSync(join(staging, COMPLETE), "");
  let target = dir;
  if (exists(dir)) {
    if (ownDir(dir)) rmSync(dir, { recursive: true, force: true });
    else target = `${dir}-${randomBytes(8).toString("hex")}`;
  }
  try {
    renameSync(staging, target);
  } catch (error) {
    // Another sidecar wrote the same files meanwhile.
    rmSync(staging, { recursive: true, force: true });
    if (!(ownDir(target) && existsSync(join(target, COMPLETE)))) throw error;
  }
  if (!ownDir(target)) throw new Error(`${target} is not a directory of ours: not loading the Jev plugin from it`);
  return target;
}
