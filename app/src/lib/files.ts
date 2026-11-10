// The files of a workspace as the browser shows them: a tree per root, a quick-open filter, and
// the file references agents write in chats (`web/src/a.ts:42`, absolute paths…).
// Paths are under the workspace folder: `<root>/<path>`, a root being a repo or `.chauffeur`.

import type { FileRoot } from "@chauffeur/protocol";

export type FileIndex = {
  roots: FileRoot[];
  /** Every file, under the workspace folder. */
  paths: string[];
  set: Set<string>;
  /** Files by their path inside their root, for references that leave the repo out. */
  byInnerPath: Map<string, string[]>;
  /** Files by base name. */
  byName: Map<string, string[]>;
};

export function buildIndex(roots: FileRoot[]): FileIndex {
  const paths: string[] = [];
  const byInnerPath = new Map<string, string[]>();
  const byName = new Map<string, string[]>();
  for (const root of roots) {
    for (const file of root.files) {
      const path = `${root.name}/${file}`;
      paths.push(path);
      push(byInnerPath, file, path);
      push(byName, basename(file), path);
    }
  }
  return { roots, paths, set: new Set(paths), byInnerPath, byName };
}

function push(map: Map<string, string[]>, key: string, value: string) {
  const list = map.get(key);
  if (list) list.push(value);
  else map.set(key, [value]);
}

export function basename(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

export function dirname(path: string): string {
  const index = path.lastIndexOf("/");
  return index >= 0 ? path.slice(0, index) : "";
}

export type TreeDir = { name: string; path: string; dirs: TreeDir[]; files: string[] };

/** A root's files as nested folders, folders first, each level sorted. */
export function buildTree(root: FileRoot): TreeDir {
  const top: TreeDir = { name: root.name, path: root.name, dirs: [], files: [] };
  const dirs = new Map<string, TreeDir>([["", top]]);
  const dirOf = (inner: string): TreeDir => {
    const existing = dirs.get(inner);
    if (existing) return existing;
    const parent = dirOf(dirname(inner));
    const dir: TreeDir = { name: basename(inner), path: `${root.name}/${inner}`, dirs: [], files: [] };
    parent.dirs.push(dir);
    dirs.set(inner, dir);
    return dir;
  };
  for (const file of root.files) dirOf(dirname(file)).files.push(`${root.name}/${file}`);
  const sort = (dir: TreeDir) => {
    dir.dirs.sort((a, b) => a.name.localeCompare(b.name));
    dir.files.sort((a, b) => basename(a).localeCompare(basename(b)));
    dir.dirs.forEach(sort);
  };
  sort(top);
  return top;
}

/** The folders leading to `path`, for revealing it in the tree. */
export function ancestors(path: string): string[] {
  const parts = path.split("/");
  const out: string[] = [];
  for (let i = 1; i < parts.length; i++) out.push(parts.slice(0, i).join("/"));
  return out;
}

/**
 * Quick open: files whose path contains the query, or else its characters in order, best first.
 * A match in the base name, contiguous runs and short paths rank higher.
 */
export function filterFiles(paths: string[], query: string, limit = 100): string[] {
  const q = query.toLowerCase().replace(/\s+/g, "");
  if (!q) return [];
  let scored: Array<{ path: string; score: number; whole: boolean }> = [];
  for (const path of paths) {
    const lower = path.toLowerCase();
    const score = matchScore(lower, q);
    if (score !== null) scored.push({ path, score, whole: lower.includes(q) });
  }
  // Scattered letters only when nothing contains the query as typed.
  if (scored.some((s) => s.whole)) scored = scored.filter((s) => s.whole);
  scored.sort((a, b) => b.score - a.score || a.path.length - b.path.length || a.path.localeCompare(b.path));
  return scored.slice(0, limit).map((s) => s.path);
}

function matchScore(path: string, query: string): number | null {
  const nameStart = path.lastIndexOf("/") + 1;
  const name = path.slice(nameStart);
  let score = 0;
  if (name.includes(query)) score += 100 + (name.startsWith(query) ? 50 : 0) + (name === query ? 50 : 0);
  else if (path.includes(query)) score += 60;
  // Characters in order, rewarding runs and base-name hits.
  let position = 0;
  let run = 0;
  for (const char of query) {
    const found = path.indexOf(char, position);
    if (found < 0) return null;
    run = found === position ? run + 1 : 0;
    score += run * 2 + (found >= nameStart ? 3 : 0);
    position = found + 1;
  }
  return score - path.length / 50;
}

export type FileRef = { path: string; line: number | null };

/** macOS resolves /tmp and /var to /private/tmp and /private/var: agents may report either. */
export function withoutPrivate(path: string): string {
  return path.replace(/^\/private(?=\/(?:tmp|var)\/)/, "");
}

/**
 * The file a piece of chat text names, if it names exactly one: a path under the workspace
 * (`web/src/a.ts`), an absolute path inside it, a path inside one repo (`src/a.ts`), or a
 * unique base name with an extension. `:42`, `:42:7` and `#L42` give the line.
 */
export function resolveFileRef(text: string, index: FileIndex, workspacePath: string | null): FileRef | null {
  let ref = text.trim().replace(/^file:\/\//, "").replace(/^["'`(<]+|[)"'`>.,;:]+$/g, "");
  let line: number | null = null;
  const anchor = /#L(\d+)(?:-L?\d+)?$/.exec(ref);
  if (anchor) {
    line = Number(anchor[1]);
    ref = ref.slice(0, anchor.index);
  } else {
    const suffix = /:(\d+)(?::\d+)?(?:-\d+)?$/.exec(ref);
    if (suffix) {
      line = Number(suffix[1]);
      ref = ref.slice(0, suffix.index);
    }
  }
  if (!ref || ref.includes(" ") || /^[a-z]+:\/\//i.test(ref)) return null;
  if (ref.startsWith("/")) {
    const base = workspacePath && withoutPrivate(workspacePath.replace(/\/$/, ""));
    ref = withoutPrivate(ref);
    if (!base || !ref.startsWith(`${base}/`)) return null;
    ref = ref.slice(base.length + 1);
  }
  ref = ref.replace(/^\.\//, "");
  if (index.set.has(ref)) return { path: ref, line };
  const inner = index.byInnerPath.get(ref);
  if (inner?.length === 1) return { path: inner[0], line };
  if (!ref.includes("/") && /\.\w+$/.test(ref)) {
    const named = index.byName.get(ref);
    if (named?.length === 1) return { path: named[0], line };
  }
  return null;
}

/**
 * The absolute path of a file or folder inside the folder holding the workspaces, for showing it in
 * Finder: a link to another workspace's notes (`file://` accepted, a trailing `/` and punctuation
 * dropped). `workspacePath` tells where that folder is.
 */
export function resolveWorkspacesPath(text: string, workspacePath: string | null): string | null {
  if (!workspacePath) return null;
  const ref = text
    .trim()
    .replace(/^file:\/\//, "")
    .replace(/^["'`(<]+|[)"'`>.,;:]+$/g, "")
    .replace(/\/+$/, "");
  if (!ref.startsWith("/") || ref.split("/").includes("..")) return null;
  const base = dirname(withoutPrivate(workspacePath.replace(/\/$/, "")));
  return base && withoutPrivate(ref).startsWith(`${base}/`) ? ref : null;
}
