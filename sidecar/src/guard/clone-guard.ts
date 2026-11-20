// Keeps agents out of the user's source clones (docs/adr/claude-code-import.md): a workspace's
// worktrees are made from them, and nothing else stops an agent from changing the clone itself
// (auto mode leaves it to a classifier, shell commands are not checked). A file tool writing into a
// clone is refused with the matching path in the workspace's worktree.
//
// A shell command is judged the other way round: one that names a clone (a path in it, a folder
// holding it, or the clone as the folder it runs in) is refused unless it is a read the guard
// knows (`cat`, `grep`, `ls`, `find` with no action, `git log`…), a copy out of the clone, or a
// write whose target it can place outside. A redirection into a clone is refused whatever the
// command. When the line names a clone, a command that may change files with a value only known
// when it runs (a variable it does not set, a command's output, `xargs`' input) is refused too.
// The writes git makes in a clone's `.git` for its worktrees (a commit from the worktree) stay
// allowed. A refusal costs little: the worktree holds the same files.
//
// A guard rail, not a sandbox: a program whose own code writes into the clone, the command line
// not naming it (a script, a build step run from elsewhere), is not caught.

import { readFileSync, realpathSync } from "node:fs";
import { homedir } from "node:os";
import { basename, isAbsolute, join, relative, resolve } from "node:path";
import type { SourceClone } from "../protocol.ts";
import { type Command, type Group, type List, readLine, type Word } from "./shell-line.ts";

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
    `This command changes, or may change, files in ${clone.clone}, the user's own clone of this repo: ` +
    "agents never change it, and only reads run there (cat, grep, ls, git status, log, diff…). " +
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

/** The denial for a shell command that changes, or may change, a clone, or undefined. */
export function cloneCommandRefusal(command: string, cwd: string, clones: GuardedClone[]): string | undefined {
  const judge = new Judge(clones);
  try {
    judge.list(readLine(command), { dir: cwd, previous: undefined, stack: [], vars: new Map() });
  } catch {
    // Nested past what the stack holds: refused rather than let through unread.
    return TOO_DEEP;
  }
  const { changed, named, blind } = judge.verdict;
  const clone = changed ?? (blind ? named : undefined);
  return clone && commandDenial(clone);
}

const TOO_DEEP = "This command line is nested too deeply for the guard that keeps agents out of the user's clones to read it: split it into simpler commands.";

/** A word as the line gives it. */
interface Value {
  text: string;
  /** No part of it is only known when the line runs (a variable not set on the line, a command's output). */
  known: boolean;
  /** It holds a wildcard (`*`, `?`, `[`, `{`): the paths it names start with `prefix`. */
  glob: boolean;
  /** The text before the first unknown part or wildcard. */
  prefix: string;
}

/** Where a command of the line runs, and what the line set before it. */
interface State {
  /** Undefined after a `cd` to a value unknown. */
  dir: string | undefined;
  previous: string | undefined;
  stack: (string | undefined)[];
  /** The variables the line set: undefined when only known as it runs. */
  vars: Map<string, string | undefined>;
  /** A clone an environment variable names (`GIT_DIR`, `CARGO_TARGET_DIR`, `DESTDIR`…): a program may write there. */
  env?: GuardedClone;
}

function copy(state: State): State {
  return { ...state, stack: [...state.stack], vars: new Map(state.vars) };
}

function exact(text: string): Value {
  return { text, known: true, glob: false, prefix: text };
}

function isExact(value: Value): boolean {
  return value.known && !value.glob;
}

/** The value from `from` on: an option's attached value (`--file=x`, `-tdir`). */
function after(value: Value, from: number): Value {
  return { ...value, text: value.text.slice(from), prefix: value.prefix.slice(from) };
}

/** An option's name and its attached value: `--file=x` is `--file` and `x`. */
function option(text: string): [string, string | undefined] {
  const eq = text.indexOf("=");
  return text.startsWith("--") && eq > 0 ? [text.slice(0, eq), text.slice(eq + 1)] : [text, undefined];
}

/** Words that open or close a compound command: the command is what follows them. */
const RESERVED = new Set(["!", "if", "then", "elif", "else", "fi", "do", "done", "while", "until", "{", "}", "esac", "function"]);
/** Programs that only read, whatever they are given (the exceptions are in `onlyReads`). */
const READERS = new Set([
  ...["cat", "head", "tail", "less", "more", "grep", "egrep", "fgrep", "zgrep", "zcat", "rg", "ag", "ack", "ls", "tree", "wc"],
  ...["diff", "cmp", "comm", "stat", "file", "du", "df", "realpath", "readlink", "basename", "dirname", "pwd", "echo"],
  ...["printf", "true", "false", "test", "[", "[[", ":", "which", "whereis", "type", "jq", "yq", "cut", "sort", "uniq"],
  ...["tr", "column", "nl", "od", "xxd", "hexdump", "strings", "md5", "md5sum", "shasum", "sha1sum", "sha256sum"],
  ...["sha512sum", "cksum", "bat", "tac", "rev", "fold", "fmt", "paste", "join", "expand", "unexpand", "base64"],
  ...["date", "sleep", "id", "whoami", "uname", "hostname", "printenv", "locale", "seq", "sed", "gsed", "awk"],
  ...["gawk", "mawk", "nawk", "fd", "fdfind", "set", "shopt", "exit", "return", "wait", "jobs"],
]);
/** Programs that run code given in their words or their input: a clone's path in that code is a write. */
const INTERPRETERS = new Set([
  ...["python", "python3", "node", "ruby", "perl", "php", "bun", "deno", "tsx", "ts-node", "osascript", "Rscript", "lua"],
  ...["awk", "gawk", "mawk", "nawk", "sed", "gsed"],
]);
const SHELLS = new Set(["bash", "sh", "zsh", "dash", "ksh", "fish"]);
/** Programs that run the command after their own options, and those options that take a value. */
const WRAPPERS: Record<string, { values: string[]; dirs?: string[]; positional?: number }> = {
  sudo: { values: ["-u", "-g", "-h", "-p", "-C", "-T", "-U", "-r", "-t", "--user", "--group", "--host", "--prompt"], dirs: ["-D", "--chdir"] },
  doas: { values: ["-u", "-C"] },
  env: { values: ["-u", "--unset", "-S", "--split-string"], dirs: ["-C", "--chdir"] },
  nice: { values: ["-n", "--adjustment"] },
  ionice: { values: ["-c", "-n", "--class", "--classdata"] },
  nohup: { values: [] },
  timeout: { values: ["-s", "--signal", "-k", "--kill-after"], positional: 1 },
  time: { values: ["-f", "--format", "-o", "--output"] },
  stdbuf: { values: ["-i", "-o", "-e", "--input", "--output", "--error"] },
  caffeinate: { values: ["-t", "-w"] },
  command: { values: [] },
  builtin: { values: [] },
  exec: { values: ["-a"] },
  chronic: { values: [] },
};
/** xargs' options that take the next word as their value. */
const XARGS_VALUES = new Set(["-I", "-n", "-L", "-P", "-s", "-d", "-E", "-a", "--arg-file"]);
/** Copiers: what they write is their last operand, or the value of `-t`. Their options taking a value. */
const COPIERS: Record<string, string[]> = {
  cp: ["-S", "--suffix"],
  ln: ["-S", "--suffix"],
  install: ["-S", "--suffix", "-m", "--mode", "-o", "--owner", "-g", "--group"],
  scp: ["-i", "-P", "-o", "-F", "-c", "-l", "-S", "-J", "-D", "-X"],
  rsync: [
    ...["-e", "--rsh", "--exclude", "--include", "--filter", "-f", "--files-from", "--exclude-from", "--include-from"],
    ...["--compare-dest", "--copy-dest", "--link-dest", "--read-batch", "--chmod", "--chown", "--usermap"],
    ...["--groupmap", "--port", "--sockopts", "--out-format", "--log-file-format", "--password-file", "--suffix"],
    ...["--max-size", "--min-size", "-B", "--block-size", "--timeout", "--contimeout", "--modify-window", "--iconv"],
    ...["--checksum-choice", "--compress-choice", "--skip-compress", "--bwlimit", "--info", "--debug", "--max-delete"],
    ...["--rsync-path", "--address", "-M", "--remote-option"],
  ],
};
/** rsync's options naming a file or folder it writes. */
const RSYNC_WRITES = new Set(["--log-file", "-T", "--temp-dir", "--partial-dir", "--backup-dir", "--write-batch", "--only-write-batch"]);

/** Git's options before the subcommand that take the next word as their value. */
const GIT_VALUES = new Set(["-C", "-c", "--git-dir", "--work-tree", "--namespace", "--config-env"]);
/** Git subcommands that only read, whatever their options (an `--output` file is judged apart). */
const GIT_READS = new Set([
  ...["status", "log", "diff", "diff-tree", "diff-index", "diff-files", "show", "show-branch", "blame", "annotate"],
  ...["grep", "ls-files", "ls-tree", "ls-remote", "rev-parse", "rev-list", "describe", "shortlog", "cat-file"],
  ...["for-each-ref", "show-ref", "name-rev", "whatchanged", "merge-base", "cherry", "range-diff", "count-objects"],
  ...["check-ignore", "check-attr", "check-ref-format", "verify-commit", "verify-tag", "var", "help", "version"],
  ...["archive"],
]);
/** `git branch` and `git tag` options that list, and those after which a word is a pattern. */
const LISTING = {
  branch: {
    flags: ["-a", "--all", "-r", "--remotes", "-l", "--list", "-v", "-vv", "--verbose", "-q", "--quiet", "--show-current"],
    patterns: ["-l", "--list"],
  },
  tag: { flags: ["-l", "--list", "-v", "--verify", "-q", "--quiet"], patterns: ["-l", "--list", "-v", "--verify"] },
};
const LISTING_FLAGS = ["--color", "--no-color", "--column", "--no-column", "-i", "--ignore-case", "--omit-empty", "--abbrev", "--no-abbrev"];
const LISTING_VALUES = ["--contains", "--no-contains", "--merged", "--no-merged", "--points-at", "--sort", "--format"];
/** `git config` options that read, and those taking a value. */
const CONFIG_READS = [
  ...["--global", "--system", "--local", "--worktree", "--show-origin", "--show-scope", "--null", "-z", "--name-only"],
  ...["--bool", "--int", "--bool-or-int", "--path", "--expiry-date", "--includes", "--no-includes"],
];
const CONFIG_GETS = ["--get", "--get-all", "--get-regexp", "--get-urlmatch", "--get-color", "--get-colorbool", "--list", "-l"];
const CONFIG_VALUES = ["--file", "-f", "--blob", "--type", "--default"];
/** Git's options naming a file or folder it writes, whatever the subcommand. */
const GIT_OUTPUTS = new Set(["--output", "--output-directory", "--separate-git-dir"]);
/** `git clone` and `git init` options that take the next word as their value. */
const CLONE_VALUES = new Set([
  ...["-o", "--origin", "-b", "--branch", "-u", "--upload-pack", "--reference", "--reference-if-able", "--depth"],
  ...["--shallow-since", "--shallow-exclude", "-c", "--config", "--server-option", "-j", "--jobs", "--template"],
  ...["--filter", "--bundle-uri", "--separate-git-dir", "--initial-branch", "--object-format", "--ref-format"],
]);

/** What a line does to the clones, command by command. */
class Judge {
  readonly verdict: {
    /** A clone a command changes, or may change. */
    changed?: GuardedClone;
    /** A clone the line names, if only to read it. */
    named?: GuardedClone;
    /** A command that may change files works with a value only known when the line runs. */
    blind: boolean;
  } = { blind: false };

  /**
   * Variables `read` and `for` set as the line runs, unknown on the whole line: the loop that uses
   * them may run in a pipeline's subshell (`ls | while read f; do rm "$f"; done`).
   */
  private readonly looped = new Set<string>();

  constructor(private readonly clones: GuardedClone[]) {}

  list(list: List, state: State): void {
    let before = "";
    let piped: string[] = [];
    for (const { node, then } of list) {
      // A command of a pipeline, or one sent to the background, runs in a subshell: its `cd` stays there.
      const apart = then === "|" || then === "|&" || then === "&" || before === "|" || before === "|&";
      this.node(node, apart ? copy(state) : state, piped);
      // `cat <<'EOF' | bash`: a here-document piped on is the next command's input.
      piped = (then === "|" || then === "|&") && "documents" in node ? node.documents : [];
      before = then;
    }
  }

  private node(node: Command | Group, state: State, piped: string[]): void {
    if ("group" in node) {
      this.redirections(node, state);
      this.list(node.group, node.subshell ? copy(state) : state);
      return;
    }
    for (const here of node.here) this.list(here, copy(state));
    this.redirections(node, state);
    const values = node.words.map((word) => this.value(word, state));
    let i = 0;
    while (i < node.words.length && isAssignment(node.words[i])) i++;
    // Assignments alone set the line's variables; before a command, only its environment.
    if (i === values.length) {
      for (const value of values) this.assign(value, state);
      return;
    }
    // `GIT_DIR=… git …`: the variable is set for that command only.
    const env = values.slice(0, i).map((value) => this.envClone(value)).find(Boolean);
    const local = env && !state.env ? { ...copy(state), env } : state;
    const input = [
      ...piped,
      ...node.documents,
      ...node.redirections.filter((redirection) => redirection.input).map(({ target }) => this.value(target, state).text),
    ];
    this.run(values.slice(i), local, false, input);
  }

  /** The clone an assignment's value names: a variable exported with it points a program there. */
  private envClone(value: Value): GuardedClone | undefined {
    const eq = value.text.indexOf("=");
    if (eq < 0) return undefined;
    const assigned = value.text.slice(eq + 1);
    return isAbsolute(assigned) ? cloneAround(resolve(assigned), this.clones) : undefined;
  }

  private redirections({ redirections }: Command | Group, state: State): void {
    for (const { writes, target } of redirections) {
      const value = this.value(target, state);
      if (writes) this.writes(value, state, false);
      else this.reads([value], state);
    }
  }

  /** A word's value; the commands in it are judged as they run, before the command they are in. */
  private value(word: Word, state: State): Value {
    let text = "";
    let prefix: string | undefined;
    let known = true;
    let glob = false;
    for (const part of word.parts) {
      if ("text" in part) {
        if (part.glob) {
          glob = true;
          prefix ??= text;
        }
        text += part.text;
        continue;
      }
      const set = "variable" in part ? this.variable(part.variable, state) : undefined;
      if (set !== undefined) {
        text += set;
        continue;
      }
      if ("variable" in part && part.fallback) {
        const fallback = this.value({ parts: part.fallback }, state);
        if (isExact(fallback)) {
          text += fallback.text;
          continue;
        }
      }
      if ("command" in part) this.list(part.command, copy(state));
      // An array's words are read; the array's value is not known.
      if ("array" in part) this.reads(part.array.map((word) => this.value(word, state)), state);
      known = false;
      prefix ??= text;
    }
    return { text, known, glob, prefix: prefix ?? text };
  }

  private variable(name: string, state: State): string | undefined {
    if (name === "HOME") return homedir();
    if (name === "PWD") return state.dir;
    if (name === "OLDPWD") return state.previous;
    return this.looped.has(name) ? undefined : state.vars.get(name);
  }

  private assign(value: Value, state: State): void {
    const name = /^([A-Za-z_][A-Za-z0-9_]*)(\+?)=/.exec(value.text);
    if (!name?.[1]) return;
    state.vars.set(name[1], isExact(value) && !name[2] ? value.text.slice(name[0].length) : undefined);
  }

  /**
   * A command, its words valued: the program first. `fed`: more arguments come from its input
   * (`xargs`). `input`: its here-documents' and here-strings' text.
   */
  private run(words: Value[], state: State, fed: boolean, input: string[] = []): void {
    let start = 0;
    while (words[start]?.known && RESERVED.has(words[start]?.text ?? "")) start++;
    const program = words[start];
    if (!program) return;
    const args = words.slice(start + 1);
    if (!program.known) {
      this.opaque(words.slice(start), state, true, input);
      return;
    }
    const name = basename(program.text);
    const texts = args.map((arg) => arg.text);
    if (name === "for" || name === "select") {
      if (texts[0]) this.looped.add(texts[0]);
      this.reads(args.slice(texts.indexOf("in") + 1 || args.length), state);
    } else if (name === "cd" || name === "pushd" || name === "popd") this.changeDir(name, args, state);
    else if (["export", "local", "declare", "typeset", "readonly"].includes(name)) {
      for (const arg of args) this.assign(arg, state);
      if (name === "export" || texts.some((text) => /^-[a-zA-Z]*x/.test(text))) this.exported(args, state);
    } else if (["read", "mapfile", "readarray"].includes(name)) {
      for (const text of texts) if (!text.startsWith("-")) this.looped.add(text);
    } else if (name === "unset") for (const text of texts) state.vars.delete(text);
    else if (name === "case") return;
    else if (name === "eval" || name === "trap") this.script(name === "eval" ? args : args.slice(0, 1), state);
    else if (SHELLS.has(name)) this.shell([program, ...args], state, fed, input);
    else if (name === "source" || name === ".") this.source(args, state, fed);
    else if (name === "parallel") this.parallel(args, state);
    else if (name === "command" && (texts[0] === "-v" || texts[0] === "-V")) return;
    else if (WRAPPERS[name]) this.wrapper(name, args, state, fed, input);
    else if (name === "watch") this.watch(args, state);
    else if (name === "xargs") this.xargs(args, state);
    else if (name === "find") this.find(args, state, fed);
    else if (name === "git") this.git(args, state);
    else if (READERS.has(name) && onlyReads(name, texts)) {
      this.reads(args, state);
    }
    else {
      const targets = writeTargets(name, args);
      if (!targets) {
        this.opaque([program, ...args], state, fed, input);
        return;
      }
      for (const target of targets.writes) this.writes(target, state, true);
      this.reads(targets.reads, state);
      if (fed) this.verdict.blind = true;
    }
  }

  /** `export NAME=…` or `export NAME`: a clone it names is where a program may now write. */
  private exported(args: Value[], state: State): void {
    for (const arg of args) {
      const set = /^[A-Za-z_][A-Za-z0-9_]*$/.test(arg.text) ? state.vars.get(arg.text) : undefined;
      state.env ??= this.envClone(set === undefined ? arg : exact(`=${set}`));
    }
  }

  /** `source file`: a `.env` file sets variables; any other file runs commands unseen. */
  private source(args: Value[], state: State, fed: boolean): void {
    const file = args[0];
    if (file && isExact(file) && /(^|\/)\.env(\.[\w.-]+)?$|\.env$/.test(file.text)) this.reads([file], state);
    else this.opaque([exact("source"), ...args], state, fed);
  }

  /** `parallel` runs its command with words from its input or after `:::`. */
  private parallel(args: Value[], state: State): void {
    let i = 0;
    while (args[i]?.known && args[i]?.text.startsWith("-")) i++;
    const sources = args.findIndex((arg) => arg.text === ":::" || arg.text === "::::");
    const command = args.slice(i, sources < 0 ? args.length : sources);
    if (sources >= 0) this.reads(args.slice(sources + 1), state);
    this.run(command.length > 0 ? command : [exact("echo")], copy(state), true);
  }

  /** `eval` and `trap`: their words are a command line. */
  private script(words: Value[], state: State): void {
    if (words.every(isExact)) this.list(readLine(words.map((word) => word.text).join(" ")), state);
    else this.verdict.blind = true;
  }

  private changeDir(name: string, args: Value[], state: State): void {
    if (name === "popd") {
      if (state.stack.length === 0) return;
      state.previous = state.dir;
      state.dir = state.stack.pop();
      return;
    }
    const target = args.find((arg) => !arg.known || arg.text === "-" || !arg.text.startsWith("-"));
    let next: string | undefined;
    if (!target) next = name === "cd" ? homedir() : undefined;
    else if (isExact(target) && target.text === "-") next = state.previous;
    else if (isExact(target)) next = this.place(target.text, state.dir);
    else this.reads([target], state);
    if (name === "pushd") state.stack.push(state.dir);
    state.previous = state.dir;
    state.dir = next;
  }

  /** `bash -c '…' args` runs its script with `$0`, `$1`… set; `bash file` a script unseen; `bash` alone its input. */
  private shell(words: Value[], state: State, fed: boolean, input: string[]): void {
    let script = false;
    let i = 1;
    for (; i < words.length; i++) {
      const word = words[i];
      if (!word?.known || !/^[-+]/.test(word.text)) break;
      if (/^-[a-zA-Z]*c[a-zA-Z]*$/.test(word.text)) script = true;
      else if (["-o", "+o", "-O", "+O", "--rcfile", "--init-file"].includes(word.text)) i++;
    }
    const operand = words[i];
    if (script) {
      if (!operand) return;
      const local = copy(state);
      const positional = words.slice(i + 1);
      this.reads(positional, state);
      positional.forEach((arg, n) => local.vars.set(String(n), isExact(arg) ? arg.text : undefined));
      this.script([operand], local);
    } else if (operand) this.opaque(words, state, fed, input);
    else if (input.length > 0) for (const text of input) this.list(readLine(text), copy(state));
    else this.verdict.blind = true;
  }

  private wrapper(name: string, args: Value[], state: State, fed: boolean, input: string[]): void {
    const spec = WRAPPERS[name];
    if (!spec) return;
    const local = copy(state);
    let i = 0;
    while (i < args.length) {
      const arg = args[i];
      if (!arg?.known) break;
      if (arg.text === "--") {
        i++;
        break;
      }
      if (name === "env" && /^[A-Za-z_][A-Za-z0-9_]*=/.test(arg.text)) {
        local.env ??= this.envClone(arg);
        i++;
        continue;
      }
      if (!arg.text.startsWith("-") || arg.text === "-") break;
      let [opt, attached] = option(arg.text);
      // `-S'…'`, `-Cdir`: a short option with its value attached.
      const short = opt.slice(0, 2);
      if (!opt.startsWith("--") && opt.length > 2 && (spec.values.includes(short) || (spec.dirs ?? []).includes(short))) {
        attached = opt.slice(2);
        opt = short;
      }
      const takes = spec.values.includes(opt) || (spec.dirs ?? []).includes(opt);
      const value = attached !== undefined ? after(arg, arg.text.length - attached.length) : takes ? args[i + 1] : undefined;
      i += attached === undefined && takes ? 2 : 1;
      if (!value) continue;
      if ((spec.dirs ?? []).includes(opt)) {
        local.previous = local.dir;
        local.dir = isExact(value) ? this.place(value.text, local.dir) : undefined;
      } else if (name === "time" && (opt === "-o" || opt === "--output")) this.writes(value, state, false);
      else if (name === "env" && (opt === "-S" || opt === "--split-string")) {
        this.script([value, ...args.slice(i)], local);
        return;
      }
    }
    i += spec.positional ?? 0;
    const inner = args.slice(i);
    if (inner.length > 0) this.run(inner, local, fed, input);
  }

  /** `watch` runs its words as a command line, through `sh -c` unless `-x`. */
  private watch(args: Value[], state: State): void {
    let i = 0;
    let direct = false;
    for (; args[i]?.known && args[i]?.text.startsWith("-"); i++) {
      const text = args[i]?.text;
      if (text === "-n" || text === "--interval") i++;
      if (text === "-x" || text === "--exec") direct = true;
    }
    if (direct) this.run(args.slice(i), copy(state), false);
    else this.script(args.slice(i), copy(state));
  }

  /** `xargs` runs its command with words from its input: at the end, or where `-I`'s string is. */
  private xargs(args: Value[], state: State): void {
    let replace: string | undefined;
    let i = 0;
    for (; i < args.length; i++) {
      const arg = args[i];
      if (!arg?.known || !arg.text.startsWith("-") || arg.text === "-") break;
      if (arg.text === "--") {
        i++;
        break;
      }
      const [opt, attached] = option(arg.text);
      if (opt === "-I") replace = args[i + 1]?.text;
      else if (/^-I./.test(opt)) replace = opt.slice(2);
      else if (opt === "-i" || opt === "--replace") replace = attached ?? "{}";
      else if (/^-i./.test(opt)) replace = opt.slice(2);
      const input = opt === "-a" || opt === "--arg-file" ? (attached !== undefined ? after(arg, opt.length + 1) : args[i + 1]) : undefined;
      if (input) this.reads([input], state);
      if (attached === undefined && XARGS_VALUES.has(opt)) i++;
    }
    const inner = args.slice(i);
    const words = inner.length > 0 ? inner : [exact("echo")];
    const fed = replace === undefined;
    const placed = fed
      ? words
      : words.map((word) => (replace && word.text.includes(replace) ? { ...word, known: false, prefix: word.text.slice(0, word.text.indexOf(replace)) } : word));
    this.run(placed, copy(state), fed);
  }

  /** `find` reads, unless it deletes, prints into a file, or runs a command on what it finds. */
  private find(args: Value[], state: State, fed: boolean): void {
    let i = 0;
    for (; args[i]?.known && /^-(?:[HLP]|O\d*|D)$/.test(args[i]?.text ?? ""); i++) if (args[i]?.text === "-D") i++;
    const roots: Value[] = [];
    for (; i < args.length; i++) {
      const arg = args[i];
      if (!arg || (arg.known && /^[-(!),]/.test(arg.text))) break;
      roots.push(arg);
    }
    if (roots.length === 0) roots.push(exact("."));
    this.reads(roots, state);
    for (; i < args.length; i++) {
      const text = args[i]?.text ?? "";
      if (text === "-delete") for (const root of roots) this.writes(root, state, true);
      else if (["-fprint", "-fprint0", "-fls", "-fprintf"].includes(text)) {
        const file = args[++i];
        if (file) this.writes(file, state, false);
      } else if (["-exec", "-execdir", "-ok", "-okdir"].includes(text)) {
        let end = i + 1;
        while (end < args.length && !(args[end]?.known && (args[end]?.text === ";" || args[end]?.text === "+"))) end++;
        const command = args.slice(i + 1, end);
        for (const root of roots) {
          const local = copy(state);
          if (text.endsWith("dir")) local.dir = isExact(root) ? this.place(root.text, state.dir) : undefined;
          this.run(
            command.map((word) => (word.text.includes("{}") ? found(word, root) : word)),
            local,
            fed,
          );
        }
        i = end;
      }
    }
  }

  /** Git changes the repository it runs on, unless it reads; whatever it reads, its outputs. */
  private git(args: Value[], state: State): void {
    let where = state.dir;
    const given: (string | undefined)[] = [];
    let i = 0;
    for (; i < args.length; i++) {
      const arg = args[i];
      if (!arg?.known || !arg.text.startsWith("-")) break;
      const [opt, attached] = option(arg.text);
      const value = attached !== undefined ? after(arg, opt.length + 1) : GIT_VALUES.has(opt) ? args[++i] : undefined;
      if (!value) continue;
      const place = isExact(value) ? this.place(value.text, where) : undefined;
      if (opt === "-C") where = place;
      else if (opt === "--work-tree" || opt === "--git-dir") given.push(place);
    }
    const sub = args[i];
    const rest = args.slice(i + 1);
    if (!sub) return;
    const local = { ...state, dir: where };
    for (const out of gitOutputs(sub.text, rest)) this.writes(out, local, false);
    const texts = rest.map((arg) => arg.text);
    const file = sub.text === "config" ? optionValue(rest, ["-f", "--file"]) : undefined;
    // The repository it works on: `--work-tree` and `--git-dir` both given, where it runs no longer counts.
    const repos = file ? [isExact(file) ? this.place(file.text, where) : undefined] : given.length === 2 ? given : [where, ...given];
    let clone = repos.map((repo) => (repo === undefined ? undefined : cloneOf(repo, this.clones))).find((found) => found !== undefined);
    // The folder `clone` and `init` make, and paths outside the repository it names.
    if (sub.text === "clone" || sub.text === "init") {
      const operands = gitOperands(texts);
      const folder = sub.text === "clone" ? operands[1] : operands[0];
      const target = folder === undefined ? where : this.place(folder, where);
      clone = target === undefined ? undefined : cloneOf(target, this.clones);
      if (target === undefined) this.verdict.blind = true;
    }
    // `GIT_DIR` and the like, set for it, point it at a clone.
    clone ??= sub.text === "clone" || sub.text === "init" ? undefined : state.env;
    if (!sub.known) this.verdict.blind = true;
    else if (sub.text === "submodule" && texts.includes("foreach")) {
      const script = rest.slice(texts.indexOf("foreach") + 1).find((arg) => !arg.text.startsWith("-"));
      if (script) this.script([script], local);
      return;
    } else if (sub.text !== "clone" && sub.text !== "init" && gitReads(sub.text, texts)) {
      this.verdict.named ??= clone;
      return;
    }
    // Paths it is given only name what it reads or works on in the repository: only where it
    // runs, its outputs and the folder `clone` or `init` makes are written.
    if (clone) this.verdict.changed ??= clone;
    else if (repos.some((repo) => repo === undefined)) this.verdict.blind = true;
  }

  /** A path the command writes: in a clone, or for a folder (`around`), holding one. */
  private writes(value: Value, state: State, around: boolean): void {
    const paths = this.paths(value, state);
    for (const path of paths) this.verdict.changed ??= around ? cloneAround(path, this.clones) : cloneOf(path, this.clones);
    // A value known only in part, from a folder outside the clones (`web/$f`), stays there.
    if (paths.length === 0 && (!value.known || value.text !== "")) this.verdict.blind = true;
  }

  /** Paths a command only reads, and the folder it runs in. */
  private reads(values: Value[], state: State): void {
    if (state.dir !== undefined) this.verdict.named ??= cloneOf(state.dir, this.clones);
    for (const value of values) for (const path of this.paths(value, state)) this.verdict.named ??= cloneOf(path, this.clones);
  }

  /**
   * A program the guard does not know: it changes what it names, the folder it runs in, and where
   * an environment variable set for it points. An interpreter also changes a clone its code names.
   */
  private opaque(words: Value[], state: State, fed: boolean, input: string[] = []): void {
    if (state.dir === undefined) this.verdict.blind = true;
    else this.verdict.changed ??= cloneOf(state.dir, this.clones);
    this.verdict.changed ??= state.env;
    const interpreter = INTERPRETERS.has(basename(words[0]?.text ?? ""));
    for (const [n, word] of words.entries()) {
      // The program is found on the PATH unless written as a path (`./run.sh`, `/code/web/bin/x`).
      const paths = n === 0 && !word.text.includes("/") ? [] : this.paths(word, state);
      for (const path of paths) this.verdict.changed ??= cloneAround(path, this.clones);
      if (interpreter) this.verdict.changed ??= this.inText(word.text);
      if (paths.length === 0 && (!word.known || this.unplaced(word, state))) this.verdict.blind = true;
    }
    if (interpreter) for (const text of input) this.verdict.changed ??= this.inText(text);
    if (fed) this.verdict.blind = true;
  }

  /** The paths a word may name: as given, after `=` or `:`, an option's attached path, a wildcard's folder. */
  private paths(value: Value, state: State): string[] {
    const braces = value.known ? /^([^{}]*)\{([^{}]*,[^{}]*)\}(.*)$/.exec(value.text) : null;
    if (braces) {
      // `{a,b}`: each of its words.
      return (braces[2] ?? "").split(",").flatMap((choice) => {
        const text = `${braces[1] ?? ""}${choice}${braces[3] ?? ""}`;
        const wild = /[*?[{]/.exec(text);
        return this.paths(wild ? { text, known: true, glob: true, prefix: text.slice(0, wild.index) } : exact(text), state);
      });
    }
    let texts: string[];
    if (isExact(value)) texts = spellings(value.text);
    else if (value.known || value.prefix) texts = [folderOf(value.prefix)];
    else texts = [];
    return texts.flatMap((text) => {
      const place = this.place(text, state.dir);
      return place === undefined ? [] : [place];
    });
  }

  /** A relative path from a folder unknown. */
  private unplaced(value: Value, state: State): boolean {
    return state.dir === undefined && value.text !== "" && !isAbsolute(value.text);
  }

  private place(path: string, dir: string | undefined): string | undefined {
    if (isAbsolute(path)) return resolve(path);
    return dir === undefined ? undefined : resolve(dir, path);
  }

  /** A clone's path written inside a word (`python -c "open('/code/web/x', 'w')"`). */
  private inText(written: string): GuardedClone | undefined {
    // `\/code\/web` in a sed or a regex is the path too.
    const text = `${written} ${written.replace(/\\(.)/g, "$1")}`;
    for (const { clone } of this.clones) {
      for (let from = text.indexOf(clone); from >= 0; from = text.indexOf(clone, from + 1)) {
        const path = /^[^\s'"`;,()<>|&=:]*/.exec(text.slice(from))?.[0] ?? "";
        if (path !== clone && !path.startsWith(`${clone}/`)) continue;
        const found = cloneOf(resolve(path), this.clones);
        if (found) return found;
      }
    }
    return undefined;
  }
}

function isAssignment(word: Word | undefined): boolean {
  const first = word?.parts[0];
  return first !== undefined && "text" in first && /^[A-Za-z_][A-Za-z0-9_]*\+?=/.test(first.text);
}

/** The ways a word without a wildcard may name a path. */
function spellings(text: string): string[] {
  if (!text || text === "-" || text === "--") return [];
  if (/\s/.test(text)) return [text];
  const all = [text];
  const eq = text.indexOf("=");
  if (eq > 0) all.push(text.slice(eq + 1));
  if (text.includes(":")) all.push(...text.split(":"));
  if (/^-[A-Za-z][~/.]/.test(text)) all.push(text.slice(2));
  return all.filter((spelling) => spelling !== "");
}

/** The folder a wildcard's matches are in: `/a/b*` is in `/a/`, `*.log` in `.`. */
function folderOf(prefix: string): string {
  const slash = prefix.lastIndexOf("/");
  return slash < 0 ? "." : prefix.slice(0, slash + 1);
}

/** `find -exec`'s `{}`, as each starting point. */
function found(word: Value, root: Value): Value {
  const text = word.text.replaceAll("{}", root.text);
  if (isExact(root)) return exact(text);
  return { text, known: root.known, glob: root.glob, prefix: word.text.startsWith("{}") ? root.prefix : "" };
}

/** A reader that, with these options, writes after all. */
function onlyReads(name: string, texts: string[]): boolean {
  switch (name) {
    case "sed":
    case "gsed":
      // `-i` edits in place; a script's `w file` or `s/…/…/w file` writes that file, and its `e`
      // command or `s/…/…/e` runs a command.
      return (
        !texts.some((text) => /^-[a-zA-Z]*i/.test(text) || text.startsWith("--in-place")) &&
        !sedScripts(texts).some((script) => /(^|[;}\s/])w\s+\S/.test(script) || /(^|[;}\s\d$/])e(\s|$)/.test(script) || SED_E_FLAG.test(script))
      );
    case "awk":
    case "gawk":
    case "mawk":
    case "nawk": {
      let program: string | undefined;
      for (let i = 0; i < texts.length; i++) {
        const text = texts[i] ?? "";
        if (text === "-f" || text.startsWith("--file") || text.startsWith("-i") || text.startsWith("--include")) return false;
        if (text === "-v" || text === "-F") i++;
        else if (!text.startsWith("-")) {
          program = text;
          break;
        }
      }
      // `print > "file"`, `print | "cmd"`, `system()`, `"cmd" | getline`: a comparison's `>` is no write.
      return program === undefined || !/\bprintf?\b[^;}]*[>|]|system\s*\(|\|\s*getline/.test(program);
    }
    case "sort":
      return !texts.some((text) => /^-[a-zA-Z]*o/.test(text) || text.startsWith("--output"));
    case "uniq":
      return texts.filter((text) => !text.startsWith("-")).length <= 1;
    case "yq":
      return !texts.some((text) => text === "-i" || text.startsWith("--inplace"));
    case "rg":
      return !texts.some((text) => text.startsWith("--pre"));
    case "tree":
      return !texts.some((text) => text === "-o" || text.startsWith("--output"));
    case "xxd":
      return !texts.some((text) => /^-[a-zA-Z]*r/.test(text) || text === "-revert");
    case "base64":
      return !texts.some((text) => text === "-o" || text.startsWith("--output"));
    case "fd":
    case "fdfind":
      return !texts.some((text) => ["-x", "-X", "--exec", "--exec-batch"].includes(text));
    default:
      return true;
  }
}

/** `s/…/…/e`: a substitution whose flags hold `e`, any delimiter. */
const SED_E_FLAG = /s(.)(?:(?!\1)[^\\]|\\.)*\1(?:(?!\1)[^\\]|\\.)*\1[a-zA-Z0-9]*e/;

/** A sed command's scripts: `-e`'s values, else its first operand. */
function sedScripts(texts: string[]): string[] {
  const scripts: string[] = [];
  for (let i = 0; i < texts.length; i++) {
    const text = texts[i] ?? "";
    if (text === "-e" || text === "--expression") scripts.push(texts[++i] ?? "");
    else if (text.startsWith("--expression=")) scripts.push(text.slice("--expression=".length));
    else if (text === "-f" || text === "--file" || text === "-l") i++;
    else if (!text.startsWith("-") && scripts.length === 0) return [text];
  }
  return scripts;
}

/** What a writer the guard knows writes, and what it only reads; undefined for other programs. */
function writeTargets(name: string, args: Value[]): { writes: Value[]; reads: Value[] } | undefined {
  const operands = args.filter((arg) => !arg.known || !arg.text.startsWith("-") || arg.text === "-");
  if (name === "tee") return { writes: operands, reads: [] };
  if (name === "uniq") return { writes: operands.slice(1), reads: operands.slice(0, 1) };
  if (name === "sort") {
    const output = optionValue(args, ["-o", "--output"]) ?? args.map((arg) => /^-[a-zA-Z]*o(.+)$/.exec(arg.text)?.[1]).find(Boolean);
    const value = typeof output === "string" ? exact(output) : output;
    return { writes: value ? [value] : [], reads: operands };
  }
  if (name === "tar" || name === "bsdtar" || name === "gtar") return tarred(args);
  if (name === "zip") return { writes: operands.slice(0, 1), reads: operands.slice(1) };
  if (name === "unzip") {
    const into = optionValue(args, ["-d"]);
    return { writes: [into ?? exact(".")], reads: operands.filter((operand) => operand !== into) };
  }
  const values = COPIERS[name];
  return values ? copied(name, args, new Set(values)) : undefined;
}

/** `tar -czf x.tgz`, `-cfx.tar`: `f` in a cluster of short options takes the rest, or the next word. */
function clusterFile(args: Value[]): Value | undefined {
  for (const [i, arg] of args.entries()) {
    const cluster = arg.known && !arg.text.startsWith("--") ? /^-[a-zA-Z]*?f(.*)$/.exec(arg.text) : null;
    if (cluster) return cluster[1] ? after(arg, arg.text.length - cluster[1].length) : args[i + 1];
  }
  return undefined;
}

/** `tar`: creating writes its archive; extracting writes into `-C`'s folder or where it runs; listing reads. */
function tarred(args: Value[]): { writes: Value[]; reads: Value[] } | undefined {
  const first = args[0]?.text ?? "";
  const modes = first.startsWith("-") ? args.map((arg) => /^-([a-zA-Z]+)/.exec(arg.text)?.[1] ?? "").join("") : first;
  const file = optionValue(args, ["-f", "--file"]) ?? clusterFile(args) ?? (!first.startsWith("-") && first.includes("f") ? args[1] : undefined);
  const into = optionValue(args, ["-C", "--directory"]);
  const reads = args.filter((arg) => arg !== file && arg !== into && !arg.text.startsWith("-") && arg !== args[0]);
  if (/[xX]/.test(modes) || args.some((arg) => arg.text === "--extract" || arg.text === "--get")) return { writes: [into ?? exact(".")], reads: [] };
  if (/[t]/.test(modes) || args.some((arg) => arg.text === "--list")) return { writes: [], reads: file ? [file] : [] };
  if (/[cru]/.test(modes) || args.some((arg) => ["--create", "--append", "--update"].includes(arg.text))) {
    const archive = file && file.text !== "-" ? [file] : [];
    return { writes: archive, reads: into ? [into, ...reads] : reads };
  }
  return undefined;
}

/** A copier's destination (`-t`'s value, or its last operand) and the sources it only reads. */
function copied(name: string, args: Value[], values: Set<string>): { writes: Value[]; reads: Value[] } {
  const writes: Value[] = [];
  const sources: Value[] = [];
  let target: Value | undefined;
  let removesSources = false;
  const targetOption = name === "cp" || name === "ln" || name === "install";
  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    if (!arg) continue;
    if (!arg.known || !arg.text.startsWith("-") || arg.text === "-") {
      sources.push(arg);
      continue;
    }
    if (arg.text === "--") {
      sources.push(...args.slice(i + 1));
      break;
    }
    const [opt, attached] = option(arg.text);
    if (opt === "--remove-source-files") removesSources = true;
    const isTarget = targetOption && (opt === "-t" || opt === "--target-directory");
    // `-rt dir`, `-tdir`, `-Dt dir`: `t` in a cluster of short options takes the rest, or the next word.
    const cluster = targetOption && !opt.startsWith("--") ? /^-[a-zA-Z]*?t(.*)$/.exec(opt) : null;
    if (cluster && opt !== "-t") {
      const value = cluster[1] ? after(arg, opt.length - cluster[1].length) : args[++i];
      if (value) target = value;
      continue;
    }
    const takes = isTarget || values.has(opt) || (name === "rsync" && RSYNC_WRITES.has(opt));
    const value = attached !== undefined ? after(arg, opt.length + 1) : takes ? args[++i] : undefined;
    if (!value) continue;
    if (isTarget) target = value;
    else if (name === "rsync" && RSYNC_WRITES.has(opt)) writes.push(value);
  }
  // `ln -s /a/b`: the link is made in the folder the command runs in.
  target ??= name === "ln" && sources.length === 1 ? exact(".") : sources.pop();
  if (target) writes.push(target);
  return removesSources ? { writes: [...writes, ...sources], reads: [] } : { writes, reads: sources };
}

/** The value of the first of `names` among the words: `-f x`, `--file x` or `--file=x`. */
function optionValue(args: Value[], names: string[]): Value | undefined {
  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    if (!arg) continue;
    const [opt, attached] = option(arg.text);
    if (!names.includes(opt)) continue;
    return attached !== undefined ? after(arg, opt.length + 1) : args[i + 1];
  }
  return undefined;
}

/** The files and folders a git command writes by an option: `--output`, `format-patch -o`… */
function gitOutputs(sub: string, rest: Value[]): Value[] {
  const outputs: Value[] = [];
  const words = rest.filter((arg) => !arg.text.startsWith("-"));
  if (sub === "bundle" && words[0]?.text === "create" && words[1]) outputs.push(words[1]);
  for (let i = 0; i < rest.length; i++) {
    const arg = rest[i];
    if (!arg?.known) continue;
    const [opt, attached] = option(arg.text);
    const isOutput = GIT_OUTPUTS.has(opt) || ((sub === "format-patch" || sub === "archive") && opt === "-o");
    if (!isOutput && (sub === "format-patch" || sub === "archive") && /^-o./.test(opt)) outputs.push(after(arg, 2));
    if (!isOutput) continue;
    const value = attached !== undefined ? after(arg, opt.length + 1) : rest[++i];
    if (value) outputs.push(value);
  }
  return outputs;
}

/** `git clone` and `git init` operands, their options' values left out. */
function gitOperands(texts: string[]): string[] {
  const operands: string[] = [];
  for (let i = 0; i < texts.length; i++) {
    const text = texts[i] ?? "";
    if (!text.startsWith("-")) operands.push(text);
    else if (CLONE_VALUES.has(text)) i++;
  }
  return operands;
}

/** A git subcommand that only reads, with these words. */
function gitReads(sub: string, texts: string[]): boolean {
  // `git grep -O<cmd>` opens what it finds with a command.
  if (sub === "grep" && texts.some((text) => text.startsWith("-O") || text.startsWith("--open-files-in-pager"))) return false;
  if (GIT_READS.has(sub)) return true;
  const words = texts.filter((text) => !text.startsWith("-"));
  switch (sub) {
    case "branch":
    case "tag":
      return listing(texts, LISTING[sub]);
    case "remote":
      return words.length === 0 ? texts.every((text) => text === "-v" || text === "--verbose") : words[0] === "get-url" || words[0] === "show";
    case "stash":
      return words[0] === "list" || words[0] === "show";
    case "worktree":
      return words[0] === "list";
    case "reflog":
      return !["expire", "delete", "drop", "write"].includes(words[0] ?? "");
    case "notes":
      return words[0] === "list" || words[0] === "show";
    case "submodule":
      return words[0] === undefined || words[0] === "status" || words[0] === "summary";
    case "config":
      return configReads(texts);
    case "symbolic-ref":
      return words.length <= 1 && !texts.some((text) => text === "-d" || text === "--delete");
    case "fsck":
      return !texts.includes("--lost-found");
    case "lfs":
      return ["ls-files", "status", "env", "version", "logs"].includes(words[0] ?? "");
    case "bundle":
      return ["create", "verify", "list-heads"].includes(words[0] ?? "");
    case "format-patch":
      // Its patches go into the folder it runs in, unless printed or sent elsewhere.
      return texts.some((text) => text === "--stdout" || text === "-o" || /^-o./.test(text) || text.startsWith("--output-directory"));
    default:
      return false;
  }
}

/** `git branch` or `git tag` listing: listing options only, and a pattern only after `--list`. */
function listing(texts: string[], { flags, patterns }: { flags: string[]; patterns: string[] }): boolean {
  let pattern = false;
  let operands = 0;
  for (let i = 0; i < texts.length; i++) {
    const [opt, attached] = option(texts[i] ?? "");
    if (!opt.startsWith("-")) operands++;
    else if (LISTING_VALUES.includes(opt)) {
      if (attached === undefined) i++;
    } else if (flags.includes(opt) || LISTING_FLAGS.includes(opt) || /^-n\d*$/.test(opt)) pattern ||= patterns.includes(opt);
    else return false;
  }
  return operands === 0 || pattern;
}

/** `git config` reading: read options only, and one key or a `--get`/`--list`. */
function configReads(texts: string[]): boolean {
  let keys = 0;
  let gets = false;
  for (let i = 0; i < texts.length; i++) {
    const [opt, attached] = option(texts[i] ?? "");
    if (!opt.startsWith("-")) {
      if (keys === 0 && !gets && ["set", "unset", "rename-section", "remove-section", "edit"].includes(opt)) return false;
      if (keys === 0 && !gets && (opt === "get" || opt === "list")) gets = true;
      else keys++;
    } else if (CONFIG_VALUES.includes(opt)) {
      if (attached === undefined) i++;
    } else if (CONFIG_GETS.includes(opt)) gets = true;
    else if (!CONFIG_READS.includes(opt)) return false;
  }
  return gets || keys === 1;
}

/** The clone a path is in, or the clone in the folder it names. */
function cloneAround(path: string, clones: GuardedClone[]): GuardedClone | undefined {
  return cloneOf(path, clones) ?? clones.find(({ clone }) => inside(clone, path));
}

/** The clone a path is in, unless it is in a worktree of the workspace or its git folder. */
function cloneOf(path: string, clones: GuardedClone[]): GuardedClone | undefined {
  return exempt(path, clones) ? undefined : clones.find(({ clone }) => inside(path, clone));
}

/** A path in a worktree of the workspace, or in its git folder in the clone's `.git`. */
function exempt(path: string, clones: GuardedClone[]): boolean {
  return clones.some(({ worktree, gitdir }) => inside(path, worktree) || (gitdir !== undefined && inside(path, gitdir)));
}

/** macOS's file system ignores case: `~/Documents/GitHub/web` is `~/Documents/github/web`. */
const FOLD_CASE = process.platform === "darwin";

function inside(path: string, dir: string): boolean {
  const rel = FOLD_CASE ? relative(dir.toLowerCase(), path.toLowerCase()) : relative(dir, path);
  return rel === "" || (!rel.startsWith("..") && !isAbsolute(rel));
}

/** A path as a shell or a tool reads it from `dir`: `~`, `$HOME` and `${HOME}` are the home folder. */
function at(dir: string, path: string): string {
  const home = /^(?:~|\$HOME|\$\{HOME\})(?=\/|$)/.exec(path);
  if (home) return join(homedir(), path.slice(home[0].length));
  return resolve(dir, path);
}
