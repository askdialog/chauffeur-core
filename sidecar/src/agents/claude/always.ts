// "Always allow" on a Claude permission request: the rules a chat keeps allowing from then on.
//
// Claude Code's own suggestions are not enough: it gives none in plan mode (an exploration's
// chats), where it asks for every tool that is not read-only (`decisionReasonType: "mode"`)
// whatever the session's allow rules say. So the chat keeps the rules itself, answers a call they
// cover without asking, and still hands them to Claude Code (its suggestions when it made some, a
// session rule otherwise) so that its own checks agree where they apply.

import type { PermissionUpdate } from "@anthropic-ai/claude-agent-sdk";
import { isAbsolute, resolve, sep } from "node:path";

export interface Rule {
  toolName: string;
  ruleContent?: string;
}

/** What Always allow remembers for a call, and what it hands Claude Code. */
export interface AlwaysAllow {
  rules: Rule[];
  updatedPermissions: PermissionUpdate[];
}

/**
 * Claude's suggested allow rules when it suggested any, kept as they are (they may write to the
 * workspace's `.claude/settings.local.json`, as Claude Code's terminal does). Otherwise a
 * session-only rule built here: the tool (an MCP tool by its full name), or for Bash the exact
 * command, never Bash as a whole.
 *
 * Two of Claude's suggestions are dropped, as they grant more than the call allowed always:
 * - `setMode` (in default mode, `acceptEdits` for a command that writes, an edit): it would switch
 *   the whole chat to accepting every edit;
 * - `addDirectories` (a folder outside the working directories the call touches): it would make
 *   the folder a working directory, readable by every tool without asking and open to every
 *   command's path checks. The rule does not need it: Claude Code asks again for a command that
 *   touches a folder outside them whatever its rules, and the chat answers the calls its rules
 *   cover itself.
 */
export function alwaysAllow(toolName: string, input: Record<string, unknown>, suggestions: PermissionUpdate[] | undefined): AlwaysAllow {
  const kept = (suggestions ?? []).filter((update) => update.type !== "setMode" && update.type !== "addDirectories");
  const suggested = kept.flatMap((update) => (update.type === "addRules" && update.behavior === "allow" ? update.rules : []));
  if (suggested.length > 0) return { rules: suggested, updatedPermissions: kept };
  const command = typeof input.command === "string" ? input.command.trim() : "";
  const rule: Rule = toolName === "Bash" && command ? { toolName, ruleContent: command } : { toolName };
  return {
    rules: [rule],
    updatedPermissions: [...kept, { type: "addRules", rules: [rule], behavior: "allow", destination: "session" }],
  };
}

/** A rule as Claude Code writes it in its settings: `Tool` or `Tool(content)`. */
export function ruleText(rule: Rule): string {
  return rule.ruleContent === undefined ? rule.toolName : `${rule.toolName}(${rule.ruleContent})`;
}

/**
 * Whether `rule` allows this call. Kept narrow, anything unsure is asked: a whole tool (or a whole
 * MCP server, `mcp__server`), Bash commands by exact text or `prefix:*`, never a command that
 * chains, pipes, redirects or substitutes, and reads of an absolute folder (`Read(//abs/**)`, as
 * Claude Code suggests for a file outside the working directories). Other rule contents (relative
 * paths, globs, domains) are left to Claude Code's own matching.
 */
export function ruleCovers(rule: Rule, toolName: string, input: Record<string, unknown>): boolean {
  if (rule.ruleContent === undefined) {
    if (rule.toolName === toolName) return true;
    return isMcpServer(rule.toolName) && toolName.startsWith(`${rule.toolName}__`);
  }
  if (rule.toolName === "Read") return readCovers(rule.ruleContent, toolName, input);
  if (rule.toolName !== "Bash" || toolName !== "Bash" || typeof input.command !== "string") return false;
  const command = input.command.trim();
  if (SHELL_OPERATORS.test(command)) return false;
  const content = rule.ruleContent.trim();
  if (!content.endsWith(":*")) return command === content;
  const prefix = content.slice(0, -2).trim();
  return prefix !== "" && (command === prefix || command.startsWith(`${prefix} `));
}

/** A rule written as `ruleText` writes it, read back (the daemon keeps them as text). */
export function parseRule(text: string): Rule {
  const open = text.indexOf("(");
  return open > 0 && text.endsWith(")")
    ? { toolName: text.slice(0, open), ruleContent: text.slice(open + 1, -1) }
    : { toolName: text };
}

/**
 * A `Read(//abs/dir/**)` rule (or `Read(//abs/file)`) and a call of a tool that reads files
 * (Read's `file_path`, Glob's and Grep's `path`), by its absolute path once normalized, so that
 * `..` cannot leave the folder. A pattern with other wildcards is not matched here.
 */
function readCovers(content: string, toolName: string, input: Record<string, unknown>): boolean {
  if (!content.startsWith("//")) return false;
  const key = toolName === "Read" ? "file_path" : toolName === "Glob" || toolName === "Grep" ? "path" : undefined;
  const value = key ? input[key] : undefined;
  if (typeof value !== "string" || !isAbsolute(value)) return false;
  const path = resolve(value);
  const folder = content.endsWith("/**");
  const target = content.slice(1, folder ? -3 : undefined);
  if (target === "" || /[*?[\]{}]/.test(target)) return false;
  const base = resolve(target);
  if (!folder) return path === base;
  return path === base || path.startsWith(base.endsWith(sep) ? base : `${base}${sep}`);
}

/** `mcp__server` (all of a server's tools), not `mcp__server__tool`. */
function isMcpServer(name: string): boolean {
  return name.startsWith("mcp__") && !name.slice("mcp__".length).includes("__");
}

const SHELL_OPERATORS = /[;&|<>`\n\r]|\$\(/;

/** The rules a chat's Always allow answers gave, for the life of its session. */
export class AlwaysAllowed {
  private readonly rules: Rule[] = [];

  /** `earlier`: the rules allowed always in the chat's previous agent sessions. */
  constructor(earlier: string[] = []) {
    this.add(earlier.map(parseRule));
  }

  add(rules: Rule[]): void {
    for (const rule of rules) {
      if (!this.rules.some((r) => r.toolName === rule.toolName && r.ruleContent === rule.ruleContent)) this.rules.push(rule);
    }
  }

  /** The remembered rule that allows this call, if any. */
  covering(toolName: string, input: Record<string, unknown>): Rule | undefined {
    return this.rules.find((rule) => ruleCovers(rule, toolName, input));
  }
}
