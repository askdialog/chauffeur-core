// Keeps agents out of the user's source clones (docs/adr/claude-code-import.md): a workspace's
// worktrees are made from them, and nothing else stops an agent from changing the clone itself
// (auto mode leaves it to a classifier, shell commands are not checked). A file tool writing into a
// clone is refused with the matching path in the workspace's worktree; a shell command that writes
// and names a clone is refused the same way. Reads stay allowed.
//
// A guard rail, not a sandbox: it reads a shell command as the other guards do, and catches what an
// agent writes (a redirection, a file command, a git command that changes the clone). A program
// that writes where it runs (`npm install` once in the clone) is not caught.

import { realpathSync } from "node:fs";
import { homedir } from "node:os";
import { isAbsolute, join, relative, resolve } from "node:path";
import { gitSubcommand, simpleCommands } from "./worktree-guard.ts";

/** A clone of one of the workspace's repos, and its worktree in the workspace. */
export interface SourceClone {
  clone: string;
  worktree: string;
}

export function cloneWriteDenial(path: string, clone: SourceClone): string {
  return (
    `${path} is in ${clone.clone}, the user's own clone of this repo: agents never change it. ` +
    `Write ${translate(path, clone)} instead, in this workspace's worktree.`
  );
}

export function cloneCommandDenial(clone: SourceClone): string {
  return (
    `This command writes into ${clone.clone}, the user's own clone of this repo: agents never change it. ` +
    `Run it in this workspace's worktree, ${clone.worktree}, instead.`
  );
}

/** The clones as given and, when it differs, as resolved (a symlinked folder: /tmp → /private/tmp). */
export function withRealPaths(clones: SourceClone[]): SourceClone[] {
  const real = (path: string) => {
    try {
      return realpathSync(path);
    } catch {
      return path;
    }
  };
  const resolved = clones.map(({ clone, worktree }) => ({ clone: real(clone), worktree: real(worktree) }));
  return [...clones, ...resolved.filter((r, i) => r.clone !== clones[i]?.clone || r.worktree !== clones[i]?.worktree)];
}

/** The file tools and the input naming the file they write. */
const FILE_TOOLS: Record<string, string> = {
  Write: "file_path",
  Edit: "file_path",
  MultiEdit: "file_path",
  NotebookEdit: "notebook_path",
};

/** The denial for a file tool writing into a clone, or undefined. */
export function cloneWriteRefusal(toolName: string, toolInput: unknown, cwd: string, clones: SourceClone[]): string | undefined {
  const key = FILE_TOOLS[toolName];
  if (key === undefined) return undefined;
  const written = (toolInput as Record<string, unknown> | null)?.[key];
  if (typeof written !== "string" || !written) return undefined;
  const path = at(cwd, written);
  const clone = cloneOf(path, clones);
  return clone && cloneWriteDenial(path, clone);
}

/** Programs that change every path they are given. */
const WRITERS = new Set(["rm", "rmdir", "unlink", "mkdir", "touch", "mv", "chmod", "chown", "chgrp", "truncate", "tee", "shred"]);
/** Programs that read their sources and write their last path. */
const COPIERS = new Set(["cp", "rsync", "install", "ln", "scp"]);
/** Programs that change their files with `-i`. */
const IN_PLACE = new Set(["sed", "perl", "gsed"]);
/** Git subcommands that only read. */
const GIT_READS = new Set([
  "status",
  "log",
  "diff",
  "show",
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
  "help",
  "version",
]);
/** Git subcommands that only list when given no name. */
const GIT_LISTS = new Set(["branch", "tag", "remote", "worktree"]);

/** The denial for a shell command that writes into a clone, or undefined. */
export function cloneCommandRefusal(command: string, cwd: string, clones: SourceClone[]): string | undefined {
  let dir = cwd;
  for (const { program, args } of simpleCommands(command)) {
    // `cd` moves the commands after it in the same line.
    if (program === "cd") {
      const target = args.find((arg) => !arg.startsWith("-"));
      dir = target === undefined ? homedir() : at(dir, target);
      continue;
    }
    const clone = writtenClone(program, args, dir, clones);
    if (clone) return cloneCommandDenial(clone);
  }
  return undefined;
}

function writtenClone(program: string, args: string[], dir: string, clones: SourceClone[]): SourceClone | undefined {
  const inClone = (word: string) => cloneOf(at(dir, word), clones);
  for (const target of redirections(args)) {
    const clone = inClone(target);
    if (clone) return clone;
  }
  const words = args.filter((arg) => !arg.startsWith("-") && !isRedirection(arg));
  if (WRITERS.has(program)) return firstClone(words, inClone);
  if (COPIERS.has(program)) {
    const last = words.at(-1);
    return last === undefined ? undefined : inClone(last);
  }
  if (IN_PLACE.has(program) && args.some((arg) => /^-\w*i/.test(arg))) return firstClone(words, inClone);
  if (program === "dd") return firstClone(args.filter((arg) => arg.startsWith("of=")).map((arg) => arg.slice(3)), inClone);
  if (program === "git") return gitWrite(args, dir, clones);
  return undefined;
}

/** The clone a git command changes: the one it runs in (`-C`, `--work-tree`, `cd`), unless it only reads. */
function gitWrite(args: string[], dir: string, clones: SourceClone[]): SourceClone | undefined {
  let where = dir;
  for (let i = 0; i < args.length; i++) {
    const arg = args[i] as string;
    if (arg === "-C" && args[i + 1] !== undefined) where = at(where, args[i + 1] as string);
    for (const prefix of ["--work-tree=", "--git-dir="]) {
      if (arg.startsWith(prefix)) where = at(dir, arg.slice(prefix.length));
    }
  }
  const clone = cloneOf(where, clones);
  if (!clone) return undefined;
  const { name, args: rest } = gitSubcommand(args);
  if (name === undefined || GIT_READS.has(name)) return undefined;
  if (GIT_LISTS.has(name) && rest.every((arg) => arg.startsWith("-") || arg === "list")) return undefined;
  if (name === "stash" && (rest[0] === "list" || rest[0] === "show")) return undefined;
  if (name === "config" && rest.some((arg) => arg === "--get" || arg === "--list" || arg === "-l" || arg.startsWith("--get-"))) return undefined;
  return clone;
}

function firstClone(words: string[], inClone: (word: string) => SourceClone | undefined): SourceClone | undefined {
  for (const word of words) {
    const clone = inClone(word);
    if (clone) return clone;
  }
  return undefined;
}

/** A redirection's operator, alone or with its target: `>`, `>>`, `2>`, `&>`, `>file`, `x>>file`. */
const REDIRECTION = /^[^>]*?(?:\d|&)?>>?(?!&)(.*)$/;

function isRedirection(word: string): boolean {
  return REDIRECTION.test(word);
}

/** The files the command's output goes to. */
function redirections(args: string[]): string[] {
  const targets: string[] = [];
  for (let i = 0; i < args.length; i++) {
    const match = REDIRECTION.exec(args[i] as string);
    if (!match) continue;
    const target = match[1] || args[i + 1];
    if (target && !target.startsWith("&")) targets.push(target);
  }
  return targets;
}

/** The clone a path is in, unless it is in a worktree of the workspace (a worktree inside its clone). */
function cloneOf(path: string, clones: SourceClone[]): SourceClone | undefined {
  if (clones.some(({ worktree }) => inside(path, worktree))) return undefined;
  return clones.find(({ clone }) => inside(path, clone));
}

function translate(path: string, { clone, worktree }: SourceClone): string {
  return join(worktree, relative(clone, path));
}

function inside(path: string, dir: string): boolean {
  const rel = relative(dir, path);
  return rel === "" || (!rel.startsWith("..") && !isAbsolute(rel));
}

/** A path as a shell or a tool reads it from `dir`: `~` is the home folder. */
function at(dir: string, path: string): string {
  if (path === "~") return homedir();
  if (path.startsWith("~/")) return join(homedir(), path.slice(2));
  return resolve(dir, path);
}
