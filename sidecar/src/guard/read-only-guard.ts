// The shell commands an exploration refuses: those that write git history or refs, or GitHub.
// The checkout itself is disposable, so a stray edit there is harmless; what an exploration must
// not do is leave anything behind in the repos the checkouts share, or on GitHub.
//
// A guard rail rather than a sandbox: Claude's explorations run Bash under Claude Code's OS sandbox
// (writes limited to the worktree) and Codex's in its read-only sandbox. Neither can tell a GitHub
// read from a write over HTTPS, so this parser is the guard for GitHub writes, and the first line,
// with a clear message, for git writes. It reads what an agent writes; what it cannot read (code
// handed to an interpreter, a program named by a variable, words fed by `xargs`) it refuses when
// git or GitHub is mentioned.

import { gitSubcommand, simpleCommands, type SimpleCommand } from "./worktree-guard.ts";

export const READ_ONLY_DENIAL =
  "This Chauffeur workspace is an exploration, read-only: commits, pushes, fetches, branches, tags, stashes, " +
  "worktrees and GitHub writes are refused. Read and explain; `git checkout --detach <ref>` moves the checkout " +
  "without naming a branch. To change code, the user creates a Chauffeur workspace.";

/** The same refusal in a design chat, whose agent designs screens and never touches the repos. */
export const DESIGN_READ_ONLY_DENIAL =
  "This is a Chauffeur design chat: you design screens, you never change code. Commits, pushes, fetches, " +
  "branches, tags, stashes, worktrees and GitHub writes are refused. When the design is ready, the user sends " +
  "it to a coding chat that implements it.";

/** True when the shell command writes git history or refs, or GitHub, anywhere in it. */
export function writesOutsideTheCheckout(command: string): boolean {
  return simpleCommands(command).some((simple) => writes(simple, command));
}

/**
 * Git subcommands refused outright. The rule: denied is anything that creates, moves or deletes
 * refs, writes commits or tags (or any object by hand, with the plumbing) into the shared object
 * store, talks to a remote, or hands out the user's credentials. The index and files of the
 * disposable checkout go with it: `add`, `rm`, `mv`, `restore`, `clean`, `apply`, `read-tree`,
 * `update-index`, `sparse-checkout` stay allowed. Subcommands with read forms are handled below.
 */
const GIT_WRITES = new Set([
  // Commits and refs.
  "commit",
  "merge",
  "rebase",
  "cherry-pick",
  "revert",
  "am",
  "update-ref",
  "filter-branch",
  "filter-repo",
  "subtree",
  "gui",
  "citool",
  // Objects written by hand.
  "commit-tree",
  "mktag",
  "mktree",
  "write-tree",
  "fast-import",
  "unpack-objects",
  "index-pack",
  // Remotes: fetching writes remote-tracking refs the user's clone reads; cloning makes a repo.
  "push",
  "fetch",
  "pull",
  "fetch-pack",
  "send-pack",
  "http-push",
  "http-fetch",
  "send-email",
  "imap-send",
  "clone",
  "init",
  "svn",
  "p4",
  // The shared object store's housekeeping.
  "gc",
  "prune",
  "prune-packed",
  "repack",
  "pack-refs",
  "maintenance",
  "update-server-info",
  // Hooks run the user's scripts; `credential` prints their token.
  "hook",
]);

/** Listing options: a positional with one of these filters a list rather than naming a new ref. */
const REF_LISTING = ["-l", "--list", "--contains", "--no-contains", "--merged", "--no-merged", "--points-at"];
const LFS_WRITES = new Set(["push", "pull", "fetch", "prune", "migrate", "lock", "unlock", "install", "uninstall", "update", "dedup"]);
/** `git -c <key>=…` overrides that hide a subcommand behind an alias or reroute a remote. */
const CONFIG_REROUTES = /^(alias\.|core\.sshcommand=|credential\.|remote\.|url\.)/i;

/** `gh <group> <verb>` that write on GitHub (or create a branch, a repo, a token on this machine). */
const GH_WRITES: Record<string, Set<string>> = {
  pr: new Set(["create", "merge", "close", "reopen", "comment", "edit", "review", "ready", "lock", "unlock", "checkout", "update-branch", "revert"]),
  issue: new Set(["create", "close", "reopen", "comment", "edit", "delete", "transfer", "lock", "unlock", "pin", "unpin", "develop"]),
  repo: new Set(["create", "delete", "edit", "fork", "rename", "archive", "unarchive", "sync", "clone", "set-default"]),
  release: new Set(["create", "delete", "delete-asset", "edit", "upload"]),
  workflow: new Set(["run", "enable", "disable"]),
  run: new Set(["rerun", "cancel", "delete"]),
  label: new Set(["create", "edit", "delete", "clone"]),
  secret: new Set(["set", "delete"]),
  variable: new Set(["set", "delete"]),
  gist: new Set(["create", "edit", "delete", "clone", "rename"]),
  alias: new Set(["set", "delete", "import"]),
  extension: new Set(["install", "upgrade", "remove", "create", "exec"]),
  codespace: new Set(["create", "delete", "edit", "rebuild", "stop", "ssh", "cp", "code", "jupyter"]),
  "ssh-key": new Set(["add", "delete"]),
  "gpg-key": new Set(["add", "delete"]),
  // `auth token` prints the user's token: with it, `curl -X POST` becomes a GitHub write.
  auth: new Set(["login", "logout", "refresh", "setup-git", "switch", "token"]),
  cache: new Set(["delete"]),
  config: new Set(["set", "clear-cache"]),
  "agent-task": new Set(["create"]),
};
/** `gh <group>` whose verbs all write, but these. */
const GH_READS: Record<string, Set<string>> = {
  project: new Set(["list", "view", "field-list", "item-list"]),
  ruleset: new Set(["list", "view", "check"]),
};
/** `gh <group> <verb> <sub>` that write. */
const GH_NESTED_WRITES: Record<string, Set<string>> = {
  "repo deploy-key": new Set(["add", "delete"]),
  "repo autolink": new Set(["create", "delete"]),
  "codespace ports": new Set(["forward", "visibility"]),
};
/** `gh` options whose value is the next word, so it is not taken for a group or a verb. */
const GH_OPTIONS_WITH_VALUE = new Set(["-R", "--repo", "--hostname", "-X", "--method", "-f", "--raw-field", "-F", "--field", "-H", "--header", "--input", "-q", "--jq", "-t", "--template", "--cache", "-p", "--preview"]);

/** Interpreters taking code on the command line, and the option that carries it. */
const INLINE_CODE: Record<string, RegExp> = {
  python: /^-[A-Za-z]*c/,
  python2: /^-[A-Za-z]*c/,
  python3: /^-[A-Za-z]*c/,
  pypy: /^-[A-Za-z]*c/,
  pypy3: /^-[A-Za-z]*c/,
  node: /^(-[A-Za-z]*[ep]|--eval|--print)/,
  bun: /^(-[ep]|--eval|--print)$/,
  deno: /^eval$/,
  perl: /^-[A-Za-z]*[eE]/,
  ruby: /^-[A-Za-z]*e/,
  osascript: /^-e$/,
  php: /^-r$/,
};
const SHELLS = new Set(["sh", "bash", "zsh", "dash", "ksh", "fish"]);
/** Shells and interpreters: what they run is code, read for mentions of git or GitHub. */
const RUNS_CODE = new Set([...SHELLS, ...Object.keys(INLINE_CODE)]);
/** `git` or `gh` as a word of some code, or GitHub's host. */
const MENTIONS_GIT = /(?<![\w.-])(git|gh)(?![\w-])|github\.com/;

const HTTP_CLIENTS = new Set(["curl", "wget", "http", "https", "xh", "xhs"]);
/** A GitHub token in the arguments of an HTTP client: `$GH_TOKEN`, `$(gh auth token)`, a header. */
const TOKEN = /GH_TOKEN|GITHUB_TOKEN|GH_ENTERPRISE_TOKEN|gh auth token|authorization/i;

function writes(simple: SimpleCommand, command: string): boolean {
  const { program, args } = simple;
  // A program named by a variable: nobody can tell what runs.
  if (program.includes("$")) return true;
  if (program === "git") return gitWrites(args);
  if (program === "gh") return ghWrites(args, command);
  // `xargs` and `find -exec` hand words nobody can read here to their program.
  if (program === "xargs") return args.some(isGit);
  if (program === "find") {
    const exec = args.findIndex((a) => ["-exec", "-execdir", "-ok", "-okdir"].includes(a));
    if (exec < 0) return false;
    const run = args.slice(exec + 1);
    return run.some(isGit) || (RUNS_CODE.has(basename(run[0] ?? "")) && MENTIONS_GIT.test(run.join(" ")));
  }
  const inline = INLINE_CODE[program];
  if (inline !== undefined && args.some((a) => inline.test(a))) return MENTIONS_GIT.test(args.join(" "));
  // A shell or interpreter reading its script from stdin (`echo … | bash`, `python3 -`): the script
  // is somewhere in the command line, or nowhere; refused when the line mentions git or GitHub.
  if ((SHELLS.has(program) || inline !== undefined) && readsStdin(program, args)) return MENTIONS_GIT.test(command);
  if (HTTP_CLIENTS.has(program)) return httpWritesGitHub(program, args);
  return false;
}

function isGit(word: string): boolean {
  const name = basename(word);
  return name === "git" || name === "gh";
}

/** No script named and no `-c` code (a shell with `-c` and its script never gets here), or `-`/`-s`. */
function readsStdin(program: string, args: string[]): boolean {
  const script = positionalsOf(args).filter((a) => !/^[\d&]*[<>]/.test(a));
  if (script[0] === "-") return true;
  if (!SHELLS.has(program)) return script.length === 0;
  return !args.some((a) => /^-[a-zA-Z]*c/.test(a)) && (script.length === 0 || flagsOf(args).has("-s"));
}

function basename(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

function gitWrites(all: string[]): boolean {
  if (gitConfigOverrides(all).some((key) => CONFIG_REROUTES.test(key))) return true;
  const { name, args } = gitSubcommand(all);
  if (name === undefined) return false;
  if (name.includes("$") || name === "{}") return true;
  if (GIT_WRITES.has(name) || name.startsWith("credential")) return true;
  const flags = flagsOf(args);
  const has = (...wanted: string[]) => wanted.some((flag) => flags.has(flag));
  const positional = positionalsOf(args);
  const verb = positional[0] ?? "";
  switch (name) {
    case "branch":
      // Listing is fine; creating, renaming, copying, deleting or re-pointing a branch is not.
      return (
        has("-d", "-D", "--delete", "-m", "-M", "--move", "-c", "-C", "--copy", "-f", "--force", "-u", "--set-upstream-to", "--unset-upstream", "--edit-description", "-t", "--track") ||
        (positional.length > 0 && !has(...REF_LISTING))
      );
    case "tag":
      return has("-a", "-s", "-u", "-d", "-f", "-m", "-F", "-e", "--delete", "--annotate", "--sign", "--force", "--message", "--file", "--edit") || (positional.length > 0 && !has(...REF_LISTING, "-v", "--verify"));
    case "stash":
      return !["list", "show"].includes(verb);
    case "worktree":
      return verb !== "list";
    case "remote":
      return ["add", "remove", "rm", "rename", "set-url", "set-head", "set-branches", "prune", "update"].includes(verb);
    case "config":
      if (has("--unset", "--unset-all", "--add", "--replace-all", "--rename-section", "--remove-section", "-e", "--edit")) return true;
      if (["set", "unset", "unset-all", "rename-section", "remove-section", "edit"].includes(verb)) return true;
      if (["get", "list"].includes(verb) || has("--get", "--get-all", "--get-regexp", "--get-urlmatch", "-l", "--list")) return false;
      return positionalsOf(args, ["-f", "--file", "--blob", "--type", "--default"]).length >= 2;
    case "checkout":
      // Naming a branch makes it this worktree's (and a remote-only name creates it through DWIM):
      // the user's clone can no longer check it out. `--detach <ref>` and `[<ref>] -- <paths>` are fine.
      if (has("-b", "-B", "--orphan", "-t", "--track")) return true;
      if (args.includes("--")) return false;
      return positional.length > 0 && !has("--detach");
    case "switch":
      return has("-c", "-C", "--orphan") || !has("-d", "--detach");
    case "reset":
      // Moves the checked-out branch when there is one; naming paths resets the index only.
      return !args.includes("--");
    case "symbolic-ref":
      return has("-d", "--delete") || positional.length >= 2;
    case "reflog":
      return ["expire", "delete"].includes(verb);
    case "notes":
      return !["", "list", "show"].includes(positionalsOf(args, ["--ref"])[0] ?? "");
    case "replace":
      return positional.length > 0 && !has("-l", "--list");
    case "submodule":
      return !["", "status", "summary"].includes(verb);
    case "lfs":
      return LFS_WRITES.has(verb);
    case "hash-object":
      return has("-w");
    case "commit-graph":
    case "multi-pack-index":
      return verb !== "verify";
    case "bundle":
      return verb === "unbundle";
    default:
      return false;
  }
}

/** The `-c key=value` and `--config-env=key=VAR` overrides before git's subcommand. */
function gitConfigOverrides(args: string[]): string[] {
  const overrides: string[] = [];
  for (let i = 0; i < args.length; i++) {
    const arg = args[i] as string;
    if (arg === "-c" || arg === "--config-env") overrides.push(args[++i] ?? "");
    else if (arg.startsWith("--config-env=")) overrides.push(arg.slice("--config-env=".length));
    else if (["-C", "--git-dir", "--work-tree", "--namespace", "--exec-path", "--super-prefix", "--list-cmds"].includes(arg)) i++;
    else if (!arg.startsWith("-")) break;
  }
  return overrides;
}

/** The options of a command: `--opt=value` as `--opt`, `-abc` as `-a`, `-b`, `-c`; nothing past `--`. */
function flagsOf(args: string[]): Set<string> {
  const flags = new Set<string>();
  for (const arg of args) {
    if (arg === "--") break;
    if (arg.startsWith("--")) flags.add(arg.split("=")[0] as string);
    else if (arg.startsWith("-") && arg.length > 1) for (const letter of arg.slice(1)) flags.add(`-${letter}`);
  }
  return flags;
}

/** The words that are not options, nor the value of one of `withValue`. */
function positionalsOf(args: string[], withValue: string[] = []): string[] {
  const positional: string[] = [];
  for (let i = 0; i < args.length; i++) {
    const arg = args[i] as string;
    if (withValue.includes(arg)) i++;
    else if (!arg.startsWith("-") || arg === "-") positional.push(arg);
  }
  return positional;
}

function ghWrites(args: string[], command: string): boolean {
  const [group, verb, sub] = positionalsOf(args, [...GH_OPTIONS_WITH_VALUE]).filter((a) => a !== "-");
  if (group === undefined) return false;
  if (group.includes("$") || verb?.includes("$")) return true;
  if (group === "api") return ghApiWrites(args, verb, command);
  if (verb === undefined) return false;
  if (GH_WRITES[group]?.has(verb)) return true;
  const reads = GH_READS[group];
  if (reads !== undefined && !reads.has(verb)) return true;
  return sub !== undefined && (GH_NESTED_WRITES[`${group} ${verb}`]?.has(sub) ?? false);
}

function ghApiWrites(args: string[], endpoint: string | undefined, command: string): boolean {
  // GraphQL always posts: a mutation is the write. The query may come from a file or a heredoc, so
  // the whole command line is searched, best effort.
  if (endpoint === "graphql") return /\bmutation\b/i.test(command);
  // REST reads unless told to send: a method other than GET or HEAD, or fields (which imply POST).
  const method = optionValue(args, "-X", "--method");
  if (method !== undefined) return !["GET", "HEAD"].includes(method.toUpperCase());
  return args.some((a) => /^(-[fF]|--field|--raw-field|--input)/.test(a));
}

/** The value of `-X POST`, `-XPOST`, `-X=POST`, `--method POST` or `--method=POST`. */
function optionValue(args: string[], short: string, long: string): string | undefined {
  for (let i = 0; i < args.length; i++) {
    const arg = args[i] as string;
    if (arg === short || arg === long) return args[i + 1];
    if (arg.startsWith(`${long}=`)) return arg.slice(long.length + 1);
    if (arg.startsWith(short)) return arg.slice(short.length).replace(/^=/, "");
  }
  return undefined;
}

/** An HTTP client sending to GitHub, or reaching it with a token (what follows cannot be read). */
function httpWritesGitHub(program: string, args: string[]): boolean {
  const text = args.join(" ");
  if (!/github\.com/i.test(text)) return false;
  if (TOKEN.test(text)) return true;
  switch (program) {
    case "curl": {
      const method = curlMethod(args);
      if (method !== undefined && !["GET", "HEAD"].includes(method.toUpperCase())) return true;
      return args.some((a) => /^-[A-Za-z]*[dFTuK]/.test(a) || /^--(data(-\w+)?|form(-string)?|upload-file|json|user|oauth2-bearer|config)(=|$)/.test(a));
    }
    case "wget": {
      const method = args.find((a) => a.startsWith("--method="))?.slice("--method=".length);
      if (method !== undefined && !["GET", "HEAD"].includes(method.toUpperCase())) return true;
      return args.some((a) => /^--(post-data|post-file|body-data|body-file|user|password|http-user|http-password)(=|$)/.test(a));
    }
    default: {
      // httpie and xh: the method is a word, and `key=value` or `key:=json` items imply POST.
      const positional = positionalsOf(args, ["-a", "--auth", "-A", "--auth-type"]);
      if (/^(post|put|patch|delete)$/i.test(positional[0] ?? "")) return true;
      if (args.some((a) => ["-a", "--auth", "--auth-type", "-A"].includes(a) || a.startsWith("--auth="))) return true;
      return positional.some((a) => /^[A-Za-z_][\w-]*(:=|=(?!=)|@)/.test(a));
    }
  }
}

/** curl's method: `-X POST`, `-XPOST`, `-sSX POST`, `--request POST`, `--request=POST`. */
function curlMethod(args: string[]): string | undefined {
  for (let i = 0; i < args.length; i++) {
    const arg = args[i] as string;
    if (arg === "--request") return args[i + 1];
    if (arg.startsWith("--request=")) return arg.slice("--request=".length);
    const short = /^-[A-Za-z]*X(.*)$/.exec(arg);
    if (short !== null) return short[1] === "" ? args[i + 1] : (short[1] as string).replace(/^=/, "");
  }
  return undefined;
}
