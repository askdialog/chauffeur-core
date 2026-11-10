// Programmatic SDK hooks: the `git worktree add` guard (and the read-only and write-folder guards
// of explorations and design chats), and Claude's native worktree features (WorktreeCreate /
// WorktreeRemove) routed to the daemon, which creates and tracks the worktrees.

import type { HookCallback, HookCallbackMatcher, HookEvent } from "@anthropic-ai/claude-agent-sdk";
import {
  designGuardDenial,
  NO_PLANS_DENIAL,
  planGuardDenial,
  planWriteRefusal,
  type WriteFolderKind,
  writesFiles,
} from "../../guard/plan-guard.ts";
import { DESIGN_READ_ONLY_DENIAL, READ_ONLY_DENIAL, writesOutsideTheCheckout } from "../../guard/read-only-guard.ts";
import { ADD_REPO_DENIAL, runsChauffeurAddRepo, runsGitWorktreeAdd, WORKTREE_ADD_DENIAL } from "../../guard/worktree-guard.ts";
import { errorMessage } from "../../log.ts";
import type { SessionHost } from "../agent.ts";

/** Seconds the CLI waits for the daemon to create or remove a worktree (fetch + setup can be slow). */
export const WORKTREE_HOOK_TIMEOUT_S = 600;

export type Hooks = Partial<Record<HookEvent, HookCallbackMatcher[]>>;

/**
 * The one folder a confined session writes into (an exploration's plans, a design chat's screens),
 * and the session's folder that relative paths resolve from.
 */
export interface WriteFolder {
  kind: WriteFolderKind;
  dir: string;
  cwd: string;
}

/**
 * How a session is kept from changing code: an exploration (plan mode, its plans folder) or a
 * design chat (its design folder). Undefined for a workspace's chats, which write where they like.
 */
export type Confinement = "exploration" | "design";

export function buildHooks(
  host: Pick<SessionHost, "createWorktree" | "removeWorktree" | "log">,
  confinement?: Confinement,
  folder?: WriteFolder,
): Hooks {
  const worktreeCreate: HookCallback = async (input) => {
    if (input.hook_event_name !== "WorktreeCreate") return {};
    try {
      const path = await host.createWorktree(input.name);
      return { hookSpecificOutput: { hookEventName: "WorktreeCreate", worktreePath: path } };
    } catch (error) {
      host.log.warn(`worktree ${input.name} could not be created`, errorMessage(error));
      // Throwing fails the hook, and the CLI fails the worktree creation.
      throw new Error(`Chauffeur could not create the worktree: ${errorMessage(error)}`);
    }
  };
  const worktreeRemove: HookCallback = async (input) => {
    if (input.hook_event_name !== "WorktreeRemove") return {};
    try {
      await host.removeWorktree(input.worktree_path);
      return {};
    } catch (error) {
      host.log.warn(`worktree ${input.worktree_path} could not be removed`, errorMessage(error));
      throw new Error(`Chauffeur could not remove the worktree: ${errorMessage(error)}`);
    }
  };
  const bash =
    confinement === "design"
      ? [guardWorktreeAdd, guardAddRepo, guardDesignReadOnly]
      : confinement === "exploration"
        ? [guardWorktreeAdd, guardAddRepo, guardReadOnly]
        : [guardWorktreeAdd, guardAddRepo];
  const preToolUse: HookCallbackMatcher[] = [{ matcher: "Bash", hooks: bash }];
  // Every tool, no matcher: the guard picks the file-writing ones by name itself.
  if (confinement) preToolUse.push({ hooks: [guardWrites(folder)] });
  // Publishing a screen to claude.ai is asked each time, whatever the mode decides (auto mode's
  // classifier could run it): the hook's ask comes on top of the settings' ask rule.
  if (confinement === "design") preToolUse.push({ matcher: "Artifact", hooks: [askEachTime] });
  return {
    PreToolUse: preToolUse,
    WorktreeCreate: [{ hooks: [worktreeCreate], timeout: WORKTREE_HOOK_TIMEOUT_S }],
    WorktreeRemove: [{ hooks: [worktreeRemove], timeout: WORKTREE_HOOK_TIMEOUT_S }],
  };
}

/** PreToolUse on Bash: denies `git worktree add` and names the Chauffeur command to use. */
export const guardWorktreeAdd: HookCallback = bashGuard(runsGitWorktreeAdd, WORKTREE_ADD_DENIAL);

/** PreToolUse on Bash: denies `chauffeur add-repo`, which would skip the user's say; names the tool. */
export const guardAddRepo: HookCallback = bashGuard(runsChauffeurAddRepo, ADD_REPO_DENIAL);

/** PreToolUse on Bash, in an exploration: denies git and GitHub writes, even once approved. */
export const guardReadOnly: HookCallback = bashGuard(writesOutsideTheCheckout, READ_ONLY_DENIAL);

/** PreToolUse on Bash, in a design chat: the same refusals, worded for a designer. */
export const guardDesignReadOnly: HookCallback = bashGuard(writesOutsideTheCheckout, DESIGN_READ_ONLY_DENIAL);

/**
 * PreToolUse on every tool, in a confined session: Write and Edit only into its write folder (an
 * exploration's plans, a design chat's screens), NotebookEdit nowhere, even when a permission rule
 * allows them. Without a folder (an exploration without plans folder), no file is written.
 */
export function guardWrites(folder: WriteFolder | undefined): HookCallback {
  return async (input) => {
    if (input.hook_event_name !== "PreToolUse") return {};
    const refusal = writeRefusal(folder, input.tool_name, input.tool_input, input.cwd);
    if (refusal === undefined) return {};
    return {
      hookSpecificOutput: {
        hookEventName: "PreToolUse",
        permissionDecision: "deny",
        permissionDecisionReason: refusal,
      },
    };
  };
}

/** The denial for a tool call that writes outside the write folder, or undefined. */
export function writeRefusal(folder: WriteFolder | undefined, toolName: string, toolInput: unknown, cwd?: string): string | undefined {
  if (!writesFiles(toolName)) return undefined;
  if (!folder) return NO_PLANS_DENIAL;
  const why = planWriteRefusal(toolName, toolInput, folder.dir, typeof cwd === "string" && cwd ? cwd : folder.cwd, folder.kind);
  if (why === undefined) return undefined;
  return folder.kind === "design" ? designGuardDenial(folder.dir, why) : planGuardDenial(folder.dir, why);
}

/** PreToolUse: the user is asked, whatever the permission mode or the rules say. */
export const askEachTime: HookCallback = async (input) => {
  if (input.hook_event_name !== "PreToolUse") return {};
  return {
    hookSpecificOutput: {
      hookEventName: "PreToolUse",
      permissionDecision: "ask",
      permissionDecisionReason: "A design chat asks before each publication to claude.ai",
    },
  };
};

function bashGuard(refused: (command: string) => boolean, reason: string): HookCallback {
  return async (input) => {
    if (input.hook_event_name !== "PreToolUse") return {};
    const command = (input.tool_input as { command?: unknown } | null)?.command;
    if (typeof command !== "string" || !refused(command)) return {};
    return {
      hookSpecificOutput: {
        hookEventName: "PreToolUse",
        permissionDecision: "deny",
        permissionDecisionReason: reason,
      },
    };
  };
}
