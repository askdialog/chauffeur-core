//! The dry run of the import from Claude Code against real git clones and a Claude Code folder of
//! transcripts as Claude Code writes them.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use chauffeur_protocol::claudecode::{
    ClaudeCodeBranch, ClaudeCodeImportParams, ClaudeCodeImportResult, ClaudeCodeOutcome,
    ClaudeCodeRelease, ClaudeCodeWorkspaceKind,
};
use chauffeurd::Daemon;
use chauffeurd::paths::Paths;

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str, text: &str) -> String {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", file]);
    git(dir, &["rev-parse", "HEAD"])
}

/// A transcript line of Claude Code: `cwd`, `gitBranch` and time, with `extra` fields.
fn line(cwd: &Path, branch: &str, at: &str, extra: &str) -> String {
    format!(
        r#"{{"type":"assistant","cwd":"{}","gitBranch":"{branch}","entrypoint":"claude-vscode","timestamp":"{at}",{extra}}}"#,
        cwd.display()
    )
}

/// A repo `name` with `files` beyond its README on `main`: a bare origin under `root/origin`, and
/// a clone of it under `root/clones` whose origin reads `github.com/fixture/<name>`, fetched from
/// the bare one.
fn clone_repo(root: &Path, name: &str, files: &[&str]) -> PathBuf {
    let seed = root.join("seed").join(name);
    std::fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "-q", "-b", "main"]);
    commit(&seed, "README.md", &format!("{name}\n"));
    for file in files {
        commit(&seed, file, "source\n");
    }
    let origin = root.join("origin").join(format!("{name}.git"));
    let path = |p: &Path| p.to_string_lossy().into_owned();
    git(
        root,
        &["clone", "-q", "--bare", &path(&seed), &path(&origin)],
    );
    let clone = root.join("clones").join(name);
    git(root, &["clone", "-q", &path(&origin), &path(&clone)]);
    let url = format!("https://github.com/fixture/{name}.git");
    git(&clone, &["remote", "set-url", "origin", &url]);
    git(
        &clone,
        &[
            "config",
            &format!("url.{}.insteadOf", origin.display()),
            &url,
        ],
    );
    clone
}

fn edit(path: &Path) -> String {
    format!(
        r#""message":{{"content":[{{"type":"tool_use","name":"Edit","input":{{"file_path":"{}"}}}}]}}"#,
        path.display()
    )
}

fn bash(command: &str) -> String {
    format!(
        r#""message":{{"content":[{{"type":"tool_use","name":"Bash","input":{{"command":"{command}"}}}}]}}"#
    )
}

fn ago(days: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::days(days))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    claude: PathBuf,
    daemon: Arc<Daemon>,
}

/// Clones `web` and `api` (origin `github.com/fixture/<name>`, fetched from a local bare repo) and
/// a Claude Code folder whose sessions wrote on them:
/// - `simple-web` and `simple-api`: `feat/live` (in both repos, not merged);
/// - `orchestrator`: `feat/child` (created by its command) and `feat/held` (merged, but its
///   worktree holds uncommitted changes), while `web` holds changes on `main`;
/// - `history`: `feat/merged` (in `main`), `feat/squashed` (a merged PR's head), `feat/gone`
///   (deleted), and a folder removed since;
/// - `running` (its process lives), `sdk` (Chauffeur's own, on the Agent SDK), `notes` (no git
///   repo), and `old` (90 days ago).
fn fixture() -> Fixture {
    fixture_with(|_| String::new())
}

/// The fixture, its settings ending with `extra(root)`.
fn fixture_with(extra: impl Fn(&Path) -> String) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for name in ["web", "api", "docs"] {
        let clone = clone_repo(&root, name, &[]);
        git(&clone, &["switch", "-q", "-c", "feat/live"]);
        commit(&clone, "live.txt", "live\n");
        git(&clone, &["switch", "-q", "main"]);
    }
    let web = root.join("clones/web");
    let api = root.join("clones/api");
    let branch_at = |name: &str, file: &str| {
        git(&web, &["switch", "-q", "-c", name, "main"]);
        let tip = commit(&web, file, "x\n");
        git(&web, &["switch", "-q", "main"]);
        tip
    };
    branch_at("feat/merged", "merged.txt");
    git(&web, &["merge", "-q", "--ff-only", "feat/merged"]);
    git(&web, &["push", "-q", "origin", "main"]);
    let squashed = branch_at("feat/squashed", "squashed.txt");
    // A branch adding a folder `main` does not have.
    git(&web, &["switch", "-q", "-c", "feat/away", "main"]);
    std::fs::create_dir_all(web.join("away")).unwrap();
    commit(&web, "away/x.ts", "away\n");
    git(&web, &["switch", "-q", "main"]);
    branch_at("feat/child", "child.txt");
    git(&web, &["branch", "feat/held", "main"]);
    let held = root.join("wt/held");
    git(
        &web,
        &["worktree", "add", "-q", held.to_str().unwrap(), "feat/held"],
    );
    std::fs::write(held.join("README.md"), "held, uncommitted\n").unwrap();
    std::fs::create_dir_all(web.join("src")).unwrap();
    std::fs::create_dir_all(root.join("notes")).unwrap();
    // A clone whose origin is no GitHub repo, and a folder of Chauffeur's own workspaces.
    let local = root.join("clones/local");
    let web_origin = root.join("origin/web.git");
    git(
        &root,
        &[
            "clone",
            "-q",
            web_origin.to_str().unwrap(),
            local.to_str().unwrap(),
        ],
    );
    let inside = root.join("workspaces/chauffeur-ws");
    std::fs::create_dir_all(&inside).unwrap();
    // A PR from feat/released merged into a release branch, not into main.
    let released = branch_at("feat/released", "released.txt");
    // Squash-merged long ago: past the merged PRs the list returns, found by its branch.
    let old_squash = branch_at("feat/old-squash", "old-squash.txt");
    // Worktrees inside the clone: .wt/y deleted by hand (git still lists it), .wt/z removed.
    branch_at("feat/y", "y.txt");
    branch_at("feat/z", "z.txt");
    for name in ["y", "z"] {
        let at = web.join(".wt").join(name);
        git(
            &web,
            &[
                "worktree",
                "add",
                "-q",
                at.to_str().unwrap(),
                &format!("feat/{name}"),
            ],
        );
    }
    std::fs::remove_dir_all(web.join(".wt/y")).unwrap();
    git(
        &web,
        &["worktree", "remove", web.join(".wt/z").to_str().unwrap()],
    );
    // A branch named like a tag.
    branch_at("v1", "v1.txt");
    git(&web, &["tag", "v1", "main"]);
    // A branch already in one of Chauffeur's workspaces, its worktree holding changes there.
    branch_at("feat/chauffeur", "chauffeur.txt");
    let chauffeur_wt = root.join("workspaces/ws-web/web");
    git(
        &web,
        &[
            "worktree",
            "add",
            "-q",
            chauffeur_wt.to_str().unwrap(),
            "feat/chauffeur",
        ],
    );
    std::fs::write(chauffeur_wt.join("README.md"), "changed in Chauffeur\n").unwrap();
    // A second clone of web, on its own live branch.
    let web2 = root.join("clones/web-2");
    git(
        &root,
        &[
            "clone",
            "-q",
            web_origin.to_str().unwrap(),
            web2.to_str().unwrap(),
        ],
    );
    git(
        &web2,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/fixture/web.git",
        ],
    );
    git(&web2, &["switch", "-q", "-c", "feat/two"]);
    commit(&web2, "two.txt", "two\n");
    // The second clone has feat/chauffeur too, the branch a Chauffeur workspace made from web holds.
    git(&web2, &["branch", "feat/chauffeur"]);
    // api's origin/HEAD names a branch its remote no longer has (renamed since).
    git(
        &api,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/master",
        ],
    );
    // The clones, named through a symlinked folder.
    std::os::unix::fs::symlink(root.join("clones"), root.join("alias")).unwrap();
    // web's own changes on main, made once the branches above are cut (they would carry them).
    // docs, touched on its default branch only, holds changes there too.
    std::fs::write(
        root.join("clones/docs/README.md"),
        "docs, uncommitted on main\n",
    )
    .unwrap();
    // A scratch worktree outside the clone, deleted since by hand: git still lists it.
    branch_at("feat/ext", "ext.txt");
    let ext = root.join("scratch/ext");
    git(
        &web,
        &["worktree", "add", "-q", ext.to_str().unwrap(), "feat/ext"],
    );
    std::fs::remove_dir_all(root.join("scratch")).unwrap();
    std::fs::write(web.join("README.md"), "web, uncommitted on main\n").unwrap();

    // A gh that knows one merged PR, from feat/squashed at its tip.
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let gh = bin.join("gh");
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\ncase \"$PWD\" in */api) exit 1 ;; esac\ncase \"$*\" in\n  *--head\\ feat/old-squash*) echo '[{{\"headRefOid\":\"{old_squash}\",\"baseRefName\":\"main\"}}]' ;;\n  *--head*) echo '[]' ;;\n  *--state\\ merged*) printf '['; i=0; while [ $i -lt 1000 ]; do printf '{{\"headRefName\":\"other-%s\",\"headRefOid\":\"x\",\"baseRefName\":\"main\"}},' $i; i=$((i+1)); done; echo '{{\"headRefName\":\"feat/squashed\",\"headRefOid\":\"{squashed}\",\"baseRefName\":\"main\"}},{{\"headRefName\":\"feat/released\",\"headRefOid\":\"{released}\",\"baseRefName\":\"release\"}}]' ;;\n  *) exit 1 ;;\nesac\n"
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();

    let claude = root.join("claude");
    let projects = claude.join("projects").join("-code");
    std::fs::create_dir_all(&projects).unwrap();
    let session = |id: &str, title: &str, lines: Vec<String>| {
        let mut text = format!("{{\"type\":\"ai-title\",\"aiTitle\":\"{title}\"}}\n");
        for l in lines {
            text.push_str(&l);
            text.push('\n');
        }
        std::fs::write(projects.join(format!("{id}.jsonl")), text).unwrap();
    };
    session(
        "simple-web",
        "Live work",
        vec![line(
            &web,
            "feat/live",
            &ago(6),
            &edit(&web.join("src/a.ts")),
        )],
    );
    session(
        "simple-api",
        "Live work on api",
        vec![
            line(&api, "feat/live", &ago(4), &edit(&api.join("live.txt"))),
            line(&api, "main", &ago(4), &edit(&api.join("README.md"))),
        ],
    );
    session(
        "away",
        "Away work",
        vec![line(
            &root.join("alias/web"),
            "feat/away",
            &ago(7),
            &edit(&root.join("alias/web/away/x.ts")),
        )],
    );
    session(
        "no-github",
        "Local only",
        vec![line(
            &local,
            "main",
            &ago(8),
            &edit(&local.join("README.md")),
        )],
    );
    session(
        "inside",
        "In a Chauffeur workspace",
        vec![line(&inside, "main", &ago(2), &edit(&inside.join("x.ts")))],
    );
    session(
        "orchestrator",
        "Orchestrate the release",
        vec![
            line(
                &web,
                "main",
                &ago(3),
                &bash(&format!(
                    "git -C {} branch feat/child-x; git checkout -b feat/child",
                    web.display()
                )),
            ),
            line(&web, "main", &ago(3), &edit(&held.join("README.md"))),
        ],
    );
    session(
        "history",
        "Old work",
        vec![
            line(&web, "feat/merged", &ago(10), &edit(&web.join("README.md"))),
            line(
                &web,
                "feat/squashed",
                &ago(10),
                &edit(&web.join("squashed.txt")),
            ),
            line(&web, "main", &ago(10), &edit(&web.join("README.md"))),
            line(&web, "main", &ago(10), &bash("git switch -c feat/gone")),
            line(
                &web,
                "feat/old-squash",
                &ago(10),
                &edit(&web.join("old-squash.txt")),
            ),
            // A folder that is not there: no repo, not its parent's.
            line(
                &web,
                "main",
                &ago(10),
                &bash(&format!(
                    "git -C {}/gone switch -c feat/nowhere",
                    api.display()
                )),
            ),
            line(
                &web,
                "main",
                &ago(10),
                &edit(&root.join("tmp/scratch/x.ts")),
            ),
            line(
                &web,
                "main",
                &ago(10),
                &edit(&root.join("clones/docs/README.md")),
            ),
        ],
    );
    session(
        "running",
        "Still open",
        vec![line(&web, "main", &ago(1), &edit(&web.join("src/b.ts")))],
    );
    session(
        "sdk",
        "A Chauffeur chat",
        vec![
            line(&web, "feat/live", &ago(1), &edit(&web.join("src/c.ts")))
                .replace("claude-vscode", "sdk-ts"),
        ],
    );
    session(
        "notes",
        "Notes",
        vec![line(
            &root.join("notes"),
            "",
            &ago(2),
            &edit(&root.join("notes/n.md")),
        )],
    );
    session(
        "old",
        "Long ago",
        vec![line(&web, "main", &ago(90), &edit(&web.join("README.md")))],
    );
    let one = |id: &str, title: &str, at: i64, cwd: &Path, branch: &str, file: &Path| {
        session(id, title, vec![line(cwd, branch, &ago(at), &edit(file))])
    };
    one(
        "released",
        "Released work",
        11,
        &web,
        "feat/released",
        &web.join("README.md"),
    );
    session(
        "nested",
        "Nested worktrees",
        vec![
            line(&web, "main", &ago(12), &edit(&web.join(".wt/y/a.ts"))),
            line(&web, "main", &ago(12), &edit(&web.join(".wt/z/b.ts"))),
        ],
    );
    one("tagged", "Tagged", 13, &web, "v1", &web.join("README.md"));
    one(
        "second-clone",
        "Second clone",
        14,
        &web2,
        "feat/two",
        &web2.join("README.md"),
    );
    one(
        "in-chauffeur",
        "Already in Chauffeur",
        15,
        &web,
        "feat/chauffeur",
        &web.join("README.md"),
    );
    session(
        "external",
        "In a scratch worktree",
        vec![
            line(&web, "main", &ago(16), &edit(&web.join("src/e.ts"))),
            line(&ext, "feat/ext", &ago(16), &edit(&ext.join("x.ts"))),
        ],
    );
    session(
        "in-chauffeur-2",
        "Already in Chauffeur, from another clone",
        vec![line(
            &web2,
            "feat/chauffeur",
            &ago(17),
            &edit(&web2.join("README.md")),
        )],
    );
    let sessions = claude.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let pid = std::process::id();
    std::fs::write(
        sessions.join(format!("{pid}.json")),
        format!(r#"{{"pid":{pid},"sessionId":"running"}}"#),
    )
    .unwrap();

    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("settings.toml"),
        format!("github_org = \"fixture\"\ngithub_sources = false\nbranch_prefix = \"t\"\nclone_roots = []\nport_range_start = 46950\nlinear_enabled = false\n{}", extra(&root)),
    )
    .unwrap();
    // Chauffeur's workspaces folder named through a symlink, as a user may set it.
    std::os::unix::fs::symlink(root.join("workspaces"), root.join("ws-link")).unwrap();
    let daemon = Daemon::open(Paths::new(home, root.join("ws-link"), root.join("repos"))).unwrap();
    let mut vars: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let path = format!("{}:{}", bin.display(), vars["PATH"]);
    vars.insert("PATH".into(), path);
    // The agents' own Claude folder, where imported sessions are copied: never the user's.
    vars.insert(
        "CLAUDE_CONFIG_DIR".into(),
        root.join("agents-claude").to_string_lossy().into_owned(),
    );
    daemon.shell_env.set_vars(vars);
    Fixture {
        _dir: dir,
        root,
        claude,
        daemon,
    }
}

async fn plan(f: &Fixture, since_days: Option<u32>, all: bool) -> ClaudeCodeImportResult {
    planned(
        f,
        ClaudeCodeImportParams {
            since_days,
            all,
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

async fn planned(
    f: &Fixture,
    params: ClaudeCodeImportParams,
) -> chauffeurd::Result<ClaudeCodeImportResult> {
    chauffeurd::claudecode::import(
        &f.daemon,
        ClaudeCodeImportParams {
            dry_run: true,
            claude_dir: Some(f.claude.to_string_lossy().into_owned()),
            ..params
        },
    )
    .await
}

fn branch(repo: &str, name: &str, live: bool) -> ClaudeCodeBranch {
    ClaudeCodeBranch {
        repo: repo.into(),
        branch: name.into(),
        live,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dry_run_lists_the_window_s_sessions_their_branches_and_where_each_lands() {
    let f = fixture();
    let plan = plan(&f, None, false).await;
    let session = |id: &str| {
        plan.sessions
            .iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("{id} in {:?}", plan.sessions))
    };

    // The last 60 days, the most recent first; Chauffeur's own (on the Agent SDK) left out.
    let ids: Vec<&str> = plan.sessions.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "running",
            "notes",
            "orchestrator",
            "simple-api",
            "simple-web",
            "away",
            "no-github",
            "history",
            "released",
            "nested",
            "tagged",
            "second-clone",
            "in-chauffeur",
            "external",
            "in-chauffeur-2"
        ]
    );
    assert_eq!(plan.since_days, Some(60));

    let running = session("running");
    assert_eq!(running.outcome, ClaudeCodeOutcome::Running);
    assert!(
        running
            .reason
            .as_deref()
            .unwrap()
            .contains("close it, then run the import again")
    );
    assert_eq!(session("notes").outcome, ClaudeCodeOutcome::Skipped);
    assert!(
        plan.warnings
            .iter()
            .any(|w| w.contains("“Notes” ran in") && w.contains("not a git repo")),
        "{:?}",
        plan.warnings
    );

    let no_github = session("no-github");
    assert_eq!(no_github.outcome, ClaudeCodeOutcome::Skipped);
    assert!(
        no_github
            .reason
            .as_deref()
            .unwrap()
            .contains("has no GitHub remote")
    );
    assert!(
        plan.warnings
            .iter()
            .any(|w| w.contains("clones/local has no GitHub remote")),
        "{:?}",
        plan.warnings
    );
    // GitHub could not say whether api's branch is merged: it counts as live, and says so.
    // api's main is its default branch though origin/HEAD names a master gone since.
    assert_eq!(
        session("simple-api").branches,
        [
            branch("api", "feat/live", true),
            branch("api", "main", false)
        ]
    );
    // A PR into a release branch merged nothing into main.
    assert_eq!(
        session("released").branches,
        [branch("web", "feat/released", true)]
    );
    // A worktree deleted but still listed gives its branch; one removed, nothing.
    assert_eq!(session("nested").branches, [branch("web", "feat/y", true)]);
    assert!(
        plan.warnings
            .iter()
            .any(|w| w.contains("“Nested worktrees” wrote in") && w.contains(".wt/z")),
        "{:?}",
        plan.warnings
    );
    assert_eq!(session("tagged").branches, [branch("web", "v1", true)]);
    // A scratch worktree removed since but listed by git: its branch.
    assert_eq!(
        session("external").branches,
        [
            branch("web", "main", false),
            branch("web", "feat/ext", true)
        ]
    );
    assert_eq!(
        session("second-clone").branches,
        [branch("web", "feat/two", true)]
    );
    assert_eq!(
        session("in-chauffeur").branches,
        [branch("web", "feat/chauffeur", false)]
    );
    // From the other clone of web too, and said once.
    assert_eq!(
        session("in-chauffeur-2").branches,
        [branch("web", "feat/chauffeur", false)]
    );
    let said = plan
        .warnings
        .iter()
        .filter(|w| *w == "feat/chauffeur is in a Chauffeur workspace already: left out.")
        .count();
    assert_eq!(said, 1, "{:?}", plan.warnings);
    assert!(
        plan.warnings
            .iter()
            .any(|w| w == "Could not ask whether feat/live is merged: it is counted as live."),
        "{:?}",
        plan.warnings
    );
    // A folder only that branch has is no folder gone: the write counts, on the branch then.
    assert_eq!(session("away").branches, [branch("web", "feat/away", true)]);

    // Branches: written (on the branch checked out then, or there now elsewhere) or created.
    assert_eq!(
        session("simple-web").branches,
        [branch("web", "feat/live", true)]
    );
    assert_eq!(
        session("history").branches,
        [
            branch("web", "feat/merged", false),
            branch("web", "feat/squashed", false),
            branch("web", "main", false),
            branch("web", "feat/old-squash", false),
            branch("docs", "main", false),
            branch("web", "feat/gone", false)
        ]
    );
    assert!(
        plan.warnings
            .iter()
            .any(|w| w.contains("“Old work” wrote in") && w.contains("gone since")),
        "{:?}",
        plan.warnings
    );
    assert_eq!(
        session("orchestrator").branches,
        [
            branch("web", "feat/held", true),
            branch("web", "feat/child", true)
        ]
    );

    // Where each lands.
    let shape: Vec<String> = plan
        .workspaces
        .iter()
        .map(|w| {
            format!(
                "{:?} {} on {} [{}] parent={} chats={} changes={}",
                w.kind,
                w.name,
                w.branch.as_deref().unwrap_or("(new)"),
                w.repos.join(","),
                w.parent.as_deref().unwrap_or("-"),
                w.sessions.join(","),
                w.changes_from.join(","),
            )
        })
        .collect();
    assert_eq!(
        shape,
        [
            "Parent orchestrate-the-release on (new) [web] parent=- chats=orchestrator changes=web",
            "Branch feat/child on feat/child [web] parent=orchestrate-the-release chats= changes=",
            "Branch feat/held on feat/held [web] parent=orchestrate-the-release chats= changes=",
            "Branch feat/live on feat/live [api,web] parent=- chats=simple-web,simple-api changes=",
            "Branch feat/away on feat/away [web] parent=- chats=away changes=",
            "Branch feat/released on feat/released [web] parent=- chats=released changes=",
            "Branch feat/y on feat/y [web] parent=- chats=nested changes=",
            "Branch v1 on v1 [web] parent=- chats=tagged changes=",
            "Branch feat/two on feat/two [web] parent=- chats=second-clone changes=",
            "Branch feat/ext on feat/ext [web] parent=- chats=external changes=",
            // docs, touched only on main, carries its changes there.
            "Changes docs-changes on (new) [docs] parent=- chats= changes=docs",
        ]
    );
    assert_eq!(session("simple-api").lands_in.as_deref(), Some("feat/live"));
    assert_eq!(
        session("history").lands_in.as_deref(),
        Some("history of web")
    );
    assert_eq!(plan.histories.len(), 1);
    assert_eq!(
        (
            plan.histories[0].repo.as_str(),
            plan.histories[0].base.as_str()
        ),
        ("web", "main")
    );
    assert_eq!(
        plan.histories[0].sessions,
        ["in-chauffeur-2", "in-chauffeur", "history"]
    );

    // A live branch checked out in one of the user's worktrees is released there; in another
    // clone of the repo (feat/two in web-2), it is fetched into the one chosen for the run, which
    // keeps it.
    // One whose folder is gone carries its commits only; an orchestration's child (feat/held) is
    // released as any.
    let release = |branch: &str, folder: &str, commits_only: bool| ClaudeCodeRelease {
        repo: "web".into(),
        branch: branch.into(),
        folder: f.root.join(folder).to_string_lossy().into_owned(),
        commits_only,
        moved: None,
    };
    assert_eq!(
        plan.releases,
        [
            release("feat/held", "wt/held", false),
            release("feat/y", "clones/web/.wt/y", true),
            release("feat/ext", "scratch/ext", true),
        ]
    );
}

/// A GitLab installation (`gitlab.example.com`, main group `team`): `web` is `team/web` there,
/// `api` `team/shop/api`, `docs` is on GitHub. A `glab` that logs what it is asked and knows 101
/// merged merge requests into main, a full page then the one from web's `feat/squashed` at its tip.
fn on_gitlab() -> (Fixture, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let urls = [
        ("web", "git@gitlab.example.com:team/web.git"),
        ("api", "https://gitlab.example.com/team/shop/api.git"),
        ("docs", "https://github.com/fixture/docs.git"),
    ];
    for (name, url) in urls {
        let clone = clone_repo(&root, name, &[]);
        git(&clone, &["remote", "set-url", "origin", url]);
        let origin = root.join("origin").join(format!("{name}.git"));
        git(
            &clone,
            &[
                "config",
                &format!("url.{}.insteadOf", origin.display()),
                url,
            ],
        );
    }
    let web = root.join("clones/web");
    let at = |name: &str, file: &str| {
        git(&web, &["switch", "-q", "-c", name, "main"]);
        let tip = commit(&web, file, "x\n");
        git(&web, &["switch", "-q", "main"]);
        tip
    };
    at("feat/live", "live.txt");
    let squashed = at("feat/squashed", "squashed.txt");

    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = root.join("glab.log");
    let glab = bin.join("glab");
    std::fs::write(
        &glab,
        format!(
            "#!/bin/sh\necho \"$*\" >> '{log}'\ncase \"$*\" in\n  *state=merged*target_branch=main*page=1) printf '['; i=0; while [ $i -lt 99 ]; do printf '{{\"iid\":%s,\"state\":\"merged\",\"source_branch\":\"other-%s\",\"target_branch\":\"main\",\"sha\":\"x\"}},' $i $i; i=$((i+1)); done; echo '{{\"iid\":99,\"state\":\"merged\",\"source_branch\":\"other-99\",\"target_branch\":\"main\",\"sha\":\"x\"}}]' ;;\n  *state=merged*target_branch=main*page=2) echo '[{{\"iid\":100,\"state\":\"merged\",\"source_branch\":\"feat/squashed\",\"target_branch\":\"main\",\"sha\":\"{squashed}\"}}]' ;;\n  *) echo '[]' ;;\nesac\n",
            log = log.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&glab, std::fs::Permissions::from_mode(0o755)).unwrap();

    let claude = root.join("claude");
    let projects = claude.join("projects").join("-code");
    std::fs::create_dir_all(&projects).unwrap();
    for (id, folder, branch) in [
        ("live", "web", "feat/live"),
        ("squashed", "web", "feat/squashed"),
        ("api", "api", "main"),
        ("docs", "docs", "main"),
    ] {
        let clone = root.join("clones").join(folder);
        let text = format!(
            "{{\"type\":\"ai-title\",\"aiTitle\":\"{id}\"}}\n{}\n",
            line(&clone, branch, &ago(3), &edit(&clone.join("src/a.ts")))
        );
        std::fs::write(projects.join(format!("{id}.jsonl")), text).unwrap();
    }

    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("settings.toml"),
        "forge = \"gitlab\"\ngitlab_host = \"gitlab.example.com\"\ngitlab_group = \"team\"\ngithub_sources = false\nbranch_prefix = \"t\"\nclone_roots = []\nport_range_start = 46970\nlinear_enabled = false\n",
    )
    .unwrap();
    let daemon = Daemon::open(Paths::new(
        home,
        root.join("workspaces"),
        root.join("repos"),
    ))
    .unwrap();
    let mut vars: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let path = format!("{}:{}", bin.display(), vars["PATH"]);
    vars.insert("PATH".into(), path);
    vars.insert(
        "CLAUDE_CONFIG_DIR".into(),
        root.join("agents-claude").to_string_lossy().into_owned(),
    );
    daemon.shell_env.set_vars(vars);
    (
        Fixture {
            _dir: dir,
            root,
            claude,
            daemon,
        },
        log,
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn on_a_gitlab_sessions_are_matched_by_project_and_squash_merges_read_from_its_merge_requests()
 {
    let (f, log) = on_gitlab();
    let plan = plan(&f, None, false).await;
    let session = |id: &str| {
        plan.sessions
            .iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("{id} in {:?}", plan.sessions))
    };
    assert_eq!(session("live").branches, [branch("web", "feat/live", true)]);
    assert_eq!(
        session("api").branches,
        [branch("team/shop/api", "main", false)]
    );
    assert_eq!(session("docs").outcome, ClaudeCodeOutcome::Skipped);
    // Squash-merged: its merge request into main, from its tip, on the second page.
    assert_eq!(
        session("squashed").branches,
        [branch("web", "feat/squashed", false)]
    );
    // Read once, by pages up to the first short one, never branch by branch (the ticket's rule).
    let asked = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        asked.lines().filter(|l| l.contains("state=merged")).count(),
        2,
        "{asked}"
    );
    assert!(!asked.contains("source_branch="), "{asked}");
}

async fn refusal(f: &Fixture, params: ClaudeCodeImportParams) -> String {
    chauffeurd::claudecode::import(
        &f.daemon,
        ClaudeCodeImportParams {
            dry_run: true,
            ..params
        },
    )
    .await
    .unwrap_err()
    .to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreadable_window_or_folder_is_refused_and_an_empty_one_said() {
    let f = fixture();
    let since = refusal(
        &f,
        ClaudeCodeImportParams {
            since_days: Some(u32::MAX),
            claude_dir: Some(f.claude.to_string_lossy().into_owned()),
            ..Default::default()
        },
    )
    .await;
    assert!(since.contains("goes back before any date"), "{since}");
    let relative = refusal(
        &f,
        ClaudeCodeImportParams {
            claude_dir: Some("claude".into()),
            ..Default::default()
        },
    )
    .await;
    assert!(relative.contains("is relative"), "{relative}");
    let empty = chauffeurd::claudecode::import(
        &f.daemon,
        ClaudeCodeImportParams {
            dry_run: true,
            claude_dir: Some(f.root.join("notes").to_string_lossy().into_owned()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(empty.sessions.is_empty());
    assert!(
        empty
            .warnings
            .iter()
            .any(|w| w.contains("holds no `projects` folder")),
        "{:?}",
        empty.warnings
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_window_widens_with_since_a_day_and_all() {
    let f = fixture();
    let ids =
        |plan: ClaudeCodeImportResult| plan.sessions.into_iter().map(|s| s.id).collect::<Vec<_>>();
    assert!(!ids(plan(&f, None, false).await).contains(&"old".to_string()));
    assert!(ids(plan(&f, Some(100), false).await).contains(&"old".to_string()));
    let all = plan(&f, Some(1), true).await;
    assert_eq!(all.since_days, None);
    assert!(ids(all).contains(&"old".to_string()));
    // Since a day: from its midnight here.
    let day = |days: i64| {
        (chrono::Local::now() - chrono::Duration::days(days))
            .format("%Y-%m-%d")
            .to_string()
    };
    let since_date = |days| ClaudeCodeImportParams {
        since_date: Some(day(days)),
        ..Default::default()
    };
    let by_date = planned(&f, since_date(100)).await.unwrap();
    assert_eq!(
        (by_date.since_date.clone(), by_date.since_days),
        (Some(day(100)), None)
    );
    assert!(ids(by_date).contains(&"old".to_string()));
    assert!(!ids(planned(&f, since_date(30)).await.unwrap()).contains(&"old".to_string()));
    let refused = planned(
        &f,
        ClaudeCodeImportParams {
            since_date: Some("last week".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("takes a number of days or a date, YYYY-MM-DD"),
        "{refused}"
    );
    let narrow = ids(plan(&f, Some(5), false).await);
    assert!(
        narrow.contains(&"simple-api".to_string()) && !narrow.contains(&"simple-web".to_string()),
        "{narrow:?}"
    );
}

/// Held by the tests that import: the import runs one at a time in a process.
static IMPORTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The stand-in for the agent sidecar the import talks to (`fixtures/history-sidecar.ts`).
fn history_sidecar() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/history-sidecar.ts")
}

/// The fixture with `web` known to Chauffeur, so that it can make its history exploration, and
/// the stand-in sidecar.
fn importing() -> Fixture {
    let f = fixture_with(|root| {
        format!(
            "\n[repos.web]\npath = \"{}\"\ndefault_branch = \"main\"\n",
            root.join("clones/web").display()
        )
    });
    f.daemon.chats.sidecar.use_binary(history_sidecar());
    // The agents' Claude folder is the one read, as by default: the copies are read again.
    let mut vars = f.daemon.shell_env.vars();
    vars.insert(
        "CLAUDE_CONFIG_DIR".into(),
        f.claude.to_string_lossy().into_owned(),
    );
    f.daemon.shell_env.set_vars(vars);
    f
}

async fn import(f: &Fixture, dry_run: bool) -> ClaudeCodeImportResult {
    chauffeurd::claudecode::import(
        &f.daemon,
        ClaudeCodeImportParams {
            dry_run,
            claude_dir: Some(f.claude.to_string_lossy().into_owned()),
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

fn outcome(plan: &ClaudeCodeImportResult, id: &str) -> (ClaudeCodeOutcome, Option<String>) {
    let session = plan.sessions.iter().find(|s| s.id == id).unwrap();
    (session.outcome, session.lands_in.clone())
}

/// A session with no live branch, its prompt and answer at `at`.
fn past_session(f: &Fixture, id: &str, at: &str) {
    let web = f.root.join("clones/web");
    let text = [
        format!(
            r#"{{"type":"user","cwd":"{}","entrypoint":"cli","timestamp":"{at}","message":{{"role":"user","content":"What does {id} change?"}}}}"#,
            web.display()
        ),
        line(
            &web,
            "main",
            at,
            r#""message":{"id":"msg_1","content":[{"type":"text","text":"It changes nothing now."}]}"#,
        ),
    ]
    .join("\n");
    std::fs::write(
        f.claude.join(format!("projects/-code/{id}.jsonl")),
        text + "\n",
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn sessions_left_in_history_are_copied_into_the_repos_exploration_as_closed_tabs_once() {
    let _imports = IMPORTS.lock().await;
    let f = importing();
    let d = &f.daemon;
    // "history" carries a prompt.
    let at = ago(10);
    let transcript = f.claude.join("projects/-code/history.jsonl");
    let mut text = std::fs::read_to_string(&transcript).unwrap();
    text.push_str(&format!(
        "{{\"type\":\"user\",\"timestamp\":\"{at}\",\"message\":{{\"role\":\"user\",\"content\":\"Clean up the old branches\"}}}}\n"
    ));
    std::fs::write(&transcript, &text).unwrap();
    past_session(&f, "unreadable", &ago(9));
    past_session(&f, "lost", &ago(9));
    let planned = import(&f, true).await;
    assert_eq!(planned.histories[0].workspace, None);

    // Two at once: one runs, the other is refused.
    let run = || {
        chauffeurd::claudecode::import(
            d,
            ClaudeCodeImportParams {
                claude_dir: Some(f.claude.to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
    };
    let (plan, refused) = match tokio::join!(run(), run()) {
        (Ok(plan), Err(refused)) | (Err(refused), Ok(plan)) => (plan, refused),
        other => panic!("one import runs at a time: {other:?}"),
    };
    assert!(refused.to_string().contains("already running"), "{refused}");
    assert_eq!(plan.histories.len(), 1);
    assert_eq!(plan.histories[0].workspace.as_deref(), Some("web-history"));
    for id in ["history", "in-chauffeur", "in-chauffeur-2"] {
        assert_eq!(
            outcome(&plan, id),
            (ClaudeCodeOutcome::Imported, Some("web-history".into())),
            "{id}"
        );
    }
    // One that cannot be read is said, the others imported all the same.
    let unreadable = plan.sessions.iter().find(|s| s.id == "unreadable").unwrap();
    assert_eq!(unreadable.outcome, ClaudeCodeOutcome::Failed);
    // It landed nowhere.
    assert_eq!(unreadable.lands_in, None);
    assert!(
        unreadable
            .reason
            .as_deref()
            .is_some_and(|r| r.contains("could not be copied")),
        "{unreadable:?}"
    );
    // A session on a live branch goes to its workspace.
    assert_eq!(outcome(&plan, "simple-web").0, ClaudeCodeOutcome::Imported);

    let ws = d.workspaces.get("web-history").unwrap();
    assert!(ws.explore);
    assert_eq!(ws.state, chauffeur_protocol::model::WorkspaceState::Ready);
    // At the base branch: origin's main.
    let checkout = Path::new(&ws.path).join("web");
    assert_eq!(
        git(&checkout, &["rev-parse", "HEAD"]),
        git(&f.root.join("origin/web.git"), &["rev-parse", "main"])
    );
    let chats = d.store.chats_of(&ws.id).unwrap();
    let imported: Vec<_> = chats.iter().filter(|c| c.closed).collect();
    assert_eq!(imported.len(), 3, "{chats:?}");
    assert!(
        chats.iter().any(|c| !c.closed),
        "the exploration keeps a chat open"
    );
    let history = imported
        .iter()
        .find(|c| c.agent_session_id.as_deref() == Some("history"))
        .unwrap();
    assert_eq!(history.title.as_deref(), Some("Old work"));
    assert_eq!(
        history.permission_mode,
        chauffeur_protocol::model::PermissionMode::Plan
    );
    assert_eq!(history.status, chauffeur_protocol::model::ChatStatus::Idle);

    // The conversation, each message at its time.
    let events = d.store.events_of_chat(&history.id).unwrap();
    let prompt = events
        .iter()
        .find_map(|e| match &e.event {
            chauffeur_protocol::events::Event::Chat {
                event: chauffeur_protocol::events::ChatEvent::UserMessage { text, .. },
                ..
            } => Some((text.clone(), e.ts.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        prompt,
        ("Clean up the old branches".to_string(), at.clone())
    );

    // A copy that cannot be read back is no import.
    let lost = plan.sessions.iter().find(|s| s.id == "lost").unwrap();
    assert_eq!(lost.outcome, ClaudeCodeOutcome::Failed);
    assert!(
        lost.reason
            .as_deref()
            .is_some_and(|r| r.contains("read back")),
        "{lost:?}"
    );

    // A fork of an imported chat, kept for the exploration, still names the folder the session
    // first ran in: it is Chauffeur's.
    let copies = f.claude.join("projects").join(
        Path::new(&ws.path)
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .replace(|c: char| !c.is_ascii_alphanumeric(), "-"),
    );
    assert!(copies.join("history.jsonl").is_file());
    assert!(
        !copies.join("lost.jsonl").exists(),
        "a failed import's copy goes"
    );
    std::fs::copy(
        copies.join("history.jsonl"),
        copies.join("fork-of-history.jsonl"),
    )
    .unwrap();

    // Resumed in Claude Code since, its original runs: still imported, nothing more.
    let parent = std::os::unix::process::parent_id();
    std::fs::write(
        f.claude.join(format!("sessions/{parent}.json")),
        format!(r#"{{"pid":{parent},"sessionId":"history"}}"#),
    )
    .unwrap();

    // A folder of the user's whose name Claude writes as a workspace's, followed by more, is the
    // user's.
    let beside = f.root.join("workspaces-web-history2");
    let beside_key = beside
        .to_string_lossy()
        .replace(|c: char| !c.is_ascii_alphanumeric(), "-");
    std::fs::create_dir_all(f.claude.join("projects").join(&beside_key)).unwrap();
    std::fs::write(
        f.claude
            .join("projects")
            .join(&beside_key)
            .join("beside.jsonl"),
        line(&beside, "main", &ago(9), &edit(&beside.join("a.ts"))) + "\n",
    )
    .unwrap();

    // Again: none twice, each listed once; those that failed are tried again.
    let again = import(&f, false).await;
    assert_eq!(outcome(&again, "beside").0, ClaudeCodeOutcome::Skipped);
    let mut ids: Vec<_> = again.sessions.iter().map(|s| s.id.as_str()).collect();
    ids.sort_unstable();
    let count = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), count, "{ids:?}");
    assert!(!ids.contains(&"fork-of-history"), "{ids:?}");
    for id in ["history", "in-chauffeur", "in-chauffeur-2"] {
        assert_eq!(
            outcome(&again, id),
            (
                ClaudeCodeOutcome::AlreadyImported,
                Some("web-history".into())
            ),
            "{id}"
        );
    }
    assert_eq!(outcome(&again, "unreadable").0, ClaudeCodeOutcome::Failed);
    assert_eq!(outcome(&again, "lost").0, ClaudeCodeOutcome::Failed);
    assert_eq!(d.store.chats_of(&ws.id).unwrap().len(), chats.len());

    // A session since: added to the same exploration, its project folder holding the others.
    // Kept in two project folders (resumed from another folder): one session.
    past_session(&f, "later", &ago(8));
    let other = f.claude.join("projects/-other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::copy(
        f.claude.join("projects/-code/later.jsonl"),
        other.join("later.jsonl"),
    )
    .unwrap();
    let planned = import(&f, true).await;
    assert_eq!(
        planned.histories[0].workspace.as_deref(),
        Some("web-history")
    );
    let mut waiting = planned.histories[0].sessions.clone();
    waiting.sort_unstable();
    assert_eq!(waiting, ["later", "lost", "unreadable"]);
    let added = import(&f, false).await;
    assert_eq!(
        outcome(&added, "later"),
        (ClaudeCodeOutcome::Imported, Some("web-history".into()))
    );
    assert_eq!(d.store.chats_of(&ws.id).unwrap().len(), chats.len() + 1);

    // Archived, the exploration still holds its sessions; one since gets a new exploration.
    d.workspaces.archive(&ws.id, false).await.unwrap();
    past_session(&f, "after-archive", &ago(7));
    let after = import(&f, false).await;
    assert_eq!(
        outcome(&after, "after-archive"),
        (ClaudeCodeOutcome::Imported, Some("web-history-2".into()))
    );
    assert_eq!(
        outcome(&after, "history"),
        (
            ClaudeCodeOutcome::AlreadyImported,
            Some("web-history".into())
        )
    );
    // The captures went with the runs that made them.
    let left: Vec<_> = std::fs::read_dir(f.root.join("home"))
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("claude-code-import")
        })
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

/// Two repos whose clones hold live branches in every place a branch can be:
/// - shop's clone itself on `feat/held`, a file changed and one untracked;
/// - `feat/wt` in a worktree of the user's, an untracked file there;
/// - `feat/free` checked out nowhere;
/// - `feat/tmp` in a worktree whose files a cleanup deleted, `feat/gone` in one removed whole;
/// - `feat/both` in shop (a worktree) and in docs (its clone), docs' clone for Chauffeur (another
///   one, set in the settings) holding a `feat/both` of its own, which no fetch can move.
///
/// One session per branch, and a stand-in sidecar.
struct Branches {
    f: Fixture,
    shop: PathBuf,
    tips: std::collections::HashMap<String, String>,
}

fn branches_fixture() -> Branches {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let shop = clone_repo(&root, "shop", &["src.txt"]);
    let docs = clone_repo(&root, "docs", &["src.txt"]);
    let lab = clone_repo(&root, "lab", &["src.txt"]);
    // feat/gone in a repo of its own, which no failing workspace prunes.
    let tools = clone_repo(&root, "tools", &["src.txt"]);

    let mut tips = std::collections::HashMap::new();
    let mut branch = |clone: &Path, name: &str| {
        git(clone, &["switch", "-q", "-c", name, "main"]);
        let tip = commit(clone, &format!("{}.txt", name.replace('/', "-")), "work\n");
        git(clone, &["switch", "-q", "main"]);
        tips.insert(name.to_string(), tip);
    };
    for name in [
        "feat/held",
        "feat/wt",
        "feat/free",
        "feat/tmp",
        "feat/both",
        "feat/multi",
        "feat/busy",
    ] {
        branch(&shop, name);
    }
    branch(&docs, "feat/both");
    // feat/multi in shop and lab, checked out nowhere, a setting of the user's on shop's: lab's
    // worktree cannot be made (a hook of the clone's that fails, as a missing git-lfs does).
    git(&lab, &["switch", "-q", "-c", "feat/multi", "main"]);
    commit(&lab, "feat-multi.txt", "work\n");
    git(&lab, &["switch", "-q", "main"]);
    git(
        &shop,
        &["config", "branch.feat/multi.description", "the user's"],
    );
    let hook = lab.join(".git/hooks/post-checkout");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Docs' clone for Chauffeur: another clone, whose feat/both went another way.
    let chauffeur_docs = root.join("chauffeur").join("docs");
    git(
        &root,
        &[
            "clone",
            "-q",
            root.join("origin/docs.git").to_str().unwrap(),
            chauffeur_docs.to_str().unwrap(),
        ],
    );
    git(&chauffeur_docs, &["switch", "-q", "-c", "feat/both"]);
    commit(&chauffeur_docs, "other.txt", "another way\n");
    git(&chauffeur_docs, &["switch", "-q", "main"]);
    git(&docs, &["switch", "-q", "feat/both"]);
    // The clone on feat/held, changed.
    git(&shop, &["switch", "-q", "feat/held"]);
    std::fs::write(shop.join("README.md"), "shop, changed on feat/held\n").unwrap();
    std::fs::write(shop.join("notes.txt"), "untracked\n").unwrap();
    // A repo of its own inside, untracked: git lists it as a folder.
    let vendored = shop.join("vendor/lib");
    std::fs::create_dir_all(&vendored).unwrap();
    git(&vendored, &["init", "-q"]);
    std::fs::write(vendored.join("lib.txt"), "vendored\n").unwrap();
    std::os::unix::fs::symlink("lib.txt", vendored.join("link.txt")).unwrap();
    let worktree = |name: &str, folder: &Path| {
        git(
            &shop,
            &["worktree", "add", "-q", folder.to_str().unwrap(), name],
        );
    };
    let wt = root.join("wt/feat-wt");
    worktree("feat/wt", &wt);
    std::fs::write(wt.join("todo.txt"), "untracked in a worktree\n").unwrap();
    let both = root.join("wt/both");
    worktree("feat/both", &both);
    // A cleanup cut short: one tracked file gone, the worktree still listed.
    let tmp = root.join("tmp/feat-tmp");
    worktree("feat/tmp", &tmp);
    std::fs::remove_file(tmp.join("README.md")).unwrap();
    // A worktree where Claude Code still runs.
    let busy = root.join("wt/busy");
    worktree("feat/busy", &busy);
    git(&tools, &["switch", "-q", "-c", "feat/gone", "main"]);
    tips.insert(
        "feat/gone".into(),
        commit(&tools, "feat-gone.txt", "work\n"),
    );
    git(&tools, &["switch", "-q", "main"]);
    let gone = root.join("tmp/feat-gone");
    git(
        &tools,
        &["worktree", "add", "-q", gone.to_str().unwrap(), "feat/gone"],
    );
    std::fs::remove_dir_all(&gone).unwrap();
    // feat/multi held in a worktree of shop's: released, then put back when lab fails.
    let multi = root.join("wt/multi");
    worktree("feat/multi", &multi);

    let claude = root.join("claude");
    let projects = claude.join("projects").join("-code");
    std::fs::create_dir_all(&projects).unwrap();
    let session = |id: &str, cwd: &Path, branch: &str, file: &Path| {
        let text = [
            format!(r#"{{"type":"ai-title","aiTitle":"Work on {branch}"}}"#),
            format!(
                r#"{{"type":"user","cwd":"{}","entrypoint":"cli","timestamp":"{}","message":{{"role":"user","content":"Change {branch}"}}}}"#,
                cwd.display(),
                ago(3)
            ),
            line(cwd, branch, &ago(3), &edit(file)),
        ]
        .join("\n");
        std::fs::write(projects.join(format!("{id}.jsonl")), text + "\n").unwrap();
    };
    session("s-held", &shop, "feat/held", &shop.join("README.md"));
    session("s-wt", &wt, "feat/wt", &wt.join("todo.txt"));
    session("s-free", &shop, "feat/free", &shop.join("free.txt"));
    session("s-tmp", &shop, "main", &tmp.join("src.txt"));
    session("s-gone", &tools, "main", &gone.join("src.txt"));
    let text = [
        r#"{"type":"ai-title","aiTitle":"Both repos"}"#.to_string(),
        line(&both, "feat/both", &ago(2), &edit(&both.join("a.txt"))),
        line(&docs, "feat/both", &ago(2), &edit(&docs.join("b.txt"))),
    ]
    .join("\n");
    std::fs::write(projects.join("s-both.jsonl"), text + "\n").unwrap();
    let text = [
        r#"{"type":"ai-title","aiTitle":"Two repos, one branch"}"#.to_string(),
        line(&shop, "feat/multi", &ago(2), &edit(&shop.join("m.txt"))),
        line(&lab, "feat/multi", &ago(2), &edit(&lab.join("m.txt"))),
    ]
    .join("\n");
    std::fs::write(projects.join("s-multi.jsonl"), text + "\n").unwrap();
    session("s-busy", &busy, "feat/busy", &busy.join("x.txt"));
    session("s-busy-now", &busy, "feat/busy", &busy.join("y.txt"));
    let sessions = claude.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let pid = std::process::id();
    std::fs::write(
        sessions.join(format!("{pid}.json")),
        format!(r#"{{"pid":{pid},"sessionId":"s-busy-now"}}"#),
    )
    .unwrap();

    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("settings.toml"),
        format!(
            "github_org = \"fixture\"\ngithub_sources = false\nbranch_prefix = \"t\"\nclone_roots = [\"{}\"]\nport_range_start = 46970\nlinear_enabled = false\n\n[repos.docs]\npath = \"{}\"\ndefault_branch = \"main\"\n",
            root.join("clones").display(),
            chauffeur_docs.display()
        ),
    )
    .unwrap();
    let daemon = Daemon::open(Paths::new(
        home,
        root.join("workspaces"),
        root.join("repos"),
    ))
    .unwrap();
    let mut vars: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    vars.insert(
        "CLAUDE_CONFIG_DIR".into(),
        claude.to_string_lossy().into_owned(),
    );
    daemon.shell_env.set_vars(vars);
    daemon.chats.sidecar.use_binary(history_sidecar());
    Branches {
        f: Fixture {
            _dir: dir,
            root,
            claude,
            daemon,
        },
        shop,
        tips,
    }
}

fn is_branch(dir: &Path, name: &str) -> bool {
    let head = Command::new("git")
        .args(["symbolic-ref", "-q", "--short", "HEAD"])
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8_lossy(&head.stdout).trim() == name
}

#[tokio::test(flavor = "multi_thread")]
async fn live_branches_become_workspaces_released_from_the_users_checkouts() {
    let _imports = IMPORTS.lock().await;
    let b = branches_fixture();
    let (f, shop) = (&b.f, &b.shop);
    let d = &f.daemon;
    let planned = import(f, true).await;
    // Docs' feat/both is fetched into Chauffeur's own clone: the user's checkout keeps it.
    let mut released: Vec<_> = planned
        .releases
        .iter()
        .map(|r| (r.repo.as_str(), r.branch.as_str(), r.commits_only))
        .collect();
    released.sort_unstable();
    assert_eq!(
        released,
        [
            ("shop", "feat/both", false),
            ("shop", "feat/held", false),
            ("shop", "feat/multi", false),
            ("shop", "feat/tmp", true),
            ("shop", "feat/wt", false),
            ("tools", "feat/gone", true),
        ],
        "{:?}",
        planned.releases
    );

    let plan = import(f, false).await;
    let made = |branch: &str| {
        let w = plan
            .workspaces
            .iter()
            .find(|w| w.branch.as_deref() == Some(branch))
            .unwrap();
        (w.outcome, w.workspace.clone())
    };
    let repo_of = |branch: &str| {
        if branch == "feat/gone" {
            "tools"
        } else {
            "shop"
        }
    };
    for branch in ["feat/held", "feat/wt", "feat/free", "feat/tmp", "feat/gone"] {
        let (outcome, slug) = made(branch);
        assert_eq!(outcome, ClaudeCodeOutcome::Imported, "{branch}: {plan:?}");
        let ws = d.workspaces.get(slug.as_deref().unwrap()).unwrap();
        let checkout = Path::new(&ws.path).join(repo_of(branch));
        assert!(is_branch(&checkout, branch), "{branch}");
        assert_eq!(
            git(&checkout, &["rev-parse", "HEAD"]),
            b.tips[branch],
            "{branch}"
        );
        // Its session, a resumable chat, open.
        let chats = d.store.chats_of(&ws.id).unwrap();
        assert!(
            chats
                .iter()
                .any(|c| !c.closed && c.agent_session_id.is_some()),
            "{branch}: {chats:?}"
        );
    }
    let path = |branch: &str| {
        PathBuf::from(
            d.workspaces
                .get(made(branch).1.as_deref().unwrap())
                .unwrap()
                .path,
        )
        .join(repo_of(branch))
    };

    // Released in the clone: HEAD detached on the same commit, its changes kept there and copied.
    assert!(!is_branch(shop, "feat/held"));
    assert_eq!(git(shop, &["rev-parse", "HEAD"]), b.tips["feat/held"]);
    assert_eq!(
        std::fs::read_to_string(shop.join("README.md")).unwrap(),
        "shop, changed on feat/held\n"
    );
    assert_eq!(
        std::fs::read_to_string(path("feat/held").join("README.md")).unwrap(),
        "shop, changed on feat/held\n"
    );
    assert_eq!(
        std::fs::read_to_string(path("feat/held").join("notes.txt")).unwrap(),
        "untracked\n"
    );
    assert!(path("feat/held").join("vendor/lib/lib.txt").is_file());
    assert_eq!(
        std::fs::read_link(path("feat/held").join("vendor/lib/link.txt")).unwrap(),
        Path::new("lib.txt")
    );
    // In the user's worktree too.
    assert!(!is_branch(&f.root.join("wt/feat-wt"), "feat/wt"));
    assert!(path("feat/wt").join("todo.txt").is_file());
    // A half-deleted checkout carries its commits, not its deletion; a gone one its commits.
    assert!(path("feat/tmp").join("README.md").is_file());
    assert!(
        path("feat/tmp").join("src.txt").is_file() && path("feat/tmp").join("README.md").is_file()
    );
    assert!(path("feat/gone").join("feat-gone.txt").is_file());

    // The workspace that failed (docs' branch went another way in Chauffeur's clone) puts back
    // what it released; the others went on.
    assert_eq!(made("feat/both").0, ClaudeCodeOutcome::Failed, "{plan:?}");
    assert!(is_branch(&f.root.join("wt/both"), "feat/both"));
    let both = plan.sessions.iter().find(|s| s.id == "s-both").unwrap();
    assert_eq!(both.outcome, ClaudeCodeOutcome::Failed);
    assert!(
        both.reason
            .as_deref()
            .is_some_and(|r| r.contains("could not be made")),
        "{both:?}"
    );

    // A branch of two repos whose second worktree cannot be made: the user's branch stays, with
    // its settings, though the creation had made a worktree on it.
    assert_eq!(made("feat/multi").0, ClaudeCodeOutcome::Failed, "{plan:?}");
    assert!(
        is_branch(&f.root.join("wt/multi"), "feat/multi"),
        "put back"
    );
    assert_eq!(
        git(shop, &["rev-parse", "refs/heads/feat/multi"]),
        b.tips["feat/multi"]
    );
    assert_eq!(
        git(shop, &["config", "branch.feat/multi.description"]),
        "the user's"
    );
    // Where Claude Code still runs, nothing is released.
    assert_eq!(made("feat/busy").0, ClaudeCodeOutcome::Failed);
    assert!(
        plan.workspaces.iter().any(|w| w
            .reason
            .as_deref()
            .is_some_and(|r| r.contains("still runs in"))),
        "{plan:?}"
    );
    assert!(is_branch(&f.root.join("wt/busy"), "feat/busy"));

    // Again, lab's hook gone: nothing twice; the failed ones are tried again, feat/multi with
    // both its repos.
    std::fs::remove_file(f.root.join("clones/lab/.git/hooks/post-checkout")).unwrap();
    // feat/free, imported for shop, found in lab too: the workspace gains lab.
    let lab = f.root.join("clones/lab");
    git(&lab, &["switch", "-q", "-c", "feat/free", "main"]);
    commit(&lab, "free.txt", "lab's\n");
    git(&lab, &["switch", "-q", "main"]);
    std::fs::write(
        f.claude.join("projects/-code/s-free-lab.jsonl"),
        line(&lab, "feat/free", &ago(4), &edit(&lab.join("lab.txt"))) + "\n",
    )
    .unwrap();
    // A session on feat/held the first run did not see (it ran in the clone, on that branch):
    // into the workspace the first run made on it.
    std::fs::write(
        f.claude.join("projects/-code/s-held-2.jsonl"),
        line(shop, "feat/held", &ago(4), &edit(&shop.join("README.md"))) + "\n",
    )
    .unwrap();
    let again = import(f, false).await;
    assert_eq!(
        outcome(&again, "s-held-2"),
        (ClaudeCodeOutcome::Imported, made("feat/held").1),
        "{again:?}"
    );
    assert_eq!(
        outcome(&again, "s-free-lab"),
        (ClaudeCodeOutcome::Imported, made("feat/free").1),
        "{again:?}"
    );
    let free = d
        .workspaces
        .get(made("feat/free").1.as_deref().unwrap())
        .unwrap();
    assert!(is_branch(&Path::new(&free.path).join("lab"), "feat/free"));
    assert_eq!(
        outcome(&again, "s-multi").0,
        ClaudeCodeOutcome::Imported,
        "{again:?}"
    );
    let multi = again
        .workspaces
        .iter()
        .find(|w| w.branch.as_deref() == Some("feat/multi"))
        .unwrap();
    assert_eq!(multi.repos, ["lab", "shop"]);
    for id in ["s-held", "s-wt", "s-free", "s-tmp", "s-gone"] {
        assert_eq!(
            outcome(&again, id).0,
            ClaudeCodeOutcome::AlreadyImported,
            "{id}"
        );
    }
    assert_eq!(outcome(&again, "s-both").0, ClaudeCodeOutcome::Failed);

    // feat/held, imported for shop, found in docs and in a worktree of lab's, whose new worktree
    // cannot be made: docs stays in the workspace, lab's checkout gets its branch back.
    let docs = f.root.join("clones/docs");
    for (clone, folder) in [(&docs, "wt/docs-held"), (&lab, "wt/lab-held")] {
        let at = f.root.join(folder);
        git(
            clone,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat/held",
                at.to_str().unwrap(),
                "main",
            ],
        );
        commit(&at, "held.txt", "held\n");
        let name = folder.replace('/', "-");
        std::fs::write(
            f.claude.join(format!("projects/-code/s-{name}.jsonl")),
            line(&at, "feat/held", &ago(4), &edit(&at.join("held.txt"))) + "\n",
        )
        .unwrap();
    }
    let hook = lab.join(".git/hooks/post-checkout");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let third = import(f, false).await;
    let held = d
        .workspaces
        .get(made("feat/held").1.as_deref().unwrap())
        .unwrap();
    assert_eq!(
        outcome(&third, "s-wt-lab-held").0,
        ClaudeCodeOutcome::Failed,
        "{third:?}"
    );
    assert!(is_branch(&Path::new(&held.path).join("docs"), "feat/held"));
    assert!(!Path::new(&held.path).join("lab").exists());
    assert!(
        is_branch(&f.root.join("wt/lab-held"), "feat/held"),
        "put back"
    );
    // Once the hook is gone, lab joins, docs already in.
    std::fs::remove_file(&hook).unwrap();
    let fourth = import(f, false).await;
    for id in ["s-wt-docs-held", "s-wt-lab-held"] {
        assert_eq!(
            outcome(&fourth, id),
            (ClaudeCodeOutcome::Imported, made("feat/held").1),
            "{fourth:?}"
        );
    }
    assert!(is_branch(&Path::new(&held.path).join("lab"), "feat/held"));
}

#[tokio::test(flavor = "multi_thread")]
async fn adopting_one_repos_clone_changes_no_other_repos() {
    adopting("fixture").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn adopting_takes_repos_named_with_their_owner() {
    adopting("").await;
}

/// No clone folder: `alpha` has no clone of Chauffeur's, only two of the user's, each holding a
/// branch; `beta` has Chauffeur's own beside the user's. Every user clone holds its branch with
/// changes. With `org` empty, the repos are named `fixture/alpha` and `fixture/beta`.
async fn adopting(org: &str) {
    let _imports = IMPORTS.lock().await;
    let named = |name: &str| {
        if org.is_empty() {
            format!("fixture/{name}")
        } else {
            name.to_string()
        }
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let on_branch = |name: &str, branch: &str| {
        let clone = clone_repo(&root, name, &[]);
        git(&clone, &["switch", "-q", "-c", branch]);
        commit(&clone, "work.txt", "work\n");
        std::fs::write(clone.join("README.md"), format!("{name}, changed\n")).unwrap();
        clone
    };
    let alpha = on_branch("alpha", "feat/a");
    let beta = on_branch("beta", "feat/b");
    // The user's second clone of alpha, on another branch.
    let alpha2 = root.join("clones2/alpha");
    git(
        &root,
        &[
            "clone",
            "-q",
            alpha.to_str().unwrap(),
            alpha2.to_str().unwrap(),
        ],
    );
    let alpha_url = "https://github.com/fixture/alpha.git";
    git(&alpha2, &["remote", "set-url", "origin", alpha_url]);
    git(
        &alpha2,
        &[
            "config",
            &format!("url.{}.insteadOf", root.join("origin/alpha.git").display()),
            alpha_url,
        ],
    );
    git(&alpha2, &["switch", "-q", "-c", "feat/a2", "origin/main"]);
    commit(&alpha2, "work2.txt", "work\n");
    std::fs::write(alpha2.join("README.md"), "alpha2, changed\n").unwrap();
    // Chauffeur's own clone of beta, where it clones repos.
    let own = root.join("repos").join(named("beta"));
    let origin = root.join("origin/beta.git");
    git(
        &root,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            own.to_str().unwrap(),
        ],
    );
    let url = "https://github.com/fixture/beta.git";
    git(&own, &["remote", "set-url", "origin", url]);
    git(
        &own,
        &[
            "config",
            &format!("url.{}.insteadOf", origin.display()),
            url,
        ],
    );

    let claude = root.join("claude");
    let projects = claude.join("projects").join("-code");
    std::fs::create_dir_all(&projects).unwrap();
    for (id, clone, branch) in [
        ("s-a", &alpha, "feat/a"),
        ("s-a2", &alpha2, "feat/a2"),
        ("s-b", &beta, "feat/b"),
    ] {
        std::fs::write(
            projects.join(format!("{id}.jsonl")),
            line(clone, branch, &ago(1), &edit(&clone.join("README.md"))) + "\n",
        )
        .unwrap();
    }
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("settings.toml"),
        format!("github_org = \"{org}\"\ngithub_sources = false\nbranch_prefix = \"t\"\nclone_roots = []\nport_range_start = 46990\nlinear_enabled = false\n"),
    )
    .unwrap();
    let daemon = Daemon::open(Paths::new(
        home,
        root.join("workspaces"),
        root.join("repos"),
    ))
    .unwrap();
    let mut vars: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    vars.insert(
        "CLAUDE_CONFIG_DIR".into(),
        claude.to_string_lossy().into_owned(),
    );
    daemon.shell_env.set_vars(vars);
    daemon.chats.sidecar.use_binary(history_sidecar());
    let f = Fixture {
        _dir: dir,
        root: root.clone(),
        claude,
        daemon,
    };

    // One clone of alpha is chosen for the run, its checkout released; the other's branch is
    // fetched into it, as beta's into Chauffeur's own clone.
    let planned = import(&f, true).await;
    let released: Vec<_> = planned.releases.iter().map(|r| r.repo.as_str()).collect();
    assert_eq!(released, [named("alpha")], "{planned:?}");
    let (alpha, alpha_branch, alpha2, alpha2_branch) =
        if Path::new(&planned.releases[0].folder) == alpha {
            (alpha, "feat/a", alpha2, "feat/a2")
        } else {
            (alpha2, "feat/a2", alpha, "feat/a")
        };

    let plan = import(&f, false).await;
    for w in &plan.workspaces {
        assert_eq!(w.outcome, ClaudeCodeOutcome::Imported, "{plan:?}");
    }
    // Alpha's clone becomes Chauffeur's: named in the settings for a repo of the main
    // organization, its folder among the clone folders for one named with its owner. Its checkout
    // is released.
    let settings = f.daemon.settings.get();
    if org.is_empty() {
        assert_eq!(settings.clone_roots.len(), 1, "{:?}", settings.clone_roots);
    } else {
        assert!(
            settings.clone_roots.is_empty(),
            "{:?}",
            settings.clone_roots
        );
        let path = settings.repos.get("alpha").and_then(|r| r.path.clone());
        assert_eq!(path.as_deref(), Some(alpha.to_str().unwrap()));
    }
    assert!(!is_branch(&alpha, alpha_branch));
    // The other clone of alpha stays on its branch, as beta's: Chauffeur keeps its own clone.
    assert!(is_branch(&alpha2, alpha2_branch));
    assert!(is_branch(&beta, "feat/b"));
    let ws = |branch: &str| {
        let slug = plan
            .workspaces
            .iter()
            .find(|w| w.branch.as_deref() == Some(branch))
            .and_then(|w| w.workspace.clone())
            .unwrap();
        PathBuf::from(f.daemon.workspaces.get(&slug).unwrap().path)
    };
    let beta_checkout = ws("feat/b").join("beta");
    assert_eq!(
        PathBuf::from(git(
            &beta_checkout,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"]
        )),
        own.join(".git")
    );
    let alpha2_checkout = ws("feat/a2").join("alpha");
    assert_eq!(
        PathBuf::from(git(
            &alpha2_checkout,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"]
        )),
        alpha.join(".git")
    );
    // Each carries its changes.
    for (checkout, name) in [
        (ws("feat/a").join("alpha"), "alpha"),
        (alpha2_checkout, "alpha2"),
        (beta_checkout, "beta"),
    ] {
        assert_eq!(
            std::fs::read_to_string(checkout.join("README.md")).unwrap(),
            format!("{name}, changed\n")
        );
    }
}

/// Repos the import makes a workspace in, Chauffeur having no clone of them, each with a live
/// branch: `pn` (pnpm's lockfile, and inline settings of the user's), `committed` (bun's, and a
/// setup committed), `mine` (bundle's, and a setup in the user's settings), `br` (npm's, its
/// branch committing a setup in a worktree of the user's, the clone on main), `stale` (yarn's,
/// `origin/main` committing a setup the clone's checkout has not pulled); and `hist` (poetry's),
/// whose session lands in its history exploration. Which lockfile gives which setup is
/// `setup.rs`'s. Pn's clone also has an MCP server local to it, named in a warning.
#[tokio::test(flavor = "multi_thread")]
async fn a_repo_without_setup_gets_one_guessed_from_its_root_lockfiles() {
    let _imports = IMPORTS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let claude = root.join("claude");
    let projects = claude.join("projects").join("-code");
    std::fs::create_dir_all(&projects).unwrap();
    // A repo with `lockfile` and, when given, a committed setup; on `branch`, else main alone.
    let repo = |name: &str, lockfile: &str, committed: Option<&str>, branch: Option<&str>| {
        let clone = clone_repo(&root, name, &[]);
        commit(&clone, lockfile, "lock\n");
        if let Some(setup) = committed {
            std::fs::create_dir_all(clone.join(".chauffeur")).unwrap();
            commit(
                &clone,
                ".chauffeur/settings.toml",
                &format!("[scripts]\nsetup = \"{setup}\"\n"),
            );
        }
        git(&clone, &["push", "-q", "origin", "main"]);
        let session = match branch {
            Some(branch) => {
                git(&clone, &["switch", "-q", "-c", branch]);
                commit(&clone, "work.txt", "work\n");
                line(&clone, branch, &ago(1), &edit(&clone.join("README.md")))
            }
            None => line(&clone, "main", &ago(1), r#""message":{"content":[]}"#),
        };
        std::fs::write(projects.join(format!("s-{name}.jsonl")), session + "\n").unwrap();
        clone
    };
    let pn = repo("pn", "pnpm-lock.yaml", None, Some("feat/pn"));
    // An MCP server of Claude Code's local to pn's clone.
    std::fs::write(
        claude.join(".claude.json"),
        serde_json::json!({ "projects": { pn.to_string_lossy(): {
            "mcpServers": { "linear": {} }
        } } })
        .to_string(),
    )
    .unwrap();
    let committed = repo("committed", "bun.lock", Some("make deps"), Some("feat/c"));
    repo("mine", "Gemfile.lock", None, Some("feat/mine"));
    repo("hist", "poetry.lock", None, None);
    let session = |name: &str, folder: &Path, branch: &str| {
        let text = line(folder, branch, &ago(1), &edit(&folder.join("README.md")));
        std::fs::write(projects.join(format!("s-{name}.jsonl")), text + "\n").unwrap();
    };
    let br = clone_repo(&root, "br", &[]);
    commit(&br, "package-lock.json", "lock\n");
    git(&br, &["push", "-q", "origin", "main"]);
    let br_worktree = root.join("wt-br");
    git(
        &br,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/br",
            br_worktree.to_str().unwrap(),
        ],
    );
    std::fs::create_dir_all(br_worktree.join(".chauffeur")).unwrap();
    commit(
        &br_worktree,
        ".chauffeur/settings.toml",
        "[scripts]\nsetup = \"make br\"\n",
    );
    session("br", &br_worktree, "feat/br");
    let stale = clone_repo(&root, "stale", &["yarn.lock"]);
    let team = root.join("team-stale");
    git(
        &root,
        &[
            "clone",
            "-q",
            root.join("origin/stale.git").to_str().unwrap(),
            team.to_str().unwrap(),
        ],
    );
    std::fs::create_dir_all(team.join(".chauffeur")).unwrap();
    commit(
        &team,
        ".chauffeur/settings.toml",
        "[scripts]\nsetup = \"make stale\"\n",
    );
    git(&team, &["push", "-q", "origin", "main"]);
    git(&stale, &["fetch", "-q"]);
    git(&stale, &["switch", "-q", "-c", "feat/stale"]);
    commit(&stale, "work.txt", "work\n");
    session("stale", &stale, "feat/stale");
    // dev: its base set to develop in the user's settings, which alone has a lockfile, and a
    // clone of Chauffeur's own the sessions never ran in.
    let dev = clone_repo(&root, "dev", &[]);
    git(&dev, &["switch", "-q", "-c", "develop"]);
    commit(&dev, "uv.lock", "lock\n");
    git(&dev, &["push", "-q", "origin", "develop"]);
    git(&dev, &["switch", "-q", "-c", "feat/dev", "main"]);
    commit(&dev, "work.txt", "work\n");
    session("dev", &dev, "feat/dev");
    let own_dev = root.join("repos/dev");
    let dev_origin = root.join("origin/dev.git");
    git(
        &root,
        &[
            "clone",
            "-q",
            dev_origin.to_str().unwrap(),
            own_dev.to_str().unwrap(),
        ],
    );
    let dev_url = "https://github.com/fixture/dev.git";
    git(&own_dev, &["remote", "set-url", "origin", dev_url]);
    git(
        &own_dev,
        &[
            "config",
            &format!("url.{}.insteadOf", dev_origin.display()),
            dev_url,
        ],
    );

    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("settings.toml"),
        "github_org = \"fixture\"\ngithub_sources = false\nbranch_prefix = \"t\"\nclone_roots = []\nport_range_start = 46990\nlinear_enabled = false\n\n[repos]\npn = { env = { FOO = \"1\" } }\n\n[repos.mine.scripts]\nsetup = \"make mine\"\n\n[repos.dev]\ndefault_branch = \"develop\"\n",
    )
    .unwrap();
    let daemon = Daemon::open(Paths::new(
        home.clone(),
        root.join("workspaces"),
        root.join("repos"),
    ))
    .unwrap();
    let mut vars: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    vars.insert(
        "CLAUDE_CONFIG_DIR".into(),
        claude.to_string_lossy().into_owned(),
    );
    daemon.shell_env.set_vars(vars);
    daemon.chats.sidecar.use_binary(history_sidecar());
    let f = Fixture {
        _dir: dir,
        root: root.clone(),
        claude,
        daemon,
    };
    let guessed = |plan: &ClaudeCodeImportResult| {
        let mut setups: Vec<_> = plan
            .setups
            .iter()
            .map(|s| (s.repo.clone(), s.command.clone().unwrap_or_default()))
            .collect();
        setups.sort();
        setups
    };
    let expected = [
        ("dev".to_string(), "uv sync".to_string()),
        ("hist".to_string(), "poetry install".to_string()),
        ("pn".into(), "pnpm install".into()),
    ];

    // The dry run shows them and writes nothing.
    let before = std::fs::read_to_string(home.join("settings.toml")).unwrap();
    let planned = import(&f, true).await;
    assert_eq!(guessed(&planned), expected, "{planned:?}");
    let mcp = |plan: &ClaudeCodeImportResult| {
        plan.warnings
            .iter()
            .filter(|w| w.contains("MCP"))
            .cloned()
            .collect::<Vec<_>>()
    };
    let named = mcp(&planned);
    assert_eq!(named.len(), 1, "{named:?}");
    assert!(named[0].starts_with("pn: Claude Code's MCP server linear is local to"));
    assert_eq!(
        std::fs::read_to_string(home.join("settings.toml")).unwrap(),
        before
    );

    let plan = import(&f, false).await;
    assert_eq!(guessed(&plan), expected, "{plan:?}");
    assert_eq!(mcp(&plan), named);
    for w in &plan.workspaces {
        assert_eq!(w.outcome, ClaudeCodeOutcome::Imported, "{plan:?}");
    }
    // Written to the user's settings, theirs kept, a committed one left alone; nothing in the
    // repos.
    let settings = f.daemon.settings.get();
    let setup = |repo: &str| {
        let scripts = settings.repos.get(repo).and_then(|r| r.scripts.as_ref())?;
        chauffeurd::settings::RepoSettings::load(&root, &root, Some(scripts))
            .unwrap()
            .setup
    };
    assert_eq!(setup("pn").as_deref(), Some("pnpm install"));
    assert_eq!(setup("hist").as_deref(), Some("poetry install"));
    assert_eq!(setup("dev").as_deref(), Some("uv sync"));
    assert_eq!(setup("mine").as_deref(), Some("make mine"));
    for repo in ["committed", "br", "stale"] {
        assert_eq!(setup(repo), None, "{repo}");
    }
    // The user's inline settings of pn are kept beside its setup.
    let env = settings.repos.get("pn").map(|r| r.env.0.clone());
    assert_eq!(
        env.and_then(|e| e.get("FOO").cloned()).as_deref(),
        Some("1")
    );
    assert!(!pn.join(".chauffeur").exists());
    assert_eq!(
        git(&committed, &["status", "--porcelain", "--", ".chauffeur"]),
        ""
    );
    // Written before the worktrees: pn's workspace ran it.
    let slug = plan
        .workspaces
        .iter()
        .find(|w| w.branch.as_deref() == Some("feat/pn"))
        .and_then(|w| w.workspace.clone())
        .unwrap();
    let id = f.daemon.workspaces.get(&slug).unwrap().id;
    let commands: Vec<String> = f
        .daemon
        .procs
        .list(&id)
        .into_iter()
        .map(|p| p.command)
        .collect();
    assert_eq!(commands, ["pnpm install"]);

    // A second run makes no workspace: it guesses nothing. A session found later on pn's branch
    // goes into its workspace, its MCP server named again.
    std::fs::write(
        projects.join("s-pn-2.jsonl"),
        line(&pn, "feat/pn", &ago(1), &edit(&pn.join("README.md"))) + "\n",
    )
    .unwrap();
    let again = import(&f, true).await;
    assert!(again.setups.is_empty(), "{again:?}");
    assert_eq!(mcp(&again), named);
}

/// A session that ran in a worktree of `shop`'s, `wt/other`, wrote in another, `wt/fix`, from a
/// third, `wt/third`, and in a clone of another owner's `shop`, imported into feat/fix's
/// workspace: the first message sent to its chat tells its agent where shop lives now and which
/// folders are the user's, once, a restart included; its guard keeps it off the worktrees it ran
/// in, standing for the workspace's, not off the other repo's clone. A fork made before is told
/// and kept off them too.
#[tokio::test(flavor = "multi_thread")]
async fn an_imported_chat_is_told_where_its_repo_lives_and_kept_off_the_folders_it_ran_in() {
    let _imports = IMPORTS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let shop = clone_repo(&root, "shop", &[]);
    let fix = root.join("wt/fix");
    git(
        &shop,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/fix",
            fix.to_str().unwrap(),
        ],
    );
    commit(&fix, "a.txt", "a\n");
    // The session ran in another worktree of the user's, on a branch of no work of its own.
    let other = root.join("wt/other");
    git(
        &shop,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/other",
            other.to_str().unwrap(),
        ],
    );
    let third = root.join("wt/third");
    git(
        &shop,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/third",
            third.to_str().unwrap(),
        ],
    );
    // Another owner's shop, its clone on its default branch, nothing live there.
    let theirs = clone_repo(&root.join("theirs"), "shop", &[]);
    let theirs_url = "https://github.com/someone/shop.git";
    git(&theirs, &["remote", "set-url", "origin", theirs_url]);
    let claude = root.join("claude");
    let projects = claude.join("projects").join("-code");
    std::fs::create_dir_all(&projects).unwrap();
    std::fs::write(
        projects.join("s-fix.jsonl"),
        [
            line(&other, "feat/other", &ago(1), &edit(&fix.join("a.txt"))),
            line(&third, "feat/third", &ago(1), &edit(&fix.join("a.txt"))),
            line(
                &other,
                "feat/other",
                &ago(1),
                &edit(&theirs.join("README.md")),
            ),
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("settings.toml"),
        "github_org = \"fixture\"\ngithub_sources = false\nbranch_prefix = \"t\"\nclone_roots = []\nport_range_start = 46990\nlinear_enabled = false\n",
    )
    .unwrap();
    let paths = || Paths::new(home.clone(), root.join("workspaces"), root.join("repos"));
    let open = || {
        let daemon = Daemon::open(paths()).unwrap();
        let mut vars: std::collections::BTreeMap<String, String> = std::env::vars().collect();
        vars.insert(
            "CLAUDE_CONFIG_DIR".into(),
            claude.to_string_lossy().into_owned(),
        );
        daemon.shell_env.set_vars(vars);
        daemon.chats.sidecar.use_binary(history_sidecar());
        daemon
    };
    let f = Fixture {
        _dir: dir,
        root: root.clone(),
        claude: claude.clone(),
        daemon: open(),
    };
    let plan = import(&f, false).await;
    let slug = plan.workspaces[0].workspace.clone().expect("imported");
    let ws = PathBuf::from(f.daemon.workspaces.get(&slug).unwrap().path);
    let chat = f
        .daemon
        .store
        .kv_get("claudecode_import:s-fix")
        .unwrap()
        .expect("a chat");
    let fork = f.daemon.chats.fork(&chat, None, None).await.unwrap().id;
    let log = claude.join("sidecar.log");
    let sent = || -> Vec<serde_json::Value> {
        std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .filter(|r| r["method"] == "session.start" || r["method"] == "session.send")
            .collect()
    };

    f.daemon.chats.send(&chat, "change a.txt").await.unwrap();
    let first = sent();
    assert_eq!(first.len(), 1, "{first:?}");
    let start = &first[0]["params"];
    let prompt = start["prompt"].as_str().unwrap();
    // The folder it ran in, its folder now, where shop lives, the folders it ran in.
    for path in [&other, &ws, &ws.join("shop"), &fix] {
        assert!(
            prompt.contains(&path.display().to_string()),
            "{path:?}\n{prompt}"
        );
    }
    assert!(prompt.ends_with("change a.txt"), "{prompt}");
    // Its guard: the workspace's clone, then the worktree the session ran in and the one it
    // wrote in, each for shop's.
    let guarded: Vec<(String, String)> = start["source_clones"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["clone"].as_str().unwrap().to_owned(),
                c["worktree"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let shop_worktree = ws.join("shop").display().to_string();
    assert_eq!(
        guarded,
        [
            (shop.display().to_string(), shop_worktree.clone()),
            (other.display().to_string(), shop_worktree.clone()),
            (fix.display().to_string(), shop_worktree.clone()),
            (third.display().to_string(), shop_worktree),
        ]
    );
    // The fork: told, and kept off the same folders.
    f.daemon.chats.send(&fork, "change a.txt").await.unwrap();
    let forked = sent().pop().unwrap();
    let forked = &forked["params"];
    assert!(
        forked["prompt"]
            .as_str()
            .unwrap()
            .contains(&fix.display().to_string())
    );
    assert_eq!(forked["source_clones"], start["source_clones"]);

    // Told once: not again, nor after a restart.
    f.daemon.chats.send(&chat, "and b.txt").await.unwrap();
    let restarted = open();
    restarted.chats.send(&chat, "and c.txt").await.unwrap();
    let later: Vec<String> = sent()[2..]
        .iter()
        .map(|r| {
            let p = &r["params"];
            p["prompt"]
                .as_str()
                .or(p["text"].as_str())
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    assert_eq!(later.len(), 2, "{later:?}");
    for text in &later {
        assert!(!text.contains(&fix.display().to_string()), "{text}");
    }
}

/// The fixture's `orchestrator` (feat/child, feat/held) becomes a parent on web, on a new branch
/// named from its title, holding its chat, with a child per branch made without a prompt, the
/// released one carrying its uncommitted change; the parent's chat is told of its children, its
/// guard mapping the worktree it wrote feat/held in to that child's. A parent that cannot be
/// made takes its children with it, nothing released.
#[tokio::test(flavor = "multi_thread")]
async fn an_orchestration_becomes_a_parent_whose_chat_is_told_of_its_children() {
    let _imports = IMPORTS.lock().await;
    // First, web's worktrees cannot be made (a hook of the clone's that fails).
    let f = importing();
    let d = &f.daemon;
    let hook = f.root.join("clones/web/.git/hooks/post-checkout");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let failed = import(&f, false).await;
    let child = failed
        .workspaces
        .iter()
        .find(|w| w.branch.as_deref() == Some("feat/held"))
        .unwrap();
    assert_eq!(child.outcome, ClaudeCodeOutcome::Failed);
    assert!(
        child.reason.as_deref().is_some_and(
            |r| r.contains("its parent workspace orchestrate-the-release could not be made")
        ),
        "{child:?}"
    );
    assert!(
        is_branch(&f.root.join("wt/held"), "feat/held"),
        "nothing released"
    );
    std::fs::remove_file(&hook).unwrap();

    // Then a Claude Code session runs in feat/held's worktree: that child fails, the rest is made.
    let held_folder = f.root.join("wt/held");
    std::fs::write(
        f.claude.join("projects/-code/busy-held.jsonl"),
        line(
            &held_folder,
            "feat/held",
            &ago(1),
            r#""message":{"content":[]}"#,
        ) + "\n",
    )
    .unwrap();
    let busy = f.claude.join(format!(
        "sessions/{}.json",
        std::os::unix::process::parent_id()
    ));
    std::fs::write(
        &busy,
        format!(
            r#"{{"pid":{},"sessionId":"busy-held"}}"#,
            std::os::unix::process::parent_id()
        ),
    )
    .unwrap();
    let plan = import(&f, false).await;
    let (done, parent) = outcome(&plan, "orchestrator");
    assert_eq!(done, ClaudeCodeOutcome::Imported, "{plan:?}");
    let parent = d.workspaces.get(parent.as_deref().unwrap()).unwrap();
    assert_eq!(parent.slug, "orchestrate-the-release");
    let held_plan = plan
        .workspaces
        .iter()
        .find(|w| w.branch.as_deref() == Some("feat/held"))
        .unwrap();
    assert_eq!(held_plan.outcome, ClaudeCodeOutcome::Failed, "{plan:?}");
    // Once it is closed, a later run makes it under the same parent, no other parent made.
    std::fs::remove_file(&busy).unwrap();
    let later = import(&f, false).await;
    let held_plan = later
        .workspaces
        .iter()
        .find(|w| w.branch.as_deref() == Some("feat/held"))
        .unwrap();
    assert_eq!(held_plan.outcome, ClaudeCodeOutcome::Imported, "{later:?}");
    assert_eq!(held_plan.parent.as_deref(), Some("orchestrate-the-release"));
    let parent_row = d.workspaces.row(&parent.id).unwrap();
    assert_eq!(parent_row.parent_id, None);
    let mut children: Vec<(String, String)> = d
        .store
        .list_workspaces()
        .unwrap()
        .into_iter()
        .filter(|w| w.parent_id.as_deref() == Some(&parent.id))
        .map(|w| {
            let worktrees = d.store.worktrees_of(&w.id).unwrap();
            (w.slug, worktrees[0].branch.clone())
        })
        .collect();
    children.sort();
    assert_eq!(
        children,
        [
            ("feat-child".to_string(), "feat/child".to_string()),
            ("feat-held".into(), "feat/held".into()),
        ]
    );
    let parent_branch = &d.store.worktrees_of(&parent.id).unwrap()[0].branch;
    assert!(
        parent_branch.ends_with("orchestrate-the-release"),
        "{parent_branch}"
    );
    let held = d.workspaces.get("feat-held").unwrap();
    assert_eq!(
        std::fs::read_to_string(Path::new(&held.path).join("web/README.md")).unwrap(),
        "held, uncommitted\n"
    );
    for child in [&held, &d.workspaces.get("feat-child").unwrap()] {
        let chats = d.store.chats_of(&child.id).unwrap();
        assert!(
            chats.iter().all(|c| c.agent_session_id.is_none()),
            "{chats:?}"
        );
    }
    // The parent's chat is told its branches are its children now, and kept off the worktree it
    // wrote feat/held in, standing for that child's.
    let chat = d
        .store
        .kv_get("claudecode_import:orchestrator")
        .unwrap()
        .unwrap();
    d.chats.send(&chat, "go on").await.unwrap();
    let start = std::fs::read_to_string(f.claude.join("sidecar.log"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .rfind(|r| r["method"] == "session.start")
        .unwrap();
    let prompt = start["params"]["prompt"].as_str().unwrap();
    for said in [
        format!(
            "feat-child (on feat/child, in {})",
            d.workspaces.get("feat-child").unwrap().path
        ),
        format!("feat-held (on feat/held, in {})", held.path),
        "`chauffeur status <child>`".to_string(),
    ] {
        assert!(prompt.contains(&said), "{said}\n{prompt}");
    }
    let held_folder = held_folder.display().to_string();
    let guarded = start["params"]["source_clones"].as_array().unwrap();
    let entry = guarded
        .iter()
        .find(|c| c["clone"] == held_folder.as_str())
        .unwrap_or_else(|| panic!("{guarded:?}"));
    assert_eq!(
        entry["worktree"],
        Path::new(&held.path).join("web").display().to_string()
    );

    // Its chat lost (as when it could not be made), the session goes back to its parent: no
    // second parent.
    d.store.kv_delete("claudecode_import:orchestrator").unwrap();
    let again = import(&f, false).await;
    assert_eq!(
        outcome(&again, "orchestrator").1.as_deref(),
        Some("orchestrate-the-release"),
        "{again:?}"
    );
    let parents: Vec<_> = again
        .workspaces
        .iter()
        .filter(|w| w.kind == ClaudeCodeWorkspaceKind::Parent)
        .map(|w| (w.name.clone(), w.workspace.clone()))
        .collect();
    assert_eq!(
        parents,
        [(
            "orchestrate-the-release".to_string(),
            Some("orchestrate-the-release".to_string())
        )]
    );
    let made: Vec<String> = d
        .store
        .list_workspaces()
        .unwrap()
        .into_iter()
        .map(|w| w.slug)
        .filter(|slug| slug.starts_with("orchestrate-the-release"))
        .collect();
    assert_eq!(made, ["orchestrate-the-release"]);
}

/// The uncommitted changes on the fixture's clones' default branches: web's go to the parent of
/// the orchestration that ran on web, docs' to a workspace of their own on a new branch, from the
/// commit docs' clone stands on (one not pushed: the changes apply there). Each clone keeps its
/// copy; a later run carries them no more.
#[tokio::test(flavor = "multi_thread")]
async fn default_branch_changes_are_copied_once_into_a_workspace_the_clone_keeping_them() {
    let _imports = IMPORTS.lock().await;
    let f = importing();
    let d = &f.daemon;
    let docs_clone = f.root.join("clones/docs");
    std::fs::write(docs_clone.join("local.md"), "one\n").unwrap();
    git(&docs_clone, &["add", "local.md"]);
    git(
        &docs_clone,
        &["commit", "-q", "-m", "local", "--", "local.md"],
    );
    std::fs::write(docs_clone.join("local.md"), "two\n").unwrap();

    let plan = import(&f, false).await;
    let read = |path: PathBuf| std::fs::read_to_string(path).unwrap();
    let parent = d.workspaces.get("orchestrate-the-release").unwrap();
    assert_eq!(
        read(Path::new(&parent.path).join("web/README.md")),
        "web, uncommitted on main\n"
    );
    let docs = d.workspaces.get("docs-changes").unwrap();
    let checkout = Path::new(&docs.path).join("docs");
    assert_eq!(
        read(checkout.join("README.md")),
        "docs, uncommitted on main\n"
    );
    assert_eq!(read(checkout.join("local.md")), "two\n");
    assert_eq!(git(&checkout, &["log", "-1", "--format=%s"]), "local");
    assert_ne!(d.store.worktrees_of(&docs.id).unwrap()[0].branch, "main");
    for (clone, file, text) in [
        ("web", "README.md", "web, uncommitted on main\n"),
        ("docs", "README.md", "docs, uncommitted on main\n"),
        ("docs", "local.md", "two\n"),
    ] {
        assert_eq!(read(f.root.join("clones").join(clone).join(file)), text);
    }
    assert!(
        !plan
            .warnings
            .iter()
            .any(|w| w.contains("could not be copied")),
        "{:?}",
        plan.warnings
    );

    // A later session on docs plans its changes again: carried earlier, not again.
    std::fs::write(
        f.claude.join("projects/-code/docs-again.jsonl"),
        line(&docs_clone, "main", &ago(1), r#""message":{"content":[]}"#) + "\n",
    )
    .unwrap();
    let later = import(&f, false).await;
    let carried: Vec<_> = later
        .workspaces
        .iter()
        .filter(|w| !w.changes_from.is_empty())
        .map(|w| (w.outcome, w.workspace.clone()))
        .collect();
    assert_eq!(
        carried,
        [(
            ClaudeCodeOutcome::AlreadyImported,
            Some("docs-changes".to_string())
        )]
    );
    assert!(d.workspaces.get("docs-changes-2").is_err());

    // A new orchestration on web: its parent does not carry web's changes again.
    let web = f.root.join("clones/web");
    let mut lines = Vec::new();
    for name in ["n1", "n2"] {
        let at = f.root.join("wt").join(name);
        git(
            &web,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &format!("feat/{name}"),
                at.to_str().unwrap(),
                "main",
            ],
        );
        commit(&at, &format!("{name}.txt"), "work\n");
        lines.push(line(
            &web,
            "main",
            &ago(1),
            &edit(&at.join(format!("{name}.txt"))),
        ));
    }
    std::fs::write(
        f.claude.join("projects/-code/new-orchestration.jsonl"),
        lines.join("\n") + "\n",
    )
    .unwrap();
    let planned = import(&f, true).await;
    let parent = planned
        .workspaces
        .iter()
        .find(|w| w.kind == ClaudeCodeWorkspaceKind::Parent && w.sessions == ["new-orchestration"])
        .unwrap_or_else(|| panic!("{planned:?}"));
    assert!(parent.changes_from.is_empty(), "{parent:?}");
}

/// With `move_changes`: shop's clone on feat/a (a change, a new file) and a worktree of the user's
/// on feat/b (a change) are released, their changes copied into the workspaces, then saved in a
/// ref each and cleaned: the clone back on main, the worktree detached at it (the clone holds
/// main). A ref brings the changes back; the daily purge deletes it once past the retention.
/// docs, of which Chauffeur has a clone of its own, keeps its branch and its changes: fetched,
/// never released, it is no one's to clean.
#[tokio::test(flavor = "multi_thread")]
async fn move_changes_leaves_one_copy_of_the_work_in_chauffeur() {
    let _imports = IMPORTS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let shop = clone_repo(&root, "shop", &[]);
    let wt_b = root.join("wt/b");
    git(
        &shop,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/b",
            wt_b.to_str().unwrap(),
        ],
    );
    commit(&wt_b, "b.txt", "b\n");
    std::fs::write(wt_b.join("README.md"), "shop, b's change\n").unwrap();
    git(&shop, &["switch", "-q", "-c", "feat/a"]);
    commit(&shop, "a.txt", "a\n");
    std::fs::write(shop.join("README.md"), "shop, a's change\n").unwrap();
    std::fs::write(shop.join("new.txt"), "new\n").unwrap();
    // feat/a checked out a second time, with a file of its own: a ref of its own too.
    let wt_a2 = root.join("wt/a2");
    git(
        &shop,
        &[
            "worktree",
            "add",
            "-q",
            "-f",
            wt_a2.to_str().unwrap(),
            "feat/a",
        ],
    );
    std::fs::write(wt_a2.join("two.txt"), "two\n").unwrap();
    // feat/c with a file git is told to leave alone, changed: left as it is.
    let wt_c = root.join("wt/c");
    git(
        &shop,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/c",
            wt_c.to_str().unwrap(),
            "main",
        ],
    );
    commit(&wt_c, "conf.txt", "source\n");
    git(&wt_c, &["update-index", "--assume-unchanged", "conf.txt"]);
    std::fs::write(wt_c.join("conf.txt"), "local secret\n").unwrap();
    std::fs::write(wt_c.join("README.md"), "shop, c's change\n").unwrap();
    let docs = clone_repo(&root, "docs", &[]);
    git(&docs, &["switch", "-q", "-c", "feat/d"]);
    commit(&docs, "d.txt", "d\n");
    std::fs::write(docs.join("README.md"), "docs, d's change\n").unwrap();
    let own_docs = root.join("repos/docs");
    let docs_origin = root.join("origin/docs.git");
    git(
        &root,
        &[
            "clone",
            "-q",
            docs_origin.to_str().unwrap(),
            own_docs.to_str().unwrap(),
        ],
    );
    let docs_url = "https://github.com/fixture/docs.git";
    git(&own_docs, &["remote", "set-url", "origin", docs_url]);
    git(
        &own_docs,
        &[
            "config",
            &format!("url.{}.insteadOf", docs_origin.display()),
            docs_url,
        ],
    );
    let claude = root.join("claude");
    let projects = claude.join("projects").join("-code");
    std::fs::create_dir_all(&projects).unwrap();
    // The worktree's session the most recent: made first, it does not take main from the clone.
    for (id, folder, branch, days) in [
        ("s-a", &shop, "feat/a", 3),
        ("s-b", &wt_b, "feat/b", 1),
        ("s-d", &docs, "feat/d", 2),
        ("s-c", &wt_c, "feat/c", 2),
    ] {
        std::fs::write(
            projects.join(format!("{id}.jsonl")),
            line(folder, branch, &ago(days), &edit(&folder.join("README.md"))) + "\n",
        )
        .unwrap();
    }
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("settings.toml"),
        "github_org = \"fixture\"\ngithub_sources = false\nbranch_prefix = \"t\"\nclone_roots = []\nport_range_start = 46990\nlinear_enabled = false\n",
    )
    .unwrap();
    let daemon = Daemon::open(Paths::new(
        home,
        root.join("workspaces"),
        root.join("repos"),
    ))
    .unwrap();
    let mut vars: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    vars.insert(
        "CLAUDE_CONFIG_DIR".into(),
        claude.to_string_lossy().into_owned(),
    );
    daemon.shell_env.set_vars(vars);
    daemon.chats.sidecar.use_binary(history_sidecar());
    let run = |dry_run: bool| {
        chauffeurd::claudecode::import(
            &daemon,
            ClaudeCodeImportParams {
                dry_run,
                move_changes: true,
                claude_dir: Some(claude.to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
    };

    // The dry run says which checkouts it would clean, and cleans none.
    let planned = run(true).await.unwrap();
    assert!(planned.move_changes);
    assert_eq!(planned.releases.len(), 4, "{planned:?}");
    assert!(planned.releases.iter().all(|r| r.moved.is_none()));
    assert!(is_branch(&shop, "feat/a"));

    let plan = run(false).await.unwrap();
    let moved = |folder: &Path| {
        plan.releases
            .iter()
            .find(|r| Path::new(&r.folder) == folder)
            .and_then(|r| r.moved.clone())
            .unwrap_or_else(|| panic!("{plan:?}"))
    };
    // The clone, back on main, clean; the worktree detached at main, clean.
    let (a, b) = (moved(&shop), moved(&wt_b));
    assert_eq!(a.back_on, "main");
    assert!(is_branch(&shop, "main"));
    assert!(!is_branch(&wt_b, "feat/b") && !is_branch(&wt_b, "main"));
    assert_eq!(
        git(&wt_b, &["rev-parse", "HEAD"]),
        git(&shop, &["rev-parse", "main"])
    );
    for folder in [&shop, &wt_b] {
        assert_eq!(git(folder, &["status", "--porcelain"]), "", "{folder:?}");
    }
    assert!(is_branch(&docs, "feat/d"));
    // The second checkout of feat/a: saved apart.
    let a2 = moved(&wt_a2);
    assert_ne!(a2.saved_in, a.saved_in);
    assert_eq!(
        git(&shop, &["show", &format!("{}:two.txt", a2.saved_in)]),
        "two"
    );
    // The one git hides changes of: left as it is, said.
    assert!(
        plan.releases
            .iter()
            .any(|r| Path::new(&r.folder) == wt_c && r.moved.is_none())
    );
    assert_eq!(
        std::fs::read_to_string(wt_c.join("conf.txt")).unwrap(),
        "local secret\n"
    );
    assert_eq!(
        std::fs::read_to_string(wt_c.join("README.md")).unwrap(),
        "shop, c's change\n"
    );
    assert!(
        plan.warnings.iter().any(|w| w.contains(&format!(
            "{} is left as it is (some of its files are marked --assume-unchanged",
            wt_c.display()
        ))),
        "{:?}",
        plan.warnings
    );
    assert_eq!(
        std::fs::read_to_string(docs.join("README.md")).unwrap(),
        "docs, d's change\n"
    );
    // The work is in the workspaces, and in the refs.
    for (branch, text) in [
        ("feat/a", "shop, a's change\n"),
        ("feat/b", "shop, b's change\n"),
    ] {
        let slug = plan
            .workspaces
            .iter()
            .find(|w| w.branch.as_deref() == Some(branch))
            .and_then(|w| w.workspace.clone())
            .unwrap();
        let ws = PathBuf::from(daemon.workspaces.get(&slug).unwrap().path);
        assert_eq!(
            std::fs::read_to_string(ws.join("shop/README.md")).unwrap(),
            text
        );
    }
    assert_eq!(
        git(&shop, &["show", &format!("{}:README.md", a.saved_in)]),
        "shop, a's change"
    );
    assert_eq!(
        git(&shop, &["show", &format!("{}:new.txt", a.saved_in)]),
        "new"
    );
    assert_eq!(
        git(&shop, &["show", &format!("{}:README.md", b.saved_in)]),
        "shop, b's change"
    );
    // The command the output gives brings them back as they were, on the branch's commit.
    git(
        &shop,
        &["switch", "-q", "--detach", &format!("{}^", a.saved_in)],
    );
    git(&shop, &["read-tree", "-u", "--reset", &a.saved_in]);
    git(&shop, &["reset", "-q"]);
    assert_eq!(
        git(&shop, &["status", "--porcelain"]),
        "M README.md\n?? new.txt"
    );
    assert_eq!(
        std::fs::read_to_string(shop.join("README.md")).unwrap(),
        "shop, a's change\n"
    );
    // Past the retention, the daily purge deletes the refs.
    daemon
        .settings
        .update(|doc| doc["snapshot_retention_days"] = toml_edit::value(-1))
        .unwrap();
    let exists = |reference: &str| {
        Command::new("git")
            .args(["show-ref", "--verify", "--quiet", reference])
            .current_dir(&shop)
            .status()
            .unwrap()
            .success()
    };
    // One that cannot be deleted then (its folder read only) is tried again at the next purge.
    let a_folder = shop
        .join(".git")
        .join(Path::new(&a.saved_in).parent().unwrap());
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&a_folder, std::fs::Permissions::from_mode(0o500)).unwrap();
    daemon.workspaces.purge_expired_snapshots().await.unwrap();
    std::fs::set_permissions(&a_folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(exists(&a.saved_in));
    assert!(!exists(&b.saved_in));
    daemon.workspaces.purge_expired_snapshots().await.unwrap();
    for reference in [&a.saved_in, &a2.saved_in, &b.saved_in] {
        assert!(!exists(reference), "{reference}");
    }
}
