import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
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
    ["Write", { file_path: "/code/web/..config/x" }],
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
  const refuses = (command: string) => cloneCommandRefusal(command, CWD, CLONES);
  const table = (title: string, commands: string[], refused: boolean) => {
    for (const command of commands) {
      test(`${title}: ${JSON.stringify(command)}`, () => {
        if (refused) expect(refuses(command)).toBeDefined();
        else expect(refuses(command)).toBeUndefined();
      });
    }
  };

  test("refuses a command changing a clone, naming the worktree", () => {
    const refusal = refuses("echo hi > /code/web/notes.txt");
    expect(refusal).toContain("/code/web");
    expect(refusal).toContain("/ws/feat/web");
  });

  // Fail-closed: any program but a known read, run in a clone, is refused; the same, run in the
  // workspace, never is.
  const programs = [
    "npm install",
    "npm test",
    "make",
    "python3 setup.py build",
    "pnpm i",
    "cargo build",
    "tar xf /tmp/a.tgz",
    "unzip -o /tmp/a.zip",
    "git apply /tmp/p.diff",
    "patch -p1 -i /tmp/p.diff",
    "truncate -s 0 f",
    "ln -sf /tmp/x link",
    "chown me f",
    "npx prettier --write .",
    "touch x",
    "rm -rf build",
    "mv a b",
    "cp a b",
    "tee f",
    "sed -i s/a/b/ f",
    "sort -o f f",
    "git pull",
    "git fetch",
    "git commit -am x",
    "git checkout -b other",
    "git stash",
    "git branch new-one",
    "git config user.email x",
  ];
  table("refuses, run in a clone", programs.map((program) => `cd /code/web && ${program}`), true);
  table("allows, run in the workspace", programs, false);
  const reads = [
    "cat README.md",
    "ls -la",
    "grep -rn TODO .",
    "rg foo",
    "wc -l README.md",
    "head -5 README.md",
    "sed -n 1,5p README.md",
    "awk '{print $1}' README.md",
    "jq . package.json",
    "sort README.md",
    "find . -name '*.ts'",
    "find . -name '*.ts' -exec grep -l x {} ;",
    "git status",
    "git log --oneline -3",
    "git diff HEAD~1",
    "git show HEAD:README.md",
    "git branch -vv",
    "git branch --contains abc",
    "git branch --merged main",
    "git tag -l 'v*'",
    "git stash list",
    "git remote -v",
    "git remote get-url origin",
    "git reflog",
    "git config user.email",
    "git config --get remote.origin.url",
    "git diff-tree -r HEAD",
    "git log --format='%h -> %s' | head",
    "git log 2>&1 | tee /tmp/log",
    "ls >&2",
    "ls 2>&-",
    "ls >& 2",
  ];
  table("allows a read in a clone", reads.map((read) => `cd /code/web && ${read}`), false);

  table(
    "refuses a write naming a clone",
    [
      "echo hi > /code/web/notes.txt",
      "echo hi >/code/web/notes.txt",
      "echo hi>>/code/web/notes.txt",
      "echo x >| /code/web/f",
      "echo x >& /code/web/f",
      "echo x &>> /code/web/f",
      "make 2>/code/web/err.log",
      "> /code/web/f echo hi",
      "2>&1 git -C /code/web commit -m x",
      "printf x | tee -a /code/web/f",
      "rm -rf /code/web/build",
      "rm -rf /code/web/build > /dev/null",
      "mkdir /code/api/tmp",
      "touch /code/web/a",
      "mv /code/web/a /tmp/a",
      "cp /tmp/a /code/web/a",
      "cp a.ts /code/web/",
      "cp -t /code/web a.ts",
      "cp --target-directory=/code/web a.ts",
      "cp a.ts /code/web/a.ts 2> /dev/null",
      "cp a.ts /code/web/a.ts 2>&1",
      "> /tmp/out cp a.ts /code/web/a.ts",
      "ln -s /tmp/x /code/web/link",
      "rsync -a dist/ /code/web/dist/",
      "rsync -a --remove-source-files /code/web/dist/ /tmp/dist/",
      "sed -i s/a/b/ /code/web/f",
      "sed --in-place s/a/b/ /code/web/f",
      "perl -pi -e s/a/b/ /code/web/f",
      "chmod +x /code/web/run.sh",
      "dd if=/dev/zero of=/code/web/blob bs=1 count=1",
      "sort -o /code/web/x y",
      "uniq a /code/web/b",
      "time -o /code/web/t ls",
      "awk '{print > \"/code/web/x\"}' f",
      "python3 -c \"open('/code/web/x', 'w')\"",
      "node -e \"require('fs').writeFileSync('/code/web/x', '')\"",
      "docker run -v /code/web:/app node npm i",
      "npm --prefix /code/web install",
      "echo x > $HOME/../../code/web/a",
      "cat <<EOF > /code/web/x\nhi\nEOF",
      "tee /code/web/x <<'EOF'\nhi\nEOF",
    ],
    true,
  );

  table(
    "refuses a write to a folder holding a clone",
    ["rm -rf /code", "chmod -R u+w /code", "rm -rf /code/*", "cd /code && rm -rf web/build", "cd /code/web && rm -rf *"],
    true,
  );

  table(
    "refuses git changing a clone",
    [
      "git -C /code/web commit -m x",
      "git -C /code/web checkout -b other",
      "git -C /code/web stash",
      "git -C /code/web fetch",
      "git -C /code/web branch new-one",
      "git -C /code/web branch --set-upstream-to=origin/main",
      "git -C /code/web branch --unset-upstream",
      "git -C /code/web branch --edit-description",
      "git -C /code/web tag v1",
      "git -C /code/web config --unset user.email",
      "git -C /code/web config --remove-section user",
      "git -C /code/web diff --output=/code/web/report.patch",
      "git diff --output /code/web/report.patch",
      "git format-patch -3 --output-directory=/code/web/patches",
      "git --work-tree=/code/web add -A",
      "git --work-tree /code/web add -A",
      "git --git-dir /code/web/.git commit -m x",
      "git --work-tree=/ws/feat/web --git-dir=/code/web/.git checkout .",
      "git --git-dir=/code/web/.git --work-tree=/ws/feat/web checkout .",
      "git --git-dir=/ws/feat/web/.git --work-tree=/code/web checkout .",
      "git config --file /code/web/.git/config user.email x",
      "cd /code/web && git config -f .git/config --unset user.email",
      "git clone https://example.com/x.git /code/web/vendor/x",
      "cd /code/web && git clone https://example.com/x.git",
      "git init --separate-git-dir=/tmp/repo.git /code/web",
      "cd /code/web && git init",
    ],
    true,
  );

  table(
    "refuses through cd, groups, wrappers and nested commands",
    [
      "cd /code/web && git commit -am x",
      "cd /code/web; echo x > f",
      "(cd /code/api && git reset --hard)",
      "pushd /code/web && git commit -am x",
      "(cd /code/web && git log) && cd /code/web && git commit -m x",
      "{ cd /code/web; touch x; }",
      "{ cd /code/web; } && touch x",
      "{ cd /code/web; ls; }; touch x",
      "cd /code/web && cd /tmp && cd - && touch x",
      "cd /code/web; (cd /tmp && cd - && touch x)",
      "env -C /code/web make",
      "env --chdir=/code/web npm test",
      "sudo -D /code/web make",
      "sudo rm /code/web/a",
      "nohup make -C /code/web &",
      "timeout 60 touch /code/web/x",
      "env -S 'rm -rf /code/web/build'",
      "watch -n 5 'touch /code/web/x'",
      "bash -c 'echo y > /code/web/z'",
      "bash -c 'echo hi' > /code/web/out",
      "sh -lc 'cd /code/web && make'",
      "eval 'rm /code/web/x'",
      'echo "$(rm -rf /code/web/build)"',
      'echo "`touch /code/web/a`"',
      "echo $(touch /code/web/a)",
      'X="$(cd /code/web && git commit -am x)"',
      "diff <(ls) >(tee /code/web/x)",
      "find /code/web -name '*.log' -delete",
      "find -L /code/web -name '*.tmp' -delete",
      "cd /code/web && find -name '*.log' -delete",
      "find /code/web -name x -exec rm {} ;",
      "find /code/web -name x -exec touch {} +",
      "find /code/web -name '*.ts' -execdir chmod +x {} ;",
    ],
    true,
  );

  table(
    "refuses a change with values only known as the line runs, when it names a clone",
    [
      "for f in /code/web/*.log; do rm \"$f\"; done",
      "find /code/web -name '*.log' | xargs rm",
      "cat /code/web/list | xargs -I{} touch {}",
      "ls /code/web | while read f; do rm \"$f\"; done",
      "D=/code/web; rm -rf $D/build",
      "export D=/code/web && make -C $D",
      "cat /code/web/x; sh -c \"$CMD\"",
      "cat /code/web/x | bash",
    ],
    true,
  );

  table(
    "refuses code and settings that point a program at a clone",
    [
      "python3 - <<'EOF'\nopen('/code/web/x.txt', 'w').write('x')\nEOF",
      "node - <<'JS'\nrequire('fs').writeFileSync('/code/web/a', '')\nJS",
      "bash <<'SH'\ncd /code/web && git pull\nSH",
      "cat <<'SH' | bash\nrm -rf /code/web/build\nSH",
      "bash <<< 'rm -rf /code/web/build'",
      "CARGO_TARGET_DIR=/code/web/target cargo build",
      "export CARGO_TARGET_DIR=/code/web/target && cargo build",
      "D=/code/web/target; export D; cargo build",
      "GIT_DIR=/code/web/.git git commit -am x",
      "env GIT_DIR=/code/web/.git git commit -m x",
      "DESTDIR=/code/web make install",
      "env -S\"rm /code/web/x\"",
    ],
    true,
  );

  table(
    "refuses what the shell expands to a clone",
    [
      'cd "${WEB:-/code/web}" && git pull',
      'touch "${D:-/code/web}/x"',
      'echo "${x:-{}"; touch /code/web/x',
      'echo ${x:-"}"}; touch /code/web/x',
      'FILES=(/code/web/dist /code/api/dist); rm -rf "${FILES[@]}"',
      "arr=($(touch /code/web/x))",
      "bash -c 'rm -rf \"$1\"' _ /code/web/build",
      "sh -c 'cd \"$1\" && git pull' sh /code/web",
      "echo x | tee {/code/web,/code/api}/.nvmrc",
      "echo $(( $(touch /code/web/x) ))",
      "echo @(foo|$(touch /code/web/x))",
      "echo 2>(tee /code/web/x)",
      "((cd /code/web && git pull))",
    ],
    true,
  );

  table(
    "refuses programs that write after all",
    [
      "cp -rt /code/web src/",
      "install -Dt /code/web/bin tool",
      "ln -st /code/web/node_modules /tmp/x",
      "find /code/web -name '*.orig' | parallel rm",
      "sed -n 'w /code/web/out' f",
      "sed 's/a/b/w /code/web/changed' f",
      "gawk -iinplace '{print}' /code/web/f",
      "tree -o /code/web/t.txt /tmp",
      "xxd -r /tmp/dump /code/web/bin",
      "tar xzf /tmp/a.tgz -C /code/web",
      "unzip /tmp/a.zip -d /code/web",
      "cd /code/web && git config edit",
      "git -C /code/web grep -O'sed -i s/a/b/' foo",
      "git -C /code/web format-patch -3",
      "sed 'e touch /code/web/x' /tmp/input",
      "sed '1w /code/web/out' /tmp/input",
      "sed -e'w /code/web/out' /tmp/input",
      "sed -ne 'w /code/web/out' /tmp/input",
      "sed -n -e p -e 'w /code/web/out' /tmp/input",
      "sed -n '1,5w /code/web/out' /tmp/input",
      "sed '$W /code/web/out' /tmp/input",
      "sed '/x/w /code/web/out' /tmp/input",
      "sed -f /tmp/script.sed /code/web/a",
      'awk "$AWK_PROGRAM" /code/web/file',
      'sed "$SCRIPT" /code/web/f',
      "sort $OPTIONS /code/web/a",
      "sort --compress-program=/code/web/script big.txt",
      "sort --compress-program /code/web/script big.txt",
      "tar -I /code/web/compress -cf /tmp/a.tar src",
      "tar -xf /tmp/a.tar --to-command=/code/web/hook",
      "HOME=/code/web; touch \"$HOME/new-file\"",
      "cd /code/web && eval \"$X\"",
      "cd /code/web && bash -c \"$X\"",
      "sed -n '1e touch /code/web/x' /tmp/input",
      "sed 's/x/touch \\/code\\/web\\/y/e' /tmp/input",
      "sed 's|x|touch /code/web/y|e' /tmp/input",
      "tar -czf /code/web/a.tgz src",
      'tar -xf /code/web/archive.tar -C "$DEST"',
      "tar -cf/code/web/a.tar src",
      "tar czf /code/web/a.tgz src",
      "python3 -c \"print(open('/code/web/a').read())\"",
      "make -n -C /code/web",
    ],
    true,
  );

  table(
    "allows what never names a clone",
    [
      "npm test",
      "cd web && npm run build 2>&1 | tail -20",
      "echo hi > web/notes.txt",
      "rm -rf /ws/feat/web/build",
      "git -C web commit -m x",
      "git commit -m \"$(cat <<'EOF'\nfix: keep out of /code/web\n\nrm -rf /code/web is refused\nEOF\n)\"",
      "gh pr create --title x --body \"$(cat <<'EOF'\nrm -rf /code/web\nEOF\n)\"",
      "cat > /tmp/notes.md <<'EOF'\nrm -rf /code/web\nEOF",
      "python3 - <<'EOF'\nprint(1)\nEOF",
      "for f in src/*.ts; do echo $f; done",
      "rm -rf $TMPDIR/x",
      "find . -name '*.log' -delete",
      "xargs rm < files.txt",
      "export PATH=$HOME/.bun/bin:$PATH && bun test",
      "ls ~ && make",
      "echo 'a; cd /code/web; b' && touch x",
      "echo '$(rm -rf /code/web/build)'",
      "echo '\"$(rm -rf /code/web/build)\"'",
      "cd /code/web | true; touch x",
      "cd /code/web && cd - && touch x",
      "( cd /code/web; ls ) && { cd web; touch x; }",
      "cd /code/web && cd /ws/feat/web && npm test",
      "cd /code/web",
      "git clone https://example.com/x.git /tmp/x",
      "cd /code/web && git --git-dir=/ws/feat/web/.git --work-tree=/ws/feat/web add .",
      "cd /code/web && git config --file /tmp/c user.email x",
    ],
    false,
  );

  table(
    "allows reading a clone and copying out of it",
    [
      "cat /code/web/README.md",
      "grep -r TODO /code/web",
      "grep '>' /code/web/f",
      'grep ">" /code/web/f',
      'grep -rn "->" /code/web/src',
      "rg '=>' /code/web",
      "ls /code/web",
      "echo /code/web",
      'echo "a > /code/web/f"',
      "[[ -d /code/web ]] && echo yes",
      "[ -d /code/web ] && echo yes",
      "test -f /code/web/x",
      "awk '{print $1}' /code/web/f",
      "sort /code/web/a",
      "cat /code/web/x > web/x",
      "diff /code/web/a web/a > /tmp/out",
      "diff <(git -C /code/web show HEAD:a) web/a",
      "cp /code/web/a ./web/a",
      "cp /code/web/.env web/.env",
      "cp -r /code/web/. /tmp/copy",
      "cp -t /ws/feat/web /code/web/a.ts",
      "rsync -a /code/web/dist/ web/dist/",
      "rsync -t /code/web/a.ts web/a.ts",
      "ln -s /code/web/node_modules web/node_modules",
      "for f in /code/web/*.env; do cp \"$f\" web/; done",
      "find /code/web -name '*.env' | xargs -I{} cp {} web/",
      "find /code/web -name '*.ts' | xargs grep foo",
      "find /code/web -name '*.ts' -exec grep -l x {} ;",
      "git -C /code/web status",
      "git -C /code/web log --oneline -5",
      "git -C /code/web log | head",
      "git -C /code/web diff --output=/tmp/report.patch",
      "git config --file /code/web/.git/config user.email",
      "(cd /code/web && git log -1) && git commit -m x",
      "BASE=$(cd /code/web && git rev-parse HEAD); git commit -m x",
      'echo "$(git -C /code/web rev-parse HEAD)"',
      "awk '$3 > 5' /code/web/data.txt",
      "awk 'length($0) > 120' /code/web/src/a.ts",
      "sed -n '1,40p' /code/web/src/a.ts",
      "sed 's/one/two/g' /code/web/a.ts",
      "sed -n '/warn/p' /code/web/log",
      "sed -ne '/warn/p' -e 's/a/b/' /code/web/log",
      'ls /code/web | while read f; do cat "$f"; done',
      'grep "$PATTERN" /code/web/f',
      "awk '{print $1}' /code/web/*.log",
      "tar -czf /tmp/a.tgz -C /code/web src",
      "tar -tzf /code/web/a.tgz",
      "sort --compress-program=gzip /code/web/big.txt",
      "tar -I zstd -cf /tmp/web.tar.zst -C /code/web src",
      "sed 's/a/b/' /code/web/a > web/a",
      "for f in .env .env.local; do cp /code/web/$f web/$f; done",
      "git -C /code/web diff --name-only | xargs -I{} cp /code/web/{} web/{}",
      'git -C /code/web diff --name-only | while read f; do cp "/code/web/$f" "web/$f"; done',
      "HEAD=$(git -C /code/web rev-parse HEAD) && git -C web reset --hard $HEAD",
      'BR=$(git -C /code/web branch --show-current); git -C web checkout -b "$BR-copy"',
      "git -C /code/web log --format=%H -1 | xargs git -C web cherry-pick",
      'gh pr comment 12 --body "see /code/web/src/a.ts"',
      "git -C /code/web archive HEAD | tar -x -C web",
      "git -C /code/web archive -o /tmp/web.tar HEAD",
      "git -C /code/web format-patch -3 -o /tmp/patches",
      "git -C /code/web format-patch -1 --stdout > /tmp/p.patch",
      "git -C /code/web bundle create /tmp/b --all",
      "git clone /code/web /tmp/x",
      "git -C web apply /code/web/x.patch",
      "git -C web fetch /code/web main",
      "tar -czf /tmp/web.tgz -C /code/web src",
      "tar -cf - -C /code/web src | tar -xf - -C web",
      "zip -r /tmp/web.zip /code/web/src",
      "ln -s /code/web/node_modules",
      "git -C /code/web symbolic-ref --short HEAD",
      "git -C /code/web fsck",
      "git -C /code/web lfs ls-files",
      "git -C /code/web submodule foreach --quiet 'echo $path'",
    ],
    false,
  );

  table(
    "refuses, each rule on its own",
    [
      "(cd /code/web && ls) | xargs rm",
      "cp -r dist /code",
      "find /code -name '*.log' -delete",
      "cat <<EOF\n$(touch /code/web/x)\nEOF",
      "export D=/code/web; cp -r src $D/",
      "f=/tmp/x; ls /code/web | while read f; do rm \"$f\"; done",
      "find /code/web -name Makefile -execdir make ;",
      "git -C /code/web remote add upstream https://example.com/x.git",
      "git -C /code/web remote set-url origin https://example.com/x.git",
      "rm -rf /code/w*",
      'echo "${x/$(touch /code/web/x)/y}"',
      "(cd /tmp && ls) > /code/web/out",
      "cp /code/web/a $DEST",
      "touch /code/web/..config",
      "git -C /code/web submodule foreach 'touch x'",
      "cd /code/web >| /tmp/log && touch x",
      "cd /code/web && make",
      "/code/web/node_modules/.bin/prettier --write src",
    ],
    true,
  );

  table(
    "allows, each rule on its own",
    [
      "timeout 5 cp /code/web/a web/a",
      "find /code/web -name '*.env' | xargs -I {} cp {} web/",
      "wc -l < /code/web/README.md",
      "touch x # see /code/web",
      "bash -c 'rm -rf \"$1\"' _ /tmp/x && cat /code/web/a",
      "unzip /code/web/a.zip -d /tmp/x",
    ],
    false,
  );

  test("a clone in the home folder is named by `~` and `$HOME` too", () => {
    const home = [{ clone: join(homedir(), "code-web"), worktree: "/ws/feat/web" }];
    expect(cloneCommandRefusal("touch ~/code-web/x", CWD, home)).toBeDefined();
    expect(cloneCommandRefusal("touch $HOME/code-web/x", CWD, home)).toBeDefined();
    expect(cloneCommandRefusal("touch ~/other/x", CWD, home)).toBeUndefined();
  });

  test("sourcing a file of assignments reads it; a file running commands is a program", () => {
    const root = mkdtempSync(join(tmpdir(), "clone-source-"));
    try {
      const clone = join(root, "web");
      mkdirSync(clone);
      writeFileSync(join(clone, ".env"), "# keys\nexport API_URL=http://localhost:3000\nTOKEN='a b'\nNAME=\"x\"\nBUILD_DIR=/tmp/chf20-build\n\n");
      writeFileSync(join(clone, ".env.run"), "API_URL=x\ntouch made-by-env\n");
      writeFileSync(join(clone, ".env.build"), `CARGO_TARGET_DIR="${clone}/target"\nOUT=${clone}/out\n`);
      const clones = [{ clone, worktree: join(root, "ws", "web") }];
      expect(cloneCommandRefusal(`source ${clone}/.env && npm run dev`, CWD, clones)).toBeUndefined();
      expect(cloneCommandRefusal(`set -a; . ${clone}/.env; set +a`, CWD, clones)).toBeUndefined();
      expect(cloneCommandRefusal(`source ${clone}/.env.run`, CWD, clones)).toContain("the user's own clone");
      expect(cloneCommandRefusal(`source ${clone}/missing.env`, CWD, clones)).toContain("the user's own clone");
      // Its variables count: one pointing at a clone sends a program there.
      expect(cloneCommandRefusal(`source ${clone}/.env.build && cargo build`, CWD, clones)).toBeDefined();
      expect(cloneCommandRefusal(`source ${clone}/.env.build; touch "$OUT"`, CWD, clones)).toBeDefined();
      expect(cloneCommandRefusal(`source ${clone}/.env; echo "$API_URL"`, CWD, clones)).toBeUndefined();
      expect(cloneCommandRefusal(`source ${clone}/.env; rm -rf "$BUILD_DIR"`, CWD, clones)).toBeUndefined();
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  test.if(process.platform === "darwin")("on macOS, a clone's path in another case is the clone", () => {
    expect(refuses("touch /Code/Web/x")).toBeDefined();
    expect(refuses("cd /CODE/web && npm install")).toBeDefined();
  });

  test("a line nested past what the stack holds is refused, not let through", () => {
    expect(refuses(`${"( ".repeat(100000)}ls${" )".repeat(100000)}`)).toContain("could not read this command line");
  });
});

describe("git from a workspace's worktree, which shares its clone's branches and settings", () => {
  const SHARED: SourceClone[] = [
    { clone: "/code/web", worktree: "/ws/feat/web", branch: "fix/typo", default_branch: "main" },
    { clone: "/code/api", worktree: "/ws/feat/api" },
  ];
  const refuses = (command: string, cwd = "/ws/feat/web") => cloneCommandRefusal(command, cwd, SHARED);
  const table = (title: string, commands: string[], refused: boolean) => {
    for (const command of commands) {
      test(`${title}: ${JSON.stringify(command)}`, () => {
        if (refused) expect(refuses(command)).toContain("git shares /code/web's branches and settings");
        else expect(refuses(command)).toBeUndefined();
      });
    }
  };

  table("refused", [
    // The clone's checked-out branch and its default branch.
    "git branch -D main",
    "git branch -d fix/typo",
    "git branch --delete other main",
    "git branch -m main old-main",
    "git branch -M feat/x main",
    "git branch -C feat/x main",
    "git branch -f main HEAD",
    "git branch --force fix/typo origin/fix/typo",
    "git branch -D refs/heads/main",
    'git branch -D "$B"',
    "git checkout -B main origin/main",
    "git switch -C fix/typo",
    "git switch --force-create main",
    "git fetch origin main:main",
    "git fetch origin +refs/heads/main:refs/heads/main",
    "git pull origin main:main",
    "git push . HEAD:main",
    "git -C /ws/feat/web branch -D main",
    // The repo's settings, remotes, refs, reflogs and worktrees.
    "git config user.email agent@example.com",
    "git config --unset core.hooksPath",
    "git config core.hooksPath .githooks",
    "git remote add fork git@github.com:me/web.git",
    "git remote set-url origin https://example.com/web.git",
    "git remote remove origin",
    "git remote rename origin upstream",
    "git update-ref refs/heads/feat/x HEAD",
    "git gc --prune=now",
    "git prune",
    "git worktree prune",
    "git worktree remove /code/web/.wt/other",
    "git reflog expire --expire=now --all",
    "git reflog delete HEAD@{1}",
    // Found by the red team: a switch onto the default branch, then work on it.
    "git checkout main && git pull",
    "git switch main && git merge feat/x",
    "git checkout --ignore-other-worktrees fix/typo",
    "git checkout main --",
    "git branch --merged | grep -v '\\*' | xargs git branch -d",
    "git stash clear",
    "git stash drop",
    "git rebase feat/x main",
    "git rebase -i origin/main main",
    "git rebase --onto origin/main feat/x main",
    "git branch -Df main",
    "git branch -fD main",
    "git branch -Mf feat/x main",
    "git checkout -fB main HEAD",
    "git switch -fC main feat/x",
    "git switch --force-create=main feat/x",
    'cp scripts/pre-commit "$(git rev-parse --git-path hooks)/pre-commit"',
    'git config --file "$(git rev-parse --git-common-dir)/config" user.email x',
    "git filter-branch --tree-filter 'rm -f secret' -- --all",
    "git push . --delete main",
    "git push . -d main",
    "git push ./ HEAD:main",
    'git push "$PWD" HEAD:main',
    "git push . :main",
    "git branch -u origin/x main",
    "git branch --unset-upstream main",
    "git branch --edit-description main",
    "git worktree add -B main ../x feat/x",
    "git worktree add ../x main",
    "git symbolic-ref refs/heads/main refs/heads/feat/x",
    "git symbolic-ref HEAD refs/heads/main",
    "git fetch origin main:heads/main",
    // Found by cubic.
    'git push . "$SPEC"',
    'git fetch origin "$SPEC"',
    "git checkout -Bmain origin/main",
    "git switch -Cfix/typo",
    "git worktree add -Bmain ../x feat/x",
    "git symbolic-ref --delete refs/remotes/origin/HEAD",
    "git notes add -m note main",
    "git notes remove HEAD",
    "git worktree lock /code/web/.wt/other",
    "git worktree repair",
    "git reflog drop",
    "git reflog write refs/heads/main main x",
    "git replace HEAD~1 HEAD",
    "git replace -d abc123",
    'git "$GIT_SUBCOMMAND" main',
    "git push -o ci.skip . HEAD:main",
    "git push --repo . HEAD:main",
    "git fetch origin 'refs/heads/*:refs/heads/*'",
    "git fetch origin '+refs/heads/ma*:refs/heads/ma*'",
    "git push . main",
    'git config --file "$CONFIG" user.email x',
  ], true);

  table("allowed", [
    "git commit -am 'fix'",
    "git checkout -b feat/new",
    "git switch -c feat/new main",
    "git branch feat/new",
    "git branch -D feat/new",
    "git branch -m feat/new feat/renamed",
    "git branch -f feat/new HEAD~1",
    "git branch -d -r origin/main",
    "git branch -u origin/feat/x",
    "git branch --set-upstream-to origin/main",
    "git rebase origin/main",
    "git rebase -i HEAD~3",
    "git reset --hard origin/main",
    "git merge main",
    "git push -u origin feat/new",
    "git push --force-with-lease origin HEAD:feat/new",
    "git push origin main:main",
    "git fetch origin",
    "git fetch origin main",
    "git fetch origin main:refs/remotes/origin/main",
    "git pull --rebase",
    "git stash",
    "git stash pop",
    "git tag v1.2.3",
    "git tag -d v1.2.3",
    "git config --get user.email",
    "git config user.email",
    "git config --global user.name 'Me'",
    "git config -f .gitmodules submodule.x.url https://example.com/x.git",
    "git remote -v",
    "git remote update",
    "git worktree list",
    "git worktree add ../other feat/other",
    "git reflog",
    "git log main..HEAD",
    "git notes list",
    "git notes show HEAD",
    "git replace -l",
    "git bisect start",
    "git bisect good",
    "git push -o ci.skip origin HEAD:main",
    "git fetch origin 'refs/heads/*:refs/remotes/origin/*'",
    "git push . feat/x",
    'git config --file "$CONFIG" --get user.email',
    "git checkout main -- README.md",
    "git checkout -- README.md",
    "git checkout -- main",
    "git switch -c feat/next main",
    "git switch --detach main",
    "git stash push -m wip",
    "git stash pop",
    "git stash apply stash@{0}",
    "git rebase main",
    "git rebase origin/main feat/x",
    "git branch -dr origin/feat/old",
    "git remote prune origin",
    "git fetch --prune",
    "git worktree add ../x -b feat/y main",
    "git worktree add --detach ../x main",
    'cat "$(git rev-parse --git-dir)/HEAD"',
    'cd "$(git rev-parse --show-toplevel)" && npm test',
  ], false);

  test("the refusal names what it protects, and what to do instead", () => {
    expect(refuses("git branch -D main")).toContain("deletes main, the default branch of /code/web");
    expect(refuses("git branch -D main")).toContain("git switch -c <name> <branch>");
    expect(refuses("git checkout main")).toContain("switches this worktree onto main, the default branch of /code/web");
    expect(refuses("git config user.email x")).toContain("git -c <key>=<value>");
    expect(refuses("git worktree prune")).toContain("ask the user to do it in their clone");
    expect(refuses("git push . :main")).toContain("deletes main");
    expect(refuses("git branch -f fix/typo HEAD")).toContain("resets fix/typo, the branch checked out in /code/web");
    expect(refuses("git fetch origin main:main")).toContain("moves main, the default branch of /code/web");
    expect(refuses("git config user.email x")).toContain("changes the settings of /code/web");
  });

  test("a clone whose branches are not known still keeps its settings, refs and worktrees", () => {
    expect(refuses("git branch -D main", "/ws/feat/api")).toBeUndefined();
    expect(refuses("git remote set-url origin x", "/ws/feat/api")).toContain("changes the remotes of /code/api");
  });

  test("outside the worktrees, git works on another repository", () => {
    expect(refuses("git branch -D main", "/tmp/elsewhere")).toBeUndefined();
    expect(refuses("git config user.email x", "/ws/feat")).toBeUndefined();
  });
});
