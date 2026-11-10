// "Always allow" on a Codex approval: what Codex's own `acceptForSession` covers, written as rules
// the card shows and the chat keeps.
//
// Codex remembers a session approval by key, in memory, for the life of its app-server (codex-rs
// `with_cached_approval`): it does not ask again for a call whose keys were all approved for the
// session.
// - A command: the command itself (its wrapper shell aside) in that working directory (also its
//   environment, terminal mode and sandbox escalation, which the request does not show).
// - A file change: each file it touches, a move's destination included. A later change touching
//   only approved files is not asked; one touching another file is. A change asked again after the
//   sandbox refused it (it comes with a reason) is never remembered.
// - Nothing else gets a rule. Input to a running command and a command's subcommand (zsh fork) are
//   approved once whatever the answer. Network access is remembered by host and port, and the
//   request does not show the port; extra permissions are granted for the session as asked.
//
// An MCP confirmation (elicitation) with nothing to fill in gets a rule of the chat's own, its
// server and message (`cua_repl: Allow Computer Use to use "Google Chrome"?`): Codex has no session
// approval for it, so the chat accepts the same confirmation itself each time Codex asks again.
//
// The chat keeps the rules too: it answers what they cover when Codex asks again anyway (a variant
// the request does not show, a new app-server after the session was closed).

import type { CommandExecutionRequestApprovalParams, FileChangeRequestApprovalParams, FileUpdateChange } from "./protocol.ts";
import { CODEX_TOOLS, shellScript } from "./events.ts";

/** The rules Always allow on a command approval stands for: none where Codex would ask again. */
export function commandRules(p: CommandExecutionRequestApprovalParams): string[] {
  if (p.kind !== "command" || p.approvalId || p.networkApprovalContext || !p.command || !p.cwd) return [];
  return [`${CODEX_TOOLS.command}(${shellScript(p.command)}) in ${p.cwd}`];
}

/** The rules Always allow on a file change approval stands for: one per file it touches. */
export function fileRules(p: FileChangeRequestApprovalParams, changes: FileUpdateChange[]): string[] {
  if (p.reason || p.grantRoot || changes.length === 0) return [];
  const paths = changes.flatMap((change) => (change.kind.type === "update" && change.kind.move_path ? [change.path, change.kind.move_path] : [change.path]));
  return [...new Set(paths)].map((path) => `${CODEX_TOOLS.fileChange}(${path})`);
}

/** The rules a chat's Always allow answers gave, for the life of its session and the next ones. */
export class CodexAlwaysAllowed {
  private readonly rules = new Set<string>();

  /** `earlier`: the rules allowed always in the chat's previous agent sessions. */
  constructor(earlier: string[] = []) {
    this.add(earlier);
  }

  add(rules: string[]): void {
    for (const rule of rules) this.rules.add(rule);
  }

  /** Whether a request standing for these rules is allowed already: all of them, as Codex has it. */
  covers(rules: string[]): boolean {
    return rules.length > 0 && rules.every((rule) => this.rules.has(rule));
  }

  /** The rules of an open request they now cover, for `PendingRequests.allowCovered`. */
  covering(rules: string[] | undefined): string[] | undefined {
    return rules && this.covers(rules) ? rules : undefined;
  }
}
