import { describe, expect, test } from "bun:test";
import { cloneCommandRefusal, cloneWriteRefusal, type SourceClone } from "../src/guard/clone-guard.ts";

const CLONES: SourceClone[] = [
  { clone: "/code/web", worktree: "/ws/feat/web" },
  { clone: "/code/api", worktree: "/ws/feat/api" },
];
const CWD = "/ws/feat";

describe("cloneWriteRefusal", () => {
  test("refuses a file tool writing into a clone, naming the worktree path", () => {
    const refusal = cloneWriteRefusal("Edit", { file_path: "/code/web/src/app.ts" }, CWD, CLONES);
    expect(refusal).toContain("/code/web/src/app.ts");
    expect(refusal).toContain("/ws/feat/web/src/app.ts");
  });
  const refused: [string, Record<string, unknown>][] = [
    ["Write", { file_path: "/code/web/README.md" }],
    ["MultiEdit", { file_path: "/code/api/main.go" }],
    ["NotebookEdit", { notebook_path: "/code/web/nb.ipynb" }],
    ["Write", { file_path: "../../code/web/x.ts" }],
    ["Edit", { file_path: "/code/web" }],
  ];
  for (const [tool, input] of refused) {
    test(`refuses: ${tool} ${JSON.stringify(input)}`, () => {
      expect(cloneWriteRefusal(tool, input, CWD, CLONES)).toBeDefined();
    });
  }
  const allowed: [string, Record<string, unknown>][] = [
    ["Write", { file_path: "/ws/feat/web/README.md" }],
    ["Edit", { file_path: "web/src/app.ts" }],
    ["Write", { file_path: "/code/website/index.html" }],
    ["Read", { file_path: "/code/web/README.md" }],
    ["Grep", { path: "/code/web" }],
    ["Write", { file_path: "/tmp/notes.md" }],
  ];
  for (const [tool, input] of allowed) {
    test(`allows: ${tool} ${JSON.stringify(input)}`, () => {
      expect(cloneWriteRefusal(tool, input, CWD, CLONES)).toBeUndefined();
    });
  }
  test("a worktree inside its clone stays writable", () => {
    const nested: SourceClone[] = [{ clone: "/code/web", worktree: "/code/web/.wt/feat" }];
    expect(cloneWriteRefusal("Write", { file_path: "/code/web/.wt/feat/a.ts" }, CWD, nested)).toBeUndefined();
    expect(cloneWriteRefusal("Write", { file_path: "/code/web/a.ts" }, CWD, nested)).toBeDefined();
  });
});

describe("cloneCommandRefusal", () => {
  test("refuses a command writing into a clone, naming the worktree", () => {
    const refusal = cloneCommandRefusal("echo hi > /code/web/notes.txt", CWD, CLONES);
    expect(refusal).toContain("/code/web");
    expect(refusal).toContain("/ws/feat/web");
  });
  const refused = [
    "echo hi > /code/web/notes.txt",
    "echo hi >/code/web/notes.txt",
    "echo hi>>/code/web/notes.txt",
    "make 2>/code/web/err.log",
    "printf x | tee -a /code/web/f",
    "rm -rf /code/web/build",
    "mkdir /code/api/tmp",
    "touch /code/web/a",
    "mv /code/web/a /tmp/a",
    "cp /tmp/a /code/web/a",
    "rsync -a dist/ /code/web/dist/",
    "sed -i s/a/b/ /code/web/f",
    "perl -pi -e s/a/b/ /code/web/f",
    "chmod +x /code/web/run.sh",
    "dd if=/dev/zero of=/code/web/blob bs=1 count=1",
    "git -C /code/web commit -m x",
    "git -C /code/web checkout -b other",
    "git -C /code/web stash",
    "git -C /code/web branch new-one",
    "git --work-tree=/code/web add -A",
    "cd /code/web && git commit -am x",
    "cd /code/web && rm -rf build",
    "cd /code/web; echo x > f",
    "(cd /code/api && git reset --hard)",
    "bash -c 'echo y > /code/web/z'",
    "cd /code && rm -rf web/build",
  ];
  for (const command of refused) {
    test(`refuses: ${JSON.stringify(command)}`, () => {
      expect(cloneCommandRefusal(command, CWD, CLONES)).toBeDefined();
    });
  }
  const allowed = [
    "cat /code/web/README.md",
    "grep -r TODO /code/web",
    "ls /code/web",
    "cp /code/web/a ./web/a",
    "rsync -a /code/web/dist/ web/dist/",
    "git -C /code/web status",
    "git -C /code/web log --oneline -5",
    "git -C /code/web diff",
    "git -C /code/web branch",
    "git -C /code/web branch -a",
    "git -C /code/web stash list",
    "git -C /code/web remote -v",
    "cd /code/web && git log",
    "cd /code/web && cat README.md",
    "echo hi > web/notes.txt",
    "make 2>&1 | tee build.log",
    "rm -rf /ws/feat/web/build",
    "git -C web commit -m x",
    "echo /code/web",
    "diff /code/web/a web/a > /tmp/out",
  ];
  for (const command of allowed) {
    test(`allows: ${JSON.stringify(command)}`, () => {
      expect(cloneCommandRefusal(command, CWD, CLONES)).toBeUndefined();
    });
  }
});
