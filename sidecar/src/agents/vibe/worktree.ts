import { isAbsolute } from "node:path";
import { runsGitWorktreeAdd, splitCommands } from "../../guard/worktree-guard.ts";

type Observation = { type: "created"; path: string } | { type: "unresolved" };

/** Finds a literal target to verify against Git, including when Vibe reports only output text. */
export function observedWorktree(output: unknown, input: unknown, sessionCwd: string): Observation | undefined {
  const result = object(output);
  const args = object(input);
  const command = typeof result?.command === "string" ? result.command : args?.command;
  if (typeof command !== "string" || !runsGitWorktreeAdd(command)) return undefined;
  if (result?.exit_code === null || result?.background === true) return { type: "unresolved" };
  const exitCode = result?.exit_code ?? result?.returncode;
  if (typeof exitCode === "number" && exitCode !== 0) return undefined;
  if (!args ||
      /[\n\r\x00$!~*?;|&<>(){}\[\]#\\`]/.test(command) || args.env || args.shell || args.background === true) {
    return { type: "unresolved" };
  }

  const [words] = splitCommands(command);
  if (!words || words[0] !== "git") return { type: "unresolved" };
  let cwd = typeof args.cwd === "string" ? relativeTo(sessionCwd, args.cwd) : sessionCwd;
  let i = 1;
  while (words[i] === "-C") {
    const directory = words[i + 1];
    if (directory === undefined) return { type: "unresolved" };
    if (directory) cwd = relativeTo(cwd, directory);
    i += 2;
  }
  if (words[i++] !== "worktree" || words[i++] !== "add") return { type: "unresolved" };
  while (words[i] === "--detach" || words[i] === "--no-checkout" || words[i] === "-b") {
    i += words[i] === "-b" ? 2 : 1;
  }
  if (words[i] === "--") i++;
  const target = words[i];
  if (!target || target.startsWith("-") || words.length > i + 2) return { type: "unresolved" };
  return { type: "created", path: relativeTo(cwd, target) };
}

/** Keep '..' intact so the daemon can resolve it against the real filesystem, including symlinks. */
function relativeTo(cwd: string, target: string): string {
  return isAbsolute(target) ? target : `${cwd}/${target}`;
}

function object(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : undefined;
}
