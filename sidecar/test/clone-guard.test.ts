import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { cloneCommandRefusal, cloneWriteRefusal, withRealPaths } from "../src/guard/clone-guard.ts";
import type { SourceClone } from "../src/protocol.ts";

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
  test("a path under the clone's .git is refused without a worktree path to use", () => {
    const refusal = cloneWriteRefusal("Write", { file_path: "/code/web/.git/config" }, CWD, CLONES);
    expect(refusal).toContain("/code/web/.git/config");
    expect(refusal).not.toContain("/ws/feat/web/.git");
  });
  test("a worktree inside its clone stays writable", () => {
    const nested: SourceClone[] = [{ clone: "/code/web", worktree: "/code/web/.wt/feat" }];
    expect(cloneWriteRefusal("Write", { file_path: "/code/web/.wt/feat/a.ts" }, CWD, nested)).toBeUndefined();
    expect(cloneWriteRefusal("Write", { file_path: "/code/web/a.ts" }, CWD, nested)).toBeDefined();
  });
  test("the worktree's own git folder in the clone's .git stays writable, the rest of .git does not", () => {
    const root = mkdtempSync(join(tmpdir(), "clone-gitdir-"));
    try {
      const clone = join(root, "code", "web");
      const worktree = join(root, "ws", "web");
      const gitdir = join(clone, ".git", "worktrees", "web");
      mkdirSync(gitdir, { recursive: true });
      mkdirSync(worktree, { recursive: true });
      writeFileSync(join(worktree, ".git"), `gitdir: ${gitdir}\n`);
      const clones = withRealPaths([{ clone, worktree }]);
      expect(cloneWriteRefusal("Write", { file_path: join(gitdir, "info", "exclude") }, root, clones)).toBeUndefined();
      expect(cloneWriteRefusal("Write", { file_path: join(clone, ".git", "config") }, root, clones)).toBeDefined();
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
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
    "git --work-tree /code/web add -A",
    "git --git-dir /code/web/.git commit -m x",
    "pushd /code/web && git commit -am x",
    "echo x > $HOME/../../code/web/a",
    "sed --in-place s/a/b/ /code/web/f",
    "find /code/web -name '*.log' -delete",
    "find /code/web -name x -exec rm {} ;",
    "(cd /code/web && git log) && cd /code/web && git commit -m x",
    'echo "$(rm -rf /code/web/build)"',
    'echo "`touch /code/web/a`"',
    'X="$(cd /code/web && git commit -am x)"',
    "git -C /code/web diff --output=/code/web/report.patch",
    "git diff --output /code/web/report.patch",
    "git format-patch -3 --output-directory=/code/web/patches",
    "git -C /code/web config --unset user.email",
    "git -C /code/web config --remove-section user",
    "git --work-tree=/ws/feat/web --git-dir=/code/web/.git checkout .",
    "git --git-dir=/code/web/.git --work-tree=/ws/feat/web checkout .",
    "cp -t /code/web a.ts",
    "cp --target-directory=/code/web a.ts",
    "cd /code/web && find -name '*.log' -delete",
    "git clone https://example.com/x.git /code/web/vendor/x",
    "cd /code/web && git clone https://example.com/x.git",
    "{ cd /code/web; touch x; }",
    "{ cd /code/web; } && touch x",
    "{ cd /code/web; ls; }; touch x",
    "git --git-dir=/ws/feat/web/.git --work-tree=/code/web checkout .",
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
    'grep -rn "->" /code/web/src',
    "rg '=>' /code/web",
    "cd /code/web && git log --format='%h -> %s'",
    "cd /code/web && jq '.x > 1' package.json",
    "(cd /code/web && git log -1) && git commit -m x",
    "BASE=$(cd /code/web && git rev-parse HEAD); git commit -m x",
    "cd /code/web | true; touch x",
    "git -C /code/web remote get-url origin",
    "git -C /code/web branch --contains abc",
    "git -C /code/web branch --merged main",
    "git -C /code/web reflog",
    "git -C /code/web config user.email",
    "git -C /code/web diff-tree -r HEAD",
    "find /code/web -name '*.ts'",
    "echo 'a; cd /code/web; b' && touch x",
    "cd /code/web && jq '.a >.b' package.json",
    'echo "$(git -C /code/web rev-parse HEAD)"',
    "git -C /code/web diff --output=/tmp/report.patch",
    "echo '$(rm -rf /code/web/build)'",
    "echo '\"$(rm -rf /code/web/build)\"'",
    "cd /code/web && git --git-dir=/ws/feat/web/.git --work-tree=/ws/feat/web add .",
    "cp -t /ws/feat/web /code/web/a.ts",
    "rsync -t /code/web/a.ts web/a.ts",
    "git clone https://example.com/x.git /tmp/x",
    "cd /code/web && find . -name '*.ts' -exec grep -l x {} ;",
    "( cd /code/web; ls ) && { cd web; touch x; }",
  ];
  for (const command of allowed) {
    test(`allows: ${JSON.stringify(command)}`, () => {
      expect(cloneCommandRefusal(command, CWD, CLONES)).toBeUndefined();
    });
  }
});
