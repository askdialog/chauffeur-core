// Detects shell commands that create a git worktree, so agents go through Chauffeur instead (it
// tracks and cleans every worktree). Agent-agnostic: Claude's PreToolUse hook uses it today, the
// Codex equivalent will use it later.
//
// This is a guard rail, not a sandbox: it reads the command the way a shell would (quotes,
// operators, env prefixes, wrappers, `sh -c`), well enough to catch what an agent writes.

export const WORKTREE_ADD_DENIAL =
  "`git worktree add` is disabled in Chauffeur workspaces: Chauffeur tracks and cleans every worktree. " +
  "Run `chauffeur worktree add --repo <repo> [--name <name>]` instead (it prints the new worktree's path), " +
  "and `chauffeur worktree remove` to remove one.";

export const ADD_REPO_DENIAL =
  "An agent does not run `chauffeur add-repo`: propose the repo with the add_repo tool (list_repos lists the " +
  "organization's), and the user answers Add or Not now. Without that tool, ask the user to add the repo from Chauffeur.";

/** True when the shell command runs `chauffeur add-repo` anywhere in it: the agent proposes a repo with its tool. */
export function runsChauffeurAddRepo(command: string): boolean {
  return simpleCommands(command).some(
    ({ program, args }) => program === "chauffeur" && args.find((arg) => !arg.startsWith("-")) === "add-repo",
  );
}

/** True when the shell command runs `git worktree add` anywhere in it. */
export function runsGitWorktreeAdd(command: string): boolean {
  return simpleCommands(command).some(({ program, args }) => program === "git" && gitArgsAddWorktree(args));
}

/** A program and its arguments, as the shell would run it. */
export type SimpleCommand = { program: string; args: string[] };

/**
 * Every program the command line runs, wrappers and env prefixes skipped, including the scripts
 * of `sh -c '…'`, `eval` and `watch`. `xargs` is reported too, before the program it runs: that
 * program gets words nobody can read here.
 */
export function simpleCommands(command: string, depth = 0): SimpleCommand[] {
  if (depth > 4) return [];
  return splitCommands(command).flatMap((words) => unwrap(words, depth));
}

const SEPARATORS = new Set(["&&", "||", ";", ";;", "|", "|&", "&", "\n", "(", ")", "{", "}"]);

/**
 * Splits a command line into simple commands (arrays of words), honouring quotes and escapes.
 * Operators, subshells, command substitutions and brace groups all start a new command.
 */
export function splitCommands(command: string): string[][] {
  const commands: string[][] = [];
  let words: string[] = [];
  let word = "";
  let inWord = false;
  let inBacktick = false;

  const endWord = () => {
    if (inWord) words.push(word);
    word = "";
    inWord = false;
  };
  const endCommand = () => {
    endWord();
    if (words.length > 0) commands.push(words);
    words = [];
  };

  for (let i = 0; i < command.length; i++) {
    const c = command[i] as string;
    const next = command[i + 1];
    if (c === "\\") {
      if (next === "\n") {
        i++;
        continue;
      }
      if (next !== undefined) {
        word += next;
        inWord = true;
        i++;
      }
      continue;
    }
    if (c === "'") {
      const end = command.indexOf("'", i + 1);
      const stop = end < 0 ? command.length : end;
      word += command.slice(i + 1, stop);
      inWord = true;
      i = stop;
      continue;
    }
    if (c === '"') {
      inWord = true;
      i++;
      while (i < command.length && command[i] !== '"') {
        if (command[i] === "\\" && i + 1 < command.length && '"\\$`\n'.includes(command[i + 1] as string)) {
          if (command[i + 1] !== "\n") word += command[i + 1];
          i += 2;
          continue;
        }
        word += command[i];
        i++;
      }
      continue;
    }
    if (c === "#" && !inWord) {
      const end = command.indexOf("\n", i);
      i = end < 0 ? command.length : end - 1;
      continue;
    }
    if (c === " " || c === "\t" || c === "\r") {
      endWord();
      continue;
    }
    if ((c === "$" && next === "(") || (c === "`" && !inBacktick)) {
      // The substitution's output stays a word of the enclosing command, as `$(...)`: a guard can
      // tell when it is the program that runs.
      word += "$(...)";
      inWord = true;
      endCommand();
      if (c === "$") i++;
      else inBacktick = true;
      continue;
    }
    if (c === "`") inBacktick = false;
    if (c === "&" || c === "|" || c === ";" || c === "\n" || c === "(" || c === ")" || c === "`") {
      // Redirections such as `2>&1` or `&>` are not command separators.
      if (c === "&" && (word.endsWith(">") || word.endsWith("<") || next === ">")) {
        word += c;
        inWord = true;
        continue;
      }
      endCommand();
      if ((c === "&" || c === "|" || c === ";") && next === c) i++;
      else if (c === "|" && next === "&") i++;
      continue;
    }
    if ((c === "{" || c === "}") && !inWord && (next === undefined || /\s|;/.test(next))) {
      endCommand();
      continue;
    }
    word += c;
    inWord = true;
  }
  endCommand();
  return commands.filter((words) => !(words.length === 1 && SEPARATORS.has(words[0] as string)));
}

const KEYWORDS = new Set(["!", "if", "then", "else", "elif", "do", "while", "until", "time", "exec", "command", "builtin", "nohup", "noglob"]);
const SHELLS = new Set(["sh", "bash", "zsh", "dash", "ksh", "fish"]);
/** Wrappers that run the program that follows them, with the options of each that take a value. */
const WRAPPERS: Record<string, Set<string>> = {
  sudo: new Set(["-u", "-g", "-C", "-D", "-h", "-p", "-r", "-t", "-U", "--user", "--group", "--chdir", "--host", "--prompt", "--role", "--type", "--other-user"]),
  doas: new Set(["-u", "-C"]),
  env: new Set(["-u", "--unset", "-C", "--chdir", "-S", "--split-string"]),
  xargs: new Set(["-I", "-i", "-n", "-L", "-P", "-d", "-s", "-E", "-a", "-J", "-R", "-S", "--replace", "--max-args", "--max-lines", "--max-procs", "--delimiter", "--max-chars", "--eof", "--arg-file"]),
  nice: new Set(["-n", "--adjustment"]),
  timeout: new Set(["-s", "-k", "--signal", "--kill-after"]),
  stdbuf: new Set(["-i", "-o", "-e", "--input", "--output", "--error"]),
  ionice: new Set(["-c", "-n", "-p", "--class", "--classdata", "--pid"]),
  setsid: new Set(),
  caffeinate: new Set(["-t", "-w"]),
  chronic: new Set(),
  unbuffer: new Set(),
};
/** Git options that take their value as the next word. */
const GIT_OPTIONS_WITH_VALUE = new Set(["-C", "-c", "--git-dir", "--work-tree", "--namespace", "--exec-path", "--config-env", "--super-prefix", "--list-cmds"]);

function unwrap(words: string[], depth: number): SimpleCommand[] {
  let i = 0;
  // `xargs` is reported as a command of its own as well: its program gets words nobody can read here.
  const wrappers: SimpleCommand[] = [];
  // Leading keywords, wrappers and environment assignments: `FOO=1 sudo -E env BAR=2 git …`.
  for (;;) {
    const word = words[i];
    if (word === undefined) return wrappers;
    if (KEYWORDS.has(word) || /^[A-Za-z_][A-Za-z0-9_]*=/.test(word)) {
      i++;
      continue;
    }
    const withValue = WRAPPERS[word];
    if (withValue !== undefined) {
      if (word === "xargs") wrappers.push({ program: word, args: words.slice(i + 1) });
      i++;
      // Skip the wrapper's own options, their values, and `timeout`'s duration.
      while (words[i] !== undefined) {
        const option = words[i] as string;
        if (withValue.has(option)) i += 2;
        else if (option.startsWith("-") || (word === "timeout" && /^\d/.test(option))) i++;
        else break;
      }
      continue;
    }
    if (word === "watch") {
      // `watch [-n s] <command…>` hands the rest to `sh -c`.
      i++;
      while (words[i] !== undefined && (words[i] as string).startsWith("-")) i += ["-n", "--interval"].includes(words[i] as string) ? 2 : 1;
      return [...wrappers, ...simpleCommands(words.slice(i).join(" "), depth + 1)];
    }
    break;
  }
  const program = basename(words[i] as string);
  const args = words.slice(i + 1);

  if (SHELLS.has(program)) {
    const script = shellScriptArgument(args);
    return [...wrappers, ...(script === undefined ? [{ program, args }] : simpleCommands(script, depth + 1))];
  }
  if (program === "eval") return [...wrappers, ...simpleCommands(args.join(" "), depth + 1)];
  return [...wrappers, { program, args }];
}

/** Git's subcommand and its arguments, global options (`-C dir`…) skipped. */
export function gitSubcommand(args: string[]): { name: string | undefined; args: string[] } {
  let i = 0;
  while (i < args.length) {
    const arg = args[i] as string;
    if (GIT_OPTIONS_WITH_VALUE.has(arg)) {
      i += 2;
      continue;
    }
    if (arg.startsWith("-")) {
      i++;
      continue;
    }
    break;
  }
  return { name: args[i], args: args.slice(i + 1) };
}

function gitArgsAddWorktree(args: string[]): boolean {
  const sub = gitSubcommand(args);
  // `git worktree add …`; tolerate options between `worktree` and its subcommand.
  return sub.name === "worktree" && sub.args.find((arg) => !arg.startsWith("-")) === "add";
}

/** The script of `sh -c '<script>'` (also `-lc`, `-ec`…), when the shell runs one. */
function shellScriptArgument(args: string[]): string | undefined {
  for (let i = 0; i < args.length; i++) {
    const arg = args[i] as string;
    if (/^-[a-zA-Z]*c[a-zA-Z]*$/.test(arg)) return args[i + 1];
    if (!arg.startsWith("-")) return undefined;
  }
  return undefined;
}

function basename(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? path : path.slice(slash + 1);
}
