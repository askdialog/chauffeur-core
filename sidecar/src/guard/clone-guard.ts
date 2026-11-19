// Keeps agents out of the user's source clones (docs/adr/claude-code-import.md): a workspace's
// worktrees are made from them, and nothing else stops an agent from changing the clone itself
// (auto mode leaves it to a classifier, shell commands are not checked). A file tool writing into a
// clone is refused with the matching path in the workspace's worktree; a shell command that writes
// and names a clone is refused the same way. Reads stay allowed, and so do the writes git makes in
// a clone's `.git` for its worktrees (a commit from the worktree).
//
// A guard rail, not a sandbox: it reads a shell command as the other guards do, and catches what an
// agent writes (a redirection, a file command, a git command that changes the clone). A program
// that writes where it runs (`npm install` once in the clone) is not caught.

import { readFileSync, realpathSync } from "node:fs";
import { homedir } from "node:os";
import { isAbsolute, join, relative, resolve } from "node:path";
import type { SourceClone } from "../protocol.ts";
import { simpleCommands } from "./worktree-guard.ts";

/** A source clone, and its worktree's git folder in the clone's `.git` when it has one. */
export interface GuardedClone extends SourceClone {
  gitdir?: string;
}

/**
 * The clones as given and, when it differs, as resolved (a symlinked folder: /tmp → /private/tmp),
 * each with its worktree's git folder: the one place in the clone that belongs to the workspace.
 */
export function withRealPaths(clones: SourceClone[]): GuardedClone[] {
  const real = (path: string) => {
    try {
      return realpathSync(path);
    } catch {
      return path;
    }
  };
  const all = clones.flatMap(({ clone, worktree }) => {
    const resolved = { clone: real(clone), worktree: real(worktree) };
    return resolved.clone === clone && resolved.worktree === worktree ? [{ clone, worktree }] : [{ clone, worktree }, resolved];
  });
  return all.map((clone) => ({ ...clone, gitdir: gitdirOf(clone.worktree) }));
}

/** A worktree's `.git` file names its git folder: `gitdir: <path>`. */
function gitdirOf(worktree: string): string | undefined {
  try {
    const line = readFileSync(join(worktree, ".git"), "utf8").trim();
    return line.startsWith("gitdir: ") ? resolve(worktree, line.slice("gitdir: ".length)) : undefined;
  } catch {
    return undefined;
  }
}

function writeDenial(path: string, clone: GuardedClone): string {
  const instead = inside(path, join(clone.clone, ".git"))
    ? "Run git in this workspace's worktree instead"
    : `Write ${join(clone.worktree, relative(clone.clone, path))} instead, in this workspace's worktree`;
  return `${path} is in ${clone.clone}, the user's own clone of this repo: agents never change it. ${instead}.`;
}

function commandDenial(clone: GuardedClone): string {
  return (
    `This command writes into ${clone.clone}, the user's own clone of this repo: agents never change it. ` +
    `Run it in this workspace's worktree, ${clone.worktree}, instead.`
  );
}

/** The file tools and the input naming the file they write. */
const FILE_TOOLS: Record<string, string> = {
  Write: "file_path",
  Edit: "file_path",
  MultiEdit: "file_path",
  NotebookEdit: "notebook_path",
};

/** The denial for a file tool writing into a clone, or undefined. */
export function cloneWriteRefusal(toolName: string, toolInput: unknown, cwd: string, clones: GuardedClone[]): string | undefined {
  const key = FILE_TOOLS[toolName];
  if (key === undefined || typeof toolInput !== "object" || toolInput === null) return undefined;
  const written: unknown = Reflect.get(toolInput, key);
  if (typeof written !== "string" || !written) return undefined;
  const path = at(cwd, written);
  const clone = cloneOf(path, clones);
  return clone && writeDenial(path, clone);
}

/** Programs that change every path they are given. */
const WRITERS = new Set(["rm", "rmdir", "unlink", "mkdir", "touch", "mv", "chmod", "chown", "chgrp", "truncate", "tee", "shred"]);
/** Programs that read their sources and write their last path. */
const COPIERS = new Set(["cp", "rsync", "install", "ln", "scp"]);
/** Programs that change their files with `-i` or `--in-place`. */
const IN_PLACE = new Set(["sed", "perl", "gsed"]);
/** Git subcommands that only read. */
const GIT_READS = new Set([
  "status",
  "log",
  "diff",
  "diff-tree",
  "diff-index",
  "diff-files",
  "show",
  "show-branch",
  "blame",
  "grep",
  "ls-files",
  "ls-tree",
  "ls-remote",
  "rev-parse",
  "rev-list",
  "describe",
  "shortlog",
  "cat-file",
  "for-each-ref",
  "show-ref",
  "name-rev",
  "whatchanged",
  "merge-base",
  "cherry",
  "range-diff",
  "count-objects",
  "check-ignore",
  "check-attr",
  "var",
  "help",
  "version",
]);
/** Options of `git branch` and `git tag` that list rather than create. */
const LISTING = new Set(["--list", "-l", "--contains", "--no-contains", "--merged", "--no-merged", "--points-at", "--show-current", "--format"]);
/** Git's global options that take a value as the next word. */
const GIT_VALUE_OPTIONS = new Set(["-C", "-c", "--git-dir", "--work-tree", "--namespace", "--exec-path", "--config-env"]);
/** Shell operators that end a command at the top level of a command line. */
const OPERATORS = ["&&", "||", ";", "\n", "|", "&"];

/** The denial for a shell command that writes into a clone, or undefined. */
export function cloneCommandRefusal(command: string, cwd: string, clones: GuardedClone[]): string | undefined {
  let dir = cwd;
  for (const { text, then } of topLevel(command)) {
    const commands = simpleCommands(text);
    // A `cd` alone moves the commands after it on the line; one in a group, a substitution or a
    // pipe stays there.
    const only = commands.length === 1 ? commands[0] : undefined;
    if (only && isCd(only.program) && /^\s*(?:cd|pushd)\b/.test(text) && then !== "|") {
      dir = cdTarget(dir, only.args);
      continue;
    }
    let local = dir;
    for (const { program, args } of commands) {
      if (isCd(program)) {
        local = cdTarget(local, args);
        continue;
      }
      const clone = writtenClone(program, args, local, clones);
      if (clone) return commandDenial(clone);
    }
    // A substitution inside double quotes runs too, where the parser keeps it as text.
    for (const inner of quotedSubstitutions(text)) {
      const refusal = cloneCommandRefusal(inner, local, clones);
      if (refusal) return refusal;
    }
  }
  return undefined;
}

/** The commands substituted inside double quotes, `"$( … )"` or `"` … `"`; single quotes substitute nothing. */
function quotedSubstitutions(text: string): string[] {
  const found: string[] = [];
  let quote: string | undefined;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (c === "\\") {
      i++;
      continue;
    }
    if (quote === "'") {
      if (c === "'") quote = undefined;
      continue;
    }
    if (c === "'" && quote === undefined) quote = c;
    else if (c === '"') quote = quote === '"' ? undefined : '"';
    else if (quote === '"' && text.startsWith("$(", i)) {
      let depth = 1;
      let j = i + 2;
      for (; j < text.length && depth > 0; j++) {
        if (text[j] === "(") depth++;
        else if (text[j] === ")") depth--;
      }
      found.push(text.slice(i + 2, depth === 0 ? j - 1 : j));
      i = j - 1;
    } else if (quote === '"' && c === "`") {
      const end = text.indexOf("`", i + 1);
      found.push(text.slice(i + 1, end === -1 ? undefined : end));
      if (end === -1) break;
      i = end;
    }
  }
  return found;
}

function isCd(program: string): boolean {
  return program === "cd" || program === "pushd";
}

function cdTarget(dir: string, args: string[]): string {
  const target = args.find((arg) => !arg.startsWith("-"));
  return target === undefined ? homedir() : at(dir, target);
}

/**
 * The command line's top-level commands, each with the operator after it: groups `( … )`,
 * substitutions `$( … )` and backticks stay inside the command they are in, as quoted text does.
 */
function topLevel(command: string): { text: string; then: string }[] {
  const parts: { text: string; then: string }[] = [];
  let depth = 0;
  let backtick = false;
  let quote: string | undefined;
  let start = 0;
  for (let i = 0; i < command.length; i++) {
    const c = command[i];
    if (c === "\\") {
      i++;
      continue;
    }
    if (quote) {
      if (c === quote) quote = undefined;
      continue;
    }
    if (c === "'" || c === '"') {
      quote = c;
      continue;
    }
    if (c === "(") depth++;
    else if (c === ")") depth = Math.max(0, depth - 1);
    else if (c === "`") backtick = !backtick;
    if (depth > 0 || backtick) continue;
    const operator = OPERATORS.find((op) => command.startsWith(op, i));
    // `2>&1` and `&>` redirect: no command ends there.
    if (operator === undefined || (operator === "&" && (command[i - 1] === ">" || command[i + 1] === ">"))) continue;
    parts.push({ text: command.slice(start, i), then: operator });
    i += operator.length - 1;
    start = i + 1;
  }
  parts.push({ text: command.slice(start), then: "" });
  return parts.filter((part) => part.text.trim() !== "");
}

function writtenClone(program: string, args: string[], dir: string, clones: GuardedClone[]): GuardedClone | undefined {
  const inClone = (word: string) => cloneOf(at(dir, word), clones);
  for (const target of redirections(args)) {
    const clone = inClone(target);
    if (clone) return clone;
  }
  const words = args.filter((arg) => !arg.startsWith("-") && redirection(arg) === undefined);
  if (WRITERS.has(program)) return firstClone(words, inClone);
  if (COPIERS.has(program)) {
    const last = words.at(-1);
    return last === undefined ? undefined : inClone(last);
  }
  if (IN_PLACE.has(program) && args.some((arg) => /^-\w*i/.test(arg) || arg.startsWith("--in-place"))) return firstClone(words, inClone);
  if (program === "dd") return firstClone(args.filter((arg) => arg.startsWith("of=")).map((arg) => arg.slice(3)), inClone);
  if (program === "find" && (args.includes("-delete") || args.some((arg, i) => arg === "-exec" && args[i + 1] === "rm"))) {
    const roots = args.slice(0, Math.max(0, args.findIndex((arg) => arg.startsWith("-"))));
    return firstClone(roots, inClone);
  }
  if (program === "git") return gitWrite(args, dir, clones);
  return undefined;
}

/** The clone a git command changes: the one it runs in (`-C`, `--work-tree`, `--git-dir`, `cd`), unless it only reads. */
function gitWrite(args: string[], dir: string, clones: GuardedClone[]): GuardedClone | undefined {
  let where = dir;
  let i = 0;
  for (; i < args.length; i++) {
    const arg = args[i];
    const value = args[i + 1];
    if (arg === undefined || !arg.startsWith("-")) break;
    const [name, attached] = arg.startsWith("--") && arg.includes("=") ? [arg.slice(0, arg.indexOf("=")), arg.slice(arg.indexOf("=") + 1)] : [arg, undefined];
    const given = attached ?? (GIT_VALUE_OPTIONS.has(name) ? value : undefined);
    if (given !== undefined && name === "-C") where = at(where, given);
    if (given !== undefined && (name === "--work-tree" || name === "--git-dir")) where = at(dir, given);
    if (attached === undefined && GIT_VALUE_OPTIONS.has(name)) i++;
  }
  const sub = args[i];
  const rest = args.slice(i + 1);
  // `--output` (diff, log, show) and `--output-directory` (format-patch) write wherever they name.
  for (const [k, arg] of rest.entries()) {
    const name = arg.split("=")[0];
    if (name !== "--output" && name !== "--output-directory") continue;
    const target = arg.includes("=") ? arg.slice(arg.indexOf("=") + 1) : rest[k + 1];
    const written = target === undefined ? undefined : cloneOf(at(where, target), clones);
    if (written) return written;
  }
  const clone = cloneOf(where, clones);
  if (!clone) return undefined;
  const words = rest.filter((arg) => !arg.startsWith("-"));
  if (sub === undefined || GIT_READS.has(sub)) return undefined;
  if ((sub === "branch" || sub === "tag") && (words.length === 0 || rest.some((arg) => LISTING.has(arg.split("=")[0] ?? arg)))) return undefined;
  if (sub === "remote" && (words.length === 0 || words[0] === "get-url" || words[0] === "show")) return undefined;
  if (sub === "stash" && (words[0] === "list" || words[0] === "show")) return undefined;
  if (sub === "worktree" && words[0] === "list") return undefined;
  if (sub === "reflog" && words[0] !== "expire" && words[0] !== "delete") return undefined;
  const configWrite = rest.some((arg) => /^--(?:unset|remove-section)/.test(arg));
  if (sub === "config" && ((words.length === 1 && !configWrite) || rest.some((arg) => arg === "--list" || arg === "-l" || arg.startsWith("--get")))) return undefined;
  return clone;
}

function firstClone(words: string[], inClone: (word: string) => GuardedClone | undefined): GuardedClone | undefined {
  for (const word of words) {
    const clone = inClone(word);
    if (clone) return clone;
  }
  return undefined;
}

/**
 * A redirection's target, when the word is one: the operator alone (`>`, `>>`, `2>`, `&>`), its
 * target the next word, or the operator with its target attached (`>file`, `x>>file`, `2>file`).
 * A quoted word with `>` inside (`'=>'`, `'.x > 1'`) is none: the parser dropped its quotes, but a
 * redirection's target has no space and follows its operator.
 */
function redirection(word: string): { attached: string } | { next: true } | undefined {
  if (/^(?:\d|&)?>>?$/.test(word)) return { next: true };
  const match = /^[^\s>]*?(?:\d|&)?>>?([^\s>&]\S*)$/.exec(word);
  return match?.[1] ? { attached: match[1] } : undefined;
}

/** The files the command's output goes to. */
function redirections(args: string[]): string[] {
  const targets: string[] = [];
  args.forEach((arg, i) => {
    const found = redirection(arg);
    const target = found && ("attached" in found ? found.attached : args[i + 1]);
    if (target && !target.startsWith("&")) targets.push(target);
  });
  return targets;
}

/** The clone a path is in, unless it is in a worktree of the workspace or its git folder. */
function cloneOf(path: string, clones: GuardedClone[]): GuardedClone | undefined {
  if (clones.some(({ worktree, gitdir }) => inside(path, worktree) || (gitdir !== undefined && inside(path, gitdir)))) return undefined;
  return clones.find(({ clone }) => inside(path, clone));
}

function inside(path: string, dir: string): boolean {
  const rel = relative(dir, path);
  return rel === "" || (!rel.startsWith("..") && !isAbsolute(rel));
}

/** A path as a shell or a tool reads it from `dir`: `~`, `$HOME` and `${HOME}` are the home folder. */
function at(dir: string, path: string): string {
  const home = /^(?:~|\$HOME|\$\{HOME\})(?=\/|$)/.exec(path);
  if (home) return join(homedir(), path.slice(home[0].length));
  return resolve(dir, path);
}
