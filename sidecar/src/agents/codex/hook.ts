// Codex has no SDK hook callbacks, but it runs command hooks: the sidecar registers itself
// (`chauffeur-sidecar codex-hook pre-tool-use`) as a PreToolUse hook on Bash, so Codex gets the
// same `git worktree add` and `chauffeur add-repo` guards as Claude, the same clone guard (the
// workspace's source clones given as `--clones <base64 JSON>`), and in an exploration
// (`… pre-tool-use read-only`) or a design chat (`… pre-tool-use design`) the same read-only guard.
// Codex sends the Claude-compatible hook payload on stdin and honours a
// `permissionDecision: "deny"` answer in every approval mode.

import { DESIGN_READ_ONLY_DENIAL, READ_ONLY_DENIAL, writesOutsideTheCheckout } from "../../guard/read-only-guard.ts";
import { ADD_REPO_DENIAL, runsChauffeurAddRepo, runsGitWorktreeAdd, WORKTREE_ADD_DENIAL } from "../../guard/worktree-guard.ts";
import { cloneCommandRefusal, withRealPaths } from "../../guard/clone-guard.ts";
import type { SourceClone } from "../../protocol.ts";
import { shellScript } from "./events.ts";

export const HOOK_COMMAND = "codex-hook";
export const PRE_TOOL_USE = "pre-tool-use";

/** The read-only guard a session's hook adds, named by its flag: an exploration's, or a design chat's. */
export type HookGuard = "read-only" | "design";

const READ_ONLY_DENIALS: Record<HookGuard, string> = { "read-only": READ_ONLY_DENIAL, design: DESIGN_READ_ONLY_DENIAL };

/** The hook's answer to a PreToolUse payload: a deny decision, or nothing to let the call run. */
export function preToolUseAnswer(payload: string, guard?: HookGuard, clones: SourceClone[] = []): string | undefined {
  let input: { tool_input?: { command?: unknown }; cwd?: unknown };
  try {
    input = JSON.parse(payload) as typeof input;
  } catch {
    return undefined;
  }
  const command = commandText(input.tool_input?.command);
  if (command === undefined) return undefined;
  const reason = runsGitWorktreeAdd(command)
    ? WORKTREE_ADD_DENIAL
    : runsChauffeurAddRepo(command)
      ? ADD_REPO_DENIAL
      : guard && writesOutsideTheCheckout(command)
        ? READ_ONLY_DENIALS[guard]
        : intoClone(command, input.cwd, clones);
  if (reason === undefined) return undefined;
  return JSON.stringify({
    hookSpecificOutput: {
      hookEventName: "PreToolUse",
      permissionDecision: "deny",
      permissionDecisionReason: reason,
    },
  });
}

/** The clone guard's refusal of `command`, run from `cwd` (the payload's, else the first worktree's). */
function intoClone(command: string, cwd: unknown, clones: SourceClone[]): string | undefined {
  if (clones.length === 0) return undefined;
  const from = typeof cwd === "string" && cwd ? cwd : (clones[0]?.worktree ?? "/");
  return cloneCommandRefusal(command, from, withRealPaths(clones));
}

function commandText(command: unknown): string | undefined {
  if (typeof command === "string") return command;
  // Some Codex shell tools pass argv: ["bash", "-lc", "<script>"].
  if (Array.isArray(command) && command.every((word) => typeof word === "string")) {
    return shellScript(command.map(quote).join(" "));
  }
  return undefined;
}

/**
 * `chauffeur-sidecar codex-hook pre-tool-use [read-only|design] [--clones <base64 JSON>]`: reads
 * the payload on stdin, answers on stdout.
 */
export async function runCodexHook(subcommand: string | undefined, args: string[] = []): Promise<number> {
  if (subcommand !== PRE_TOOL_USE) {
    process.stderr.write(`unknown hook: ${subcommand ?? "(none)"}\n`);
    return 2;
  }
  const rest = [...args];
  let flag: HookGuard | undefined;
  if (rest[0] === "read-only" || rest[0] === "design") flag = rest.shift() as HookGuard;
  let clones: SourceClone[] = [];
  if (rest[0] === "--clones" && rest[1] !== undefined) {
    try {
      clones = JSON.parse(Buffer.from(rest[1], "base64").toString("utf8")) as SourceClone[];
    } catch {
      process.stderr.write("unreadable --clones\n");
      return 2;
    }
    rest.splice(0, 2);
  }
  if (rest.length > 0) {
    process.stderr.write(`unknown hook argument: ${rest[0]}\n`);
    return 2;
  }
  const answer = preToolUseAnswer(await Bun.stdin.text(), flag, clones);
  if (answer) process.stdout.write(`${answer}\n`);
  return 0;
}

/** The shell command that runs this sidecar's hook, compiled or under `bun run`. */
export function hookCommandLine(guard?: HookGuard, clones: SourceClone[] = []): string {
  const program = Bun.isStandaloneExecutable ? [process.execPath] : [process.execPath, Bun.main];
  const given = clones.length > 0 ? ["--clones", Buffer.from(JSON.stringify(clones)).toString("base64")] : [];
  return [...program, HOOK_COMMAND, PRE_TOOL_USE, ...(guard ? [guard] : []), ...given].map(quote).join(" ");
}

function quote(word: string): string {
  return /^[\w@%+=:,./-]+$/.test(word) ? word : `'${word.replaceAll("'", `'\\''`)}'`;
}
