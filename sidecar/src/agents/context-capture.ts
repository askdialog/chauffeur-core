// One chat's native conversation, written as a strict bundle to a new private file the daemon
// chose. For a transfer the chat is stopped: one with a session in this sidecar is refused, and
// its absence is not source-stop proof, which stays the daemon's precondition. For a backup
// (`live`) it may run: each transcript is taken up to its last complete line. Private plaintext,
// not encrypted.
import { createHash } from "node:crypto";
import { constants } from "node:fs";
import { lstat, open, readdir, realpath } from "node:fs/promises";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, join, resolve } from "node:path";
import type { ContextCaptureParams, ContextCaptureResult } from "../protocol.ts";
import { captureClaudeContext, claudeProjectKey } from "./claude/context-bundle.ts";
import { captureCodexContext } from "./codex/context-bundle.ts";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const MAX_SCAN = 100_000;
const missing = () => new Error("The conversation's native history was not found exactly once; keep it on this computer and inspect the agent's sessions folder");

/** The native folders a chat's agent uses: its own environment over the sidecar's. */
export function nativeHome(agent: "claude" | "codex", env: Record<string, string | undefined>): string {
  const home = env.HOME || homedir();
  const dir = agent === "claude" ? (env.CLAUDE_CONFIG_DIR || join(home, ".claude")) : (env.CODEX_HOME || join(home, ".codex"));
  return resolve(home, dir);
}

/** The single rollout file named for `sessionId` under `sessions/` or `archived_sessions/`. */
export async function findCodexRollout(home: string, sessionId: string): Promise<string> {
  if (!UUID.test(sessionId)) throw missing();
  const found: string[] = [];
  let scanned = 0;
  async function scan(dir: string, depth: number): Promise<void> {
    let entries;
    try { entries = await readdir(dir, { withFileTypes: true }); }
    catch (error) { if (depth === 0 && (error as NodeJS.ErrnoException).code === "ENOENT") return; throw error; }
    for (const entry of entries) {
      if (++scanned > MAX_SCAN) throw missing();
      const path = join(dir, entry.name);
      if (entry.isDirectory() && depth < 4) await scan(path, depth + 1);
      else if (entry.isFile() && entry.name.startsWith("rollout-") && entry.name.endsWith(`-${sessionId}.jsonl`)) found.push(path);
    }
  }
  for (const dir of ["sessions", "archived_sessions"]) await scan(join(home, dir), 0);
  if (found.length !== 1) throw missing();
  return found[0]!;
}

async function existing(path: string): Promise<string> {
  try { return await realpath(path); } catch { throw missing(); }
}

export async function captureContext(
  params: ContextCaptureParams,
  environment: Record<string, string | undefined>,
): Promise<ContextCaptureResult> {
  const env = { ...environment, ...params.env };
  const output = params.output;
  const unusable = () => new Error("The capture file must be a new absolute path in an existing folder");
  if (!isAbsolute(output) || resolve(output) !== output || basename(output) === "") throw unusable();
  const parent = dirname(output);
  if (await realpath(parent).catch(() => "") !== parent || !(await lstat(parent)).isDirectory()) throw unusable();
  let bundle: unknown;
  try {
    if (params.agent === "claude") {
      const projects = join(await existing(nativeHome("claude", env)), "projects");
      bundle = await captureClaudeContext(projects, await claudeProjectKey(params.cwd), params.session_id, params.live === true);
    } else {
      const home = await existing(nativeHome("codex", env));
      bundle = await captureCodexContext(home, await findCodexRollout(home, params.session_id), params.session_id, params.live === true);
    }
  } catch (error) {
    // The capture modules never quote private records in their messages.
    return { outcome: "blocked", reason: error instanceof Error ? error.message : String(error) };
  }
  const bytes = Buffer.from(JSON.stringify(bundle));
  // "wx": an existing file, even a partial earlier capture, is never replaced.
  const file = await open(output, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
  try { await file.writeFile(bytes); await file.sync(); } finally { await file.close(); }
  return { outcome: "captured", sha256: createHash("sha256").update(bytes).digest("hex"), bytes: bytes.length };
}
