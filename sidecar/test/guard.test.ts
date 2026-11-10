import { describe, expect, test } from "bun:test";
import { runsChauffeurAddRepo, runsGitWorktreeAdd, splitCommands, WORKTREE_ADD_DENIAL } from "../src/guard/worktree-guard.ts";

describe("runsGitWorktreeAdd", () => {
  const blocked = [
    "git worktree add ../feature -b feature",
    "git worktree add ../x",
    "  git   worktree   add ../x",
    "/usr/bin/git worktree add ../x",
    "git -C /repo worktree add ../x",
    "git -C ../web worktree add /tmp/wt origin/staging",
    "git -c core.hooksPath=/dev/null worktree add ../x",
    "git --git-dir=/repo/.git --work-tree=/repo worktree add ../x",
    "git --no-pager -C repo worktree add ../x",
    "GIT_DIR=/repo/.git git worktree add ../x",
    "FOO=1 BAR='a b' git worktree add ../x",
    "env GIT_TRACE=1 git worktree add ../x",
    "sudo git worktree add ../x",
    "command git worktree add ../x",
    "cd repo && git worktree add ../x -b y",
    "git fetch origin; git worktree add ../x origin/main",
    "git worktree list || git worktree add ../x",
    "git status\ngit worktree add ../x",
    "(cd repo && git worktree add ../x)",
    "echo $(git worktree add ../x)",
    "echo `git worktree add ../x`",
    "{ git worktree add ../x; }",
    "if true; then git worktree add ../x; fi",
    "git worktree add ../x 2>&1 | tee log",
    "git worktree add ../x &",
    "bash -c 'git worktree add ../x'",
    "sh -lc \"cd repo && git worktree add ../x\"",
    "eval git worktree add ../x",
    "git worktree --verbose add ../x",
    'git \\\n  worktree add ../x',
  ];
  for (const command of blocked) {
    test(`blocks: ${JSON.stringify(command)}`, () => {
      expect(runsGitWorktreeAdd(command)).toBe(true);
    });
  }

  const allowed = [
    "git worktree list",
    "git worktree list --porcelain",
    "git worktree remove ../x",
    "git worktree remove --force ../x",
    "git worktree prune",
    "git worktree prune --dry-run && git worktree list",
    "git worktree move ../x ../y",
    "git worktree lock ../x",
    "git add .",
    "git add worktree",
    "git commit -m 'git worktree add is blocked'",
    'echo "git worktree add ../x"',
    "echo git worktree add",
    "grep -r 'worktree add' docs/",
    "git log --grep='worktree add'",
    "chauffeur worktree add --repo web --name fix",
    "rg \"git worktree add\" -n",
    "# git worktree add ../x",
    "ls ~/worktree add",
    "gitk worktree add",
    "bash -c 'git worktree list'",
    "git-worktree-helper add",
    "",
  ];
  for (const command of allowed) {
    test(`allows: ${JSON.stringify(command)}`, () => {
      expect(runsGitWorktreeAdd(command)).toBe(false);
    });
  }
});

describe("splitCommands", () => {
  test("splits on operators and keeps quoted words whole", () => {
    expect(splitCommands(`cd "my repo" && git commit -m 'a; b' | cat; echo done`)).toEqual([
      ["cd", "my repo"],
      ["git", "commit", "-m", "a; b"],
      ["cat"],
      ["echo", "done"],
    ]);
  });

  test("keeps a substitution's output as a word of the enclosing command", () => {
    expect(splitCommands("echo $(git rev-parse HEAD) done")).toEqual([["echo", "$(...)"], ["git", "rev-parse", "HEAD"], ["done"]]);
    expect(splitCommands("`cat cmd` -f")).toEqual([["$(...)"], ["cat", "cmd"], ["-f"]]);
  });

  test("does not split on redirections", () => {
    expect(splitCommands("make 2>&1 >out &>all")).toEqual([["make", "2>&1", ">out", "&>all"]]);
  });
});

test("the denial names the Chauffeur command to use", () => {
  expect(WORKTREE_ADD_DENIAL).toContain("chauffeur worktree add --repo <repo> [--name <name>]");
  expect(WORKTREE_ADD_DENIAL).toContain("tracks and cleans every worktree");
});

describe("runsChauffeurAddRepo", () => {
  for (const command of [
    "chauffeur add-repo deft-kestrel analytics",
    '/usr/local/bin/chauffeur add-repo "$CHAUFFEUR_WORKSPACE_ID" analytics',
    "cd web && chauffeur add-repo w analytics",
    "sh -c 'chauffeur add-repo w analytics'",
    "env FOO=1 chauffeur add-repo w analytics",
  ]) {
    test(`blocks: ${JSON.stringify(command)}`, () => {
      expect(runsChauffeurAddRepo(command)).toBe(true);
    });
  }
  for (const command of [
    "chauffeur repos",
    "chauffeur worktree add --repo analytics",
    "chauffeur new --parent w --repo add-repo",
    "echo chauffeur add-repo",
    "grep -r add-repo .",
  ]) {
    test(`allows: ${JSON.stringify(command)}`, () => {
      expect(runsChauffeurAddRepo(command)).toBe(false);
    });
  }
});
