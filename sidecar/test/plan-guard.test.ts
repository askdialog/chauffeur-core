import { afterAll, beforeEach, describe, expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { linkSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { designGuardDenial, planPathRefusal, planWriteRefusal } from "../src/guard/plan-guard.ts";

// A workspace laid out as the daemon makes it, in a temp folder: a repo checkout and the notes
// folder with its plans folder. Paths go through the temp folder as given (on macOS a symlink,
// /var → /private/var), as Claude Code's would.
const top = mkdtempSync(join(tmpdir(), "plan-guard-"));
let ws: string;
let plans: string;
let n = 0;

beforeEach(() => {
  ws = join(top, `ws-${++n}`);
  plans = join(ws, ".chauffeur", "plans");
  mkdirSync(plans, { recursive: true });
  mkdirSync(join(ws, "repo-a", "src"), { recursive: true });
  writeFileSync(join(ws, "repo-a", "README.md"), "readme\n");
  mkdirSync(join(ws, "elsewhere"));
});

afterAll(() => rmSync(top, { recursive: true, force: true }));

const refused = (path: unknown, cwd = ws) => planPathRefusal(path, plans, cwd);

describe("the plans folder", () => {
  test("a new plan, or an existing one, is accepted", () => {
    expect(refused(join(plans, "glowing-otter.md"))).toBeUndefined();
    writeFileSync(join(plans, "glowing-otter.md"), "# Plan\n");
    expect(refused(join(plans, "glowing-otter.md"))).toBeUndefined();
    expect(refused(join(plans, "glowing-otter-agent-a1b2.md"))).toBeUndefined();
  });

  test("an existing subfolder of the plans folder is accepted, a missing one refused", () => {
    mkdirSync(join(plans, "drafts"));
    expect(refused(join(plans, "drafts", "a.md"))).toBeUndefined();
    expect(refused(join(plans, "missing", "a.md"))).toContain("does not exist");
  });

  test("relative paths resolve from the session's folder", () => {
    expect(refused(".chauffeur/plans/a.md")).toBeUndefined();
    expect(refused("plans/a.md", join(ws, ".chauffeur"))).toBeUndefined();
    expect(refused("a.md", plans)).toBeUndefined();
    expect(refused("repo-a/README.md")).toContain("outside the plans folder");
    expect(refused("notes.md")).toContain("outside the plans folder");
  });

  test("`./` segments and doubled slashes are only spelling", () => {
    expect(refused(`${ws}/./.chauffeur/./plans/./a.md`)).toBeUndefined();
    expect(refused(`${ws}//.chauffeur//plans//a.md`)).toBeUndefined();
    expect(refused("./.chauffeur/plans/a.md")).toBeUndefined();
  });

  test("the real path of the temp folder names the same plans folder", () => {
    expect(refused(join(realpathSync(plans), "a.md"))).toBeUndefined();
  });
});

describe("refused", () => {
  test("`..` anywhere, even when it would land back inside", () => {
    expect(refused(`${plans}/../plans/a.md`)).toContain("`..`");
    expect(refused(`${plans}/../../repo-a/README.md`)).toContain("`..`");
    expect(refused("../outside.md", plans)).toContain("`..`");
    expect(refused(".chauffeur/plans/..")).toContain("`..`");
  });

  test("absolute paths elsewhere: the repos, the workspace root, the notes folder, ~/.claude/plans", () => {
    expect(refused(join(ws, "repo-a", "README.md"))).toContain("outside the plans folder");
    expect(refused(join(ws, "repo-a", "src", "new.ts"))).toContain("outside the plans folder");
    expect(refused(join(ws, "CLAUDE.md"))).toContain("outside the plans folder");
    expect(refused(join(ws, ".chauffeur", "tickets.md"))).toContain("outside the plans folder");
    expect(refused(join(homedir(), ".claude", "plans", "glowing-otter.md"))).toBeDefined();
    expect(refused("~/.claude/plans/glowing-otter.md")).toContain("home folder");
    expect(refused("/etc/hosts")).toContain("outside the plans folder");
    expect(refused("/tmp/x.md")).toContain("outside the plans folder");
  });

  test("the plans folder itself, or a trailing slash", () => {
    expect(refused(plans)).toContain("outside the plans folder");
    expect(refused(`${plans}/`)).toContain("names a folder");
    expect(refused(`${plans}/a.md/`)).toContain("names a folder");
    expect(refused(`${plans}/.`)).toContain("names a folder");
  });

  test("a subfolder that is a symlink to elsewhere", () => {
    symlinkSync(join(ws, "repo-a"), join(plans, "linked"));
    expect(refused(join(plans, "linked", "README.md"))).toContain("outside the plans folder");
    expect(refused(join(plans, "linked", "new.md"))).toContain("outside the plans folder");
    expect(readFileSync(join(ws, "repo-a", "README.md"), "utf8")).toBe("readme\n");
  });

  test("a symlinked parent named outside that leads elsewhere", () => {
    symlinkSync(join(ws, "elsewhere"), join(ws, "repo-a", "out"));
    expect(refused(join(ws, "repo-a", "out", "a.md"))).toContain("outside the plans folder");
  });

  test("a target that is a symlink, even to a file of the plans folder", () => {
    symlinkSync(join(ws, "repo-a", "README.md"), join(plans, "evil.md"));
    expect(refused(join(plans, "evil.md"))).toContain("symbolic link");
    writeFileSync(join(plans, "real.md"), "x");
    symlinkSync(join(plans, "real.md"), join(plans, "alias.md"));
    expect(refused(join(plans, "alias.md"))).toContain("symbolic link");
    symlinkSync(join(ws, "missing-target.md"), join(plans, "dangling.md"));
    expect(refused(join(plans, "dangling.md"))).toContain("symbolic link");
  });

  test("a hard link to a file of a repo", () => {
    linkSync(join(ws, "repo-a", "README.md"), join(plans, "hard.md"));
    expect(refused(join(plans, "hard.md"))).toContain("hard link");
  });

  test("special files: a folder, a FIFO", () => {
    mkdirSync(join(plans, "dir.md"));
    expect(refused(join(plans, "dir.md"))).toContain("not a regular file");
    execFileSync("mkfifo", [join(plans, "pipe.md")]);
    expect(refused(join(plans, "pipe.md"))).toContain("not a regular file");
  });

  test("a plans folder that is a symlink, or held by a symlinked notes folder", () => {
    rmSync(plans, { recursive: true });
    symlinkSync(join(ws, "repo-a"), plans);
    expect(refused(join(plans, "a.md"))).toContain("symbolic link");
    expect(refused(join(plans, "README.md"))).toContain("symbolic link");

    const other = join(top, `ws-${++n}`);
    mkdirSync(join(other, "real-notes", "plans"), { recursive: true });
    symlinkSync(join(other, "real-notes"), join(other, ".chauffeur"));
    expect(planPathRefusal(join(other, ".chauffeur", "plans", "a.md"), join(other, ".chauffeur", "plans"), other)).toContain(
      "symbolic link",
    );
  });

  test("a missing plans folder, or one not given as an absolute path", () => {
    rmSync(plans, { recursive: true });
    expect(refused(join(plans, "a.md"))).toContain("missing");
    expect(planPathRefusal("a.md", "relative/plans", ws)).toContain("unknown");
  });

  test("odd inputs", () => {
    expect(refused(undefined)).toBe("no file path");
    expect(refused(42)).toBe("no file path");
    expect(refused("   ")).toBe("no file path");
    expect(refused(`${plans}/a\0.md`)).toContain("NUL");
    expect(refused(`${plans}\\a.md`)).toContain("backslash");
  });

  test("case differences resolve to the real folder: the plans folder stays itself, a repo stays outside", () => {
    // On a case-insensitive volume (macOS by default) the spelling names the same folder.
    const upper = join(ws, ".Chauffeur", "PLANS", "a.md");
    const result = refused(upper);
    let caseInsensitive = false;
    try {
      realpathSync(join(ws, ".Chauffeur"));
      caseInsensitive = true;
    } catch {}
    if (caseInsensitive) expect(result).toBeUndefined();
    else expect(result).toContain("does not exist");
    expect(refused(join(ws, "REPO-A", "README.md"))).toBeDefined();
  });
});

describe("tools", () => {
  test("Write, Edit and MultiEdit are checked by path, NotebookEdit refused, others let through", () => {
    const inside = { file_path: join(plans, "a.md") };
    const outside = { file_path: join(ws, "repo-a", "README.md") };
    for (const tool of ["Write", "Edit", "MultiEdit"]) {
      expect(planWriteRefusal(tool, inside, plans, ws)).toBeUndefined();
      expect(planWriteRefusal(tool, outside, plans, ws)).toContain("outside the plans folder");
    }
    expect(planWriteRefusal("NotebookEdit", { notebook_path: join(plans, "a.ipynb") }, plans, ws)).toContain("NotebookEdit");
    expect(planWriteRefusal("Read", outside, plans, ws)).toBeUndefined();
    expect(planWriteRefusal("Bash", { command: "echo hi > README.md" }, plans, ws)).toBeUndefined();
    expect(planWriteRefusal("Write", null, plans, ws)).toBe("no file path");
  });
});

describe("a design chat's folder", () => {
  // `.chauffeur/designs/<chat>`, the session's own folder; the repos beside the notes.
  let design: string;
  beforeEach(() => {
    design = join(ws, ".chauffeur", "designs", "chat-1");
    mkdirSync(design, { recursive: true });
  });
  const designRefused = (path: unknown, cwd = design) => planPathRefusal(path, design, cwd, "design");

  test("its screens, shared CSS and canvas.json are accepted, absolute or relative to it", () => {
    expect(designRefused(join(design, "checkout.html"))).toBeUndefined();
    expect(designRefused("canvas.json")).toBeUndefined();
    writeFileSync(join(design, "styles.css"), "body {}\n");
    expect(designRefused("./styles.css")).toBeUndefined();
    mkdirSync(join(design, "mobile"));
    expect(designRefused("mobile/cart.html")).toBeUndefined();
  });

  test("a repo, the notes root, another chat's designs or the plans are outside it", () => {
    expect(designRefused(join(ws, "repo-a", "README.md"))).toContain("outside the design folder");
    expect(designRefused(join(ws, "repo-a", "src", "Cart.tsx"))).toContain("outside the design folder");
    expect(designRefused(join(ws, ".chauffeur", "tickets.md"))).toContain("outside the design folder");
    expect(designRefused(join(plans, "a.md"))).toContain("outside the design folder");
    mkdirSync(join(ws, ".chauffeur", "designs", "chat-2"));
    expect(designRefused(join(ws, ".chauffeur", "designs", "chat-2", "a.html"))).toContain("outside the design folder");
    expect(designRefused("../chat-2/a.html")).toContain("`..`");
    expect(designRefused(join(ws, "CLAUDE.md"), ws)).toContain("outside the design folder");
  });

  test("the same refusals as a plan: symlinks, hard links, special files, a symlinked folder", () => {
    symlinkSync(join(ws, "repo-a", "README.md"), join(design, "evil.html"));
    expect(designRefused(join(design, "evil.html"))).toContain("symbolic link");
    linkSync(join(ws, "repo-a", "README.md"), join(design, "hard.html"));
    expect(designRefused(join(design, "hard.html"))).toContain("hard link");
    symlinkSync(join(ws, "repo-a"), join(design, "linked"));
    expect(designRefused(join(design, "linked", "new.html"))).toContain("outside the design folder");
    execFileSync("mkfifo", [join(design, "pipe.html")]);
    expect(designRefused(join(design, "pipe.html"))).toContain("not a regular file");

    const other = join(ws, ".chauffeur", "designs", "chat-3");
    symlinkSync(join(ws, "repo-a"), other);
    expect(planPathRefusal(join(other, "a.html"), other, other, "design")).toContain("symbolic link");
  });

  test("NotebookEdit is refused as in a design chat, other tools let through", () => {
    expect(planWriteRefusal("NotebookEdit", { notebook_path: join(design, "a.ipynb") }, design, design, "design")).toBe(
      "NotebookEdit is not available in a design chat",
    );
    expect(planWriteRefusal("Write", { file_path: join(design, "a.html") }, design, design, "design")).toBeUndefined();
    expect(planWriteRefusal("Edit", { file_path: join(ws, "repo-a", "README.md") }, design, design, "design")).toContain(
      "outside the design folder",
    );
    expect(planWriteRefusal("Read", { file_path: join(ws, "repo-a", "README.md") }, design, design, "design")).toBeUndefined();
  });

  test("the denial says it is a design chat, names the folder and where the design goes", () => {
    expect(designGuardDenial("/w/.chauffeur/designs/c1", "/w/repo/a.ts is outside the design folder")).toBe(
      "This is a Chauffeur design chat: you design screens, you never change code. The only files you may write " +
        "are your screens and canvas.json, in /w/.chauffeur/designs/c1/. Refused: /w/repo/a.ts is outside the design " +
        "folder. When the design is ready, the user sends it to a coding chat that implements it.",
    );
  });
});
