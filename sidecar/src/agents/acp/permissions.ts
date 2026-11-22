// What Chauffeur allows, refuses or puts to the user when an ACP agent requests permission.
// Vibe has no OS sandbox. In native Smart Approve, Vibe may run a tool without requesting
// permission (docs/adr/vibe-smart-approve.md).
// They are weaker than a sandbox: a shell command allowed is not confined. So a shell command is
// never allowed by itself in an exploration or a design chat, only by the user; `bypass_permissions`
// alone runs them without asking.
//
// - Chauffeur's own tools always run: `add_repo` puts its question to the user itself.
// - Always refused: `git worktree add` and `chauffeur add-repo` (worktree-guard.ts), and an edit
//   or a shell command writing into one of the workspace's source clones (clone-guard.ts), naming
//   the worktree to use instead.
// - An exploration: no edit; shell commands that write git history or refs, or GitHub, refused, the
//   others asked.
// - A design chat: edits inside its design folder only; shell commands as in an exploration.
// - Otherwise, by permission mode: reads, searches and fetches run; edits inside the workspace's
//   folders run in `auto`, `accept_edits` and `bypass_permissions`, are asked elsewhere and in
//   `default` and `plan`; shell commands run in `bypass_permissions` only; other tools (MCP
//   servers') run but in `default` and `plan`.

import { resolve, sep } from "node:path";
import type { PermissionMode } from "../../protocol.ts";
import { clonePathRefusal, cloneCommandRefusal, type GuardedClone } from "../../guard/clone-guard.ts";
import { designGuardDenial, planPathRefusal } from "../../guard/plan-guard.ts";
import { DESIGN_READ_ONLY_DENIAL, READ_ONLY_DENIAL, writesOutsideTheCheckout } from "../../guard/read-only-guard.ts";
import { ADD_REPO_DENIAL, runsChauffeurAddRepo, runsGitWorktreeAdd, WORKTREE_ADD_DENIAL } from "../../guard/worktree-guard.ts";
import type { AcpToolCall } from "./events.ts";

export type Decision = { type: "allow" } | { type: "deny"; reason: string } | { type: "ask" };

export interface PermissionContext {
  mode: PermissionMode;
  readOnly: boolean;
  designDir: string | undefined;
  /** The session's folder, against which a relative path resolves. */
  cwd: string;
  /** The folders edits may go to without asking: the workspace folder and its repos. */
  roots: string[];
  /** The workspace's source clones, the user's: never written. */
  clones: GuardedClone[];
}

const EXPLORATION_EDIT_DENIAL =
  "This Chauffeur workspace is an exploration, read-only: files are not edited. Read and explain; to change code, " +
  "the user creates a Chauffeur workspace.";

const ALLOW: Decision = { type: "allow" };
const ASK: Decision = { type: "ask" };

/** Chauffeur's own tools: allowed by the chat itself (`add_repo` asks the user on its own). */
const CHAUFFEUR_TOOL_PREFIX = "mcp__chauffeur__";

export function decide(tool: AcpToolCall, context: PermissionContext): Decision {
  if (tool.name.startsWith(CHAUFFEUR_TOOL_PREFIX) && !context.designDir) return ALLOW;
  // A call the agent said nothing of (Vibe names one by its tool alone when it has not announced
  // it): no guard can read it, so the user decides.
  if (tool.kind === null && tool.title === null) return ASK;
  const command = tool.kind === "execute" ? (tool.command ?? "") : undefined;
  if (command !== undefined) {
    if (runsGitWorktreeAdd(command)) return deny(WORKTREE_ADD_DENIAL);
    if (runsChauffeurAddRepo(command)) return deny(ADD_REPO_DENIAL);
    // Where the process runs and the variables it is given (`GIT_WORK_TREE`…) hold for its whole line,
    // as exported ones do.
    const from = tool.cwd ? resolve(context.cwd, tool.cwd) : context.cwd;
    const assigned = Object.entries(tool.env ?? {}).map(([name, value]) => ` ${name}='${value.replaceAll("'", `'\\''`)}'`);
    const line = assigned.length > 0 ? `export${assigned.join("")}; ${command}` : command;
    const intoClone = cloneCommandRefusal(line, from, context.clones);
    if (intoClone) return deny(intoClone);
  }
  const edits = tool.kind === "edit" || tool.kind === "delete" || tool.kind === "move";
  if (edits) {
    for (const path of tool.paths) {
      const intoClone = clonePathRefusal(path, context.cwd, context.clones);
      if (intoClone) return deny(intoClone);
    }
  }
  if (context.designDir) {
    if (edits) {
      if (tool.paths.length === 0) return deny(designGuardDenial(context.designDir, "an edit that names no file"));
      for (const path of tool.paths) {
        const refused = planPathRefusal(path, context.designDir, context.cwd, "design");
        if (refused) return deny(designGuardDenial(context.designDir, refused));
      }
      return ALLOW;
    }
    if (command !== undefined) return writesOutsideTheCheckout(command) ? deny(DESIGN_READ_ONLY_DENIAL) : ASK;
    return ALLOW;
  }
  if (context.readOnly) {
    if (edits) return deny(EXPLORATION_EDIT_DENIAL);
    if (command !== undefined) return writesOutsideTheCheckout(command) ? deny(READ_ONLY_DENIAL) : ASK;
    return ALLOW;
  }
  const mode = context.mode;
  if (mode === "bypass_permissions") return ALLOW;
  if (command !== undefined) return ASK;
  if (edits) {
    if (mode === "default" || mode === "plan") return ASK;
    return tool.paths.length > 0 && tool.paths.every((path) => inside(path, context)) ? ALLOW : ASK;
  }
  switch (tool.kind) {
    case "read":
    case "search":
    case "fetch":
    case "think":
    case "switch_mode":
      return ALLOW;
    default:
      return mode === "default" || mode === "plan" ? ASK : ALLOW;
  }
}

function deny(reason: string): Decision {
  return { type: "deny", reason };
}

function inside(path: string, context: PermissionContext): boolean {
  if (path.split(/[\\/]/).includes("..")) return false;
  const absolute = resolve(context.cwd, path);
  return context.roots.some((root) => absolute === root || absolute.startsWith(root.endsWith(sep) ? root : root + sep));
}
