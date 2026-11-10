import { describe, expect, test } from "bun:test";
import type { RepoDiff } from "@chauffeur/protocol";
import { keepUnchangedFiles, keepUnchangedRepos, languageOf, parsePatch } from "./diff";
import { highlightLines } from "./highlight";

const PATCH = `diff --git a/src/app.ts b/src/app.ts
index 1111111..2222222 100644
--- a/src/app.ts
+++ b/src/app.ts
@@ -1,4 +1,5 @@
 import x from "x";
-const a = 1;
+const a = 2;
+const b = 3;
 export { a };
 // end
@@ -10,2 +11,2 @@ function f() {
-  return 1;
+  return 2;
 }
\\ No newline at end of file
diff --git a/old/name.md b/new/name.md
similarity index 90%
rename from old/name.md
rename to new/name.md
--- a/old/name.md
+++ b/new/name.md
@@ -1 +1 @@
-hello
+hello world
diff --git a/added.txt b/added.txt
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/added.txt
@@ -0,0 +1,2 @@
+one
+two
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
--- a/gone.txt
+++ /dev/null
@@ -1 +0,0 @@
-bye
diff --git a/logo.png b/logo.png
new file mode 100644
Binary files /dev/null and b/logo.png differ
`;

describe("parsePatch", () => {
  const files = parsePatch(PATCH);

  test("splits the patch per file", () => {
    expect(files.map((f) => f.path)).toEqual(["src/app.ts", "new/name.md", "added.txt", "gone.txt", "logo.png"]);
  });

  test("numbers lines on both sides", () => {
    const [first, second] = files[0].hunks;
    expect(first.lines.map((l) => [l.type, l.oldNo, l.newNo])).toEqual([
      ["ctx", 1, 1],
      ["del", 2, null],
      ["add", null, 2],
      ["add", null, 3],
      ["ctx", 3, 4],
      ["ctx", 4, 5],
    ]);
    expect(second.header).toBe("@@ -10,2 +11,2 @@ function f() {");
    expect(second.lines.map((l) => l.type)).toEqual(["del", "add", "ctx", "meta"]);
    expect(second.lines[2]).toMatchObject({ oldNo: 11, newNo: 12, text: "}" });
  });

  test("renames, additions, deletions and binaries", () => {
    expect(files[1].oldPath).toBe("old/name.md");
    expect(files[2].hunks[0].lines.map((l) => l.text)).toEqual(["one", "two"]);
    expect(files[3].path).toBe("gone.txt");
    expect(files[3].hunks[0].lines[0]).toMatchObject({ type: "del", text: "bye" });
    expect(files[4].binary).toBe(true);
  });

  test("an empty patch has no files", () => {
    expect(parsePatch("")).toEqual([]);
  });
});

describe("highlighting", () => {
  test("maps extensions to languages", () => {
    expect(languageOf("a/b/c.tsx")).toBe("typescript");
    expect(languageOf("Cargo.toml")).toBe("ini");
    expect(languageOf("Dockerfile")).toBe("dockerfile");
    expect(languageOf("README")).toBeNull();
  });

  test("keeps one balanced HTML string per line, across multi-line tokens", () => {
    const lines = ["const a = `multi", "line` + 1;", "// done <b>"];
    const html = highlightLines(lines, "typescript");
    expect(html).toHaveLength(3);
    for (const line of html) {
      expect((line.match(/<span/g) ?? []).length).toBe((line.match(/<\/span>/g) ?? []).length);
    }
    expect(html[1]).toContain("hljs-string");
    expect(html[2]).toContain("&lt;b&gt;");
  });

  test("escapes unknown languages", () => {
    expect(highlightLines(["<x>"], null)).toEqual(["&lt;x&gt;"]);
  });
});

describe("reading the diff again", () => {
  const repo = (patch: string, additions = 2): RepoDiff => ({
    repo: "alpha",
    worktree_id: "t1",
    path: "/w/alpha",
    branch: "b",
    base_branch: "main",
    merge_base: null,
    ahead: 0,
    files: [
      { path: "src/app.ts", old_path: null, status: "modified", additions, deletions: 1, binary: false },
      { path: "added.txt", old_path: null, status: "added", additions: 2, deletions: 0, binary: false },
    ],
    patch,
    truncated: false,
  });

  test("an unchanged diff is the one already shown", () => {
    const shown = [repo(PATCH)];
    expect(keepUnchangedRepos(shown, [repo(PATCH)])).toBe(shown);
  });

  test("a changed repo keeps the objects of its unchanged files", () => {
    const shown = [repo(PATCH)];
    const next = keepUnchangedRepos(shown, [repo(`${PATCH}\n`, 3)]);
    expect(next).not.toBe(shown);
    expect(next[0].patch).toBe(`${PATCH}\n`);
    expect(next[0].files[0]).not.toBe(shown[0].files[0]);
    expect(next[0].files[0].additions).toBe(3);
    expect(next[0].files[1]).toBe(shown[0].files[1]);
  });

  test("a patch parsed again keeps the files whose lines did not change", () => {
    const before = parsePatch(PATCH);
    const after = keepUnchangedFiles(before, parsePatch(PATCH.replace("+hello world", "+hello there")));
    expect(after.map((f) => f.path)).toEqual(before.map((f) => f.path));
    expect(after[0]).toBe(before[0]);
    expect(after[1]).not.toBe(before[1]);
    expect(after[1].hunks[0].lines[1].text).toBe("hello there");
    expect(after.slice(2)).toEqual(before.slice(2));
    after.slice(2).forEach((file, i) => expect(file).toBe(before[i + 2]));
  });
});
