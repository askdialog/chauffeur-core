// Splits a repo's unified patch (`git diff` output) into files and hunks with line numbers.

import type { RepoDiff } from "@chauffeur/protocol";

export type DiffLine = {
  type: "add" | "del" | "ctx" | "meta";
  text: string;
  oldNo: number | null;
  newNo: number | null;
};

export type DiffHunk = { header: string; lines: DiffLine[] };

export type PatchFile = {
  /** New path (old path for deletions). */
  path: string;
  oldPath: string | null;
  binary: boolean;
  hunks: DiffHunk[];
};

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;

function unquote(path: string): string {
  if (path.startsWith('"') && path.endsWith('"')) {
    return path.slice(1, -1).replace(/\\(.)/g, "$1");
  }
  return path;
}

function stripPrefix(path: string): string | null {
  const p = unquote(path.trim());
  if (p === "/dev/null") return null;
  return p.replace(/^[ab]\//, "");
}

export function parsePatch(patch: string): PatchFile[] {
  const files: PatchFile[] = [];
  let file: PatchFile | null = null;
  let hunk: DiffHunk | null = null;
  let oldNo = 0;
  let newNo = 0;
  const lines = patch.split("\n");
  if (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
  for (const line of lines) {
    if (line.startsWith("diff --git ")) {
      const match = /^diff --git (?:"?a\/)(.+?)"? (?:"?b\/)(.+?)"?$/.exec(line);
      file = {
        path: match ? match[2] : line.slice("diff --git ".length),
        oldPath: null,
        binary: false,
        hunks: [],
      };
      files.push(file);
      hunk = null;
      continue;
    }
    if (!file) continue;
    if (!hunk) {
      if (line.startsWith("--- ")) {
        const old = stripPrefix(line.slice(4));
        if (old && old !== file.path) file.oldPath = old;
        continue;
      }
      if (line.startsWith("+++ ")) {
        const next = stripPrefix(line.slice(4));
        if (next) file.path = next;
        else if (file.oldPath) file.path = file.oldPath;
        continue;
      }
      if (line.startsWith("rename from ")) {
        file.oldPath = unquote(line.slice("rename from ".length));
        continue;
      }
      if (line.startsWith("rename to ")) {
        file.path = unquote(line.slice("rename to ".length));
        continue;
      }
      if (line.startsWith("Binary files ") || line === "GIT binary patch") {
        file.binary = true;
        continue;
      }
    }
    const header = HUNK.exec(line);
    if (header) {
      oldNo = Number(header[1]);
      newNo = Number(header[2]);
      hunk = { header: line, lines: [] };
      file.hunks.push(hunk);
      continue;
    }
    if (!hunk) continue;
    if (line.startsWith("+")) {
      hunk.lines.push({ type: "add", text: line.slice(1), oldNo: null, newNo: newNo++ });
    } else if (line.startsWith("-")) {
      hunk.lines.push({ type: "del", text: line.slice(1), oldNo: oldNo++, newNo: null });
    } else if (line.startsWith("\\")) {
      hunk.lines.push({ type: "meta", text: line, oldNo: null, newNo: null });
    } else {
      hunk.lines.push({ type: "ctx", text: line.slice(1), oldNo: oldNo++, newNo: newNo++ });
    }
  }
  return files;
}

function sameFields<T extends object>(a: T, b: T, skip?: keyof T): boolean {
  if (Object.keys(a).length !== Object.keys(b).length) return false;
  for (const key in a) if (key !== skip && !Object.is(a[key], b[key])) return false;
  return true;
}

/**
 * The diff read again, with the objects of `previous` wherever a repo or one of its files did not
 * change, so that the views of unchanged files are not drawn and highlighted again; `previous`
 * itself when nothing changed.
 */
export function keepUnchangedRepos(previous: RepoDiff[] | null, next: RepoDiff[]): RepoDiff[] {
  if (!previous) return next;
  const before = new Map(previous.map((repo) => [repo.worktree_id, repo]));
  const repos = next.map((repo) => {
    const old = before.get(repo.worktree_id);
    if (!old) return repo;
    const known = new Map(old.files.map((file) => [file.path, file]));
    const files = repo.files.map((file) => {
      const same = known.get(file.path);
      return same && sameFields(same, file) ? same : file;
    });
    const sameFiles = files.length === old.files.length && files.every((file, i) => file === old.files[i]);
    if (sameFiles && sameFields(old, repo, "files")) return old;
    return { ...repo, files };
  });
  return repos.length === previous.length && repos.every((repo, i) => repo === previous[i]) ? previous : repos;
}

function sameHunks(a: DiffHunk[], b: DiffHunk[]): boolean {
  return (
    a.length === b.length &&
    a.every(
      (hunk, i) =>
        hunk.header === b[i].header &&
        hunk.lines.length === b[i].lines.length &&
        hunk.lines.every((line, j) => sameFields(line, b[i].lines[j])),
    )
  );
}

/** The files of a patch parsed again, those identical to one of `previous` kept as that object. */
export function keepUnchangedFiles(previous: readonly PatchFile[], next: PatchFile[]): PatchFile[] {
  const known = new Map(previous.map((file) => [file.path, file]));
  return next.map((file) => {
    const old = known.get(file.path);
    return old && old.oldPath === file.oldPath && old.binary === file.binary && sameHunks(old.hunks, file.hunks) ? old : file;
  });
}

/** highlight.js language for a file path, or null for plain text. */
export function languageOf(path: string): string | null {
  const name = path.split("/").pop()?.toLowerCase() ?? "";
  if (name === "dockerfile") return "dockerfile";
  if (name === "makefile") return "makefile";
  const ext = name.includes(".") ? name.split(".").pop()! : "";
  const map: Record<string, string> = {
    ts: "typescript",
    tsx: "typescript",
    mts: "typescript",
    cts: "typescript",
    js: "javascript",
    jsx: "javascript",
    mjs: "javascript",
    cjs: "javascript",
    json: "json",
    rs: "rust",
    py: "python",
    go: "go",
    rb: "ruby",
    java: "java",
    kt: "kotlin",
    swift: "swift",
    css: "css",
    scss: "scss",
    html: "xml",
    xml: "xml",
    svg: "xml",
    vue: "xml",
    md: "markdown",
    yml: "yaml",
    yaml: "yaml",
    toml: "ini",
    ini: "ini",
    sh: "bash",
    bash: "bash",
    zsh: "bash",
    sql: "sql",
    graphql: "graphql",
    gql: "graphql",
    prisma: "typescript",
  };
  return map[ext] ?? null;
}
