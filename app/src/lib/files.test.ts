import { describe, expect, test } from "bun:test";
import { ancestors, buildIndex, buildTree, filterFiles, resolveFileRef, resolveWorkspacesPath } from "./files";

const index = buildIndex([
  {
    name: "web",
    repo: "web",
    files: ["README.md", "src/search/facets.ts", "src/search/index.ts", "src/app.tsx", "package.json"],
    truncated: false,
  },
  { name: "api", repo: "api", files: ["README.md", "api/facets.py"], truncated: false },
  { name: ".chauffeur", repo: null, files: ["tickets.md"], truncated: false },
]);

describe("the file tree", () => {
  test("nests folders first, each level sorted", () => {
    const tree = buildTree(index.roots[0]);
    expect(tree.dirs.map((d) => d.path)).toEqual(["web/src"]);
    expect(tree.files).toEqual(["web/package.json", "web/README.md"]);
    const src = tree.dirs[0];
    expect(src.dirs.map((d) => d.name)).toEqual(["search"]);
    expect(src.dirs[0].files).toEqual(["web/src/search/facets.ts", "web/src/search/index.ts"]);
  });

  test("lists the folders to reveal a file", () => {
    expect(ancestors("web/src/search/facets.ts")).toEqual(["web", "web/src", "web/src/search"]);
  });
});

describe("quick open", () => {
  test("ranks base-name matches first", () => {
    expect(filterFiles(index.paths, "facets")).toEqual(["api/api/facets.py", "web/src/search/facets.ts"]);
    expect(filterFiles(index.paths, "srchidx")).toEqual(["web/src/search/index.ts"]);
    expect(filterFiles(index.paths, "zzz")).toEqual([]);
    // "fcts" is in no path as typed: scattered letters then.
    expect(filterFiles(index.paths, "fcts").slice(0, 2)).toEqual(["api/api/facets.py", "web/src/search/facets.ts"]);
    expect(filterFiles(index.paths, "")).toEqual([]);
  });
});

describe("file references in chats", () => {
  const ws = "/Users/me/chauffeur/workspaces/xyz-1";
  test("resolves workspace paths, absolute paths, repo paths and unique names, with their line", () => {
    expect(resolveFileRef("web/src/app.tsx", index, ws)).toEqual({ path: "web/src/app.tsx", line: null });
    expect(resolveFileRef("web/src/app.tsx:42", index, ws)).toEqual({ path: "web/src/app.tsx", line: 42 });
    expect(resolveFileRef(`${ws}/api/api/facets.py:7:3`, index, ws)).toEqual({
      path: "api/api/facets.py",
      line: 7,
    });
    expect(resolveFileRef("src/search/index.ts#L12-L20", index, ws)).toEqual({ path: "web/src/search/index.ts", line: 12 });
    expect(resolveFileRef("./api/facets.py", index, ws)).toEqual({ path: "api/api/facets.py", line: null });
    expect(resolveFileRef("tickets.md", index, ws)).toEqual({ path: ".chauffeur/tickets.md", line: null });
    expect(resolveFileRef("/private/tmp/w/web/src/app.tsx", index, "/tmp/w")).toEqual({
      path: "web/src/app.tsx",
      line: null,
    });
  });

  test("refuses what is ambiguous, missing or not a path", () => {
    expect(resolveFileRef("README.md", index, ws)).toBeNull();
    expect(resolveFileRef("facets.ts.bak", index, ws)).toBeNull();
    expect(resolveFileRef("/etc/hosts", index, ws)).toBeNull();
    expect(resolveFileRef("https://example.com/a.ts", index, ws)).toBeNull();
    expect(resolveFileRef("npm run build", index, ws)).toBeNull();
    expect(resolveFileRef("useState", index, ws)).toBeNull();
  });
});

describe("paths Finder may show", () => {
  const here = "/Users/j/chauffeur/workspaces/mine";
  const other = "/Users/j/chauffeur/workspaces/merge-check/.chauffeur/recipes/aime";

  test("a folder of another workspace, as a path or a file:// link", () => {
    expect(resolveWorkspacesPath(`${other}/`, here)).toBe(other);
    expect(resolveWorkspacesPath(`file://${other}/`, here)).toBe(other);
    expect(resolveWorkspacesPath(`(${other}).`, here)).toBe(other);
  });

  test("nothing outside the folder of the workspaces", () => {
    expect(resolveWorkspacesPath("/Users/j/Documents", here)).toBeNull();
    expect(resolveWorkspacesPath("/Users/j/chauffeur/workspaces", here)).toBeNull();
    expect(resolveWorkspacesPath("/Users/j/chauffeur/workspaces-old/x", here)).toBeNull();
    expect(resolveWorkspacesPath(`${other}/../../../../../../etc`, here)).toBeNull();
    expect(resolveWorkspacesPath("web/src", here)).toBeNull();
    expect(resolveWorkspacesPath(other, null)).toBeNull();
  });
});
