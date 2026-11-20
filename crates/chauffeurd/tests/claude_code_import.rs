//! The dry run of the import from Claude Code against real git clones and a Claude Code folder of
//! transcripts as Claude Code writes them.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use chauffeur_protocol::claudecode::{
    ClaudeCodeBranch, ClaudeCodeImportParams, ClaudeCodeImportResult, ClaudeCodeOutcome,
    ClaudeCodeRelease,
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
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for name in ["web", "api", "docs"] {
        let seed = root.join("seed").join(name);
        std::fs::create_dir_all(&seed).unwrap();
        git(&seed, &["init", "-q", "-b", "main"]);
        commit(&seed, "README.md", &format!("{name}\n"));
        let origin = root.join("origin").join(format!("{name}.git"));
        git(
            &root,
            &[
                "clone",
                "-q",
                "--bare",
                seed.to_str().unwrap(),
                origin.to_str().unwrap(),
            ],
        );
        let clone = root.join("clones").join(name);
        git(
            &root,
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
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
        "github_org = \"fixture\"\ngithub_sources = false\nbranch_prefix = \"t\"\nclone_roots = []\nport_range_start = 46950\nlinear_enabled = false\n",
    )
    .unwrap();
    // Chauffeur's workspaces folder named through a symlink, as a user may set it.
    std::os::unix::fs::symlink(root.join("workspaces"), root.join("ws-link")).unwrap();
    let daemon = Daemon::open(Paths::new(home, root.join("ws-link"), root.join("repos"))).unwrap();
    let mut vars: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let path = format!("{}:{}", bin.display(), vars["PATH"]);
    vars.insert("PATH".into(), path);
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

    // A live branch checked out in one of the user's worktrees, or another clone of the repo,
    // is released there.
    let release = |branch: &str, folder: &str| ClaudeCodeRelease {
        repo: "web".into(),
        branch: branch.into(),
        folder: f.root.join(folder).to_string_lossy().into_owned(),
    };
    assert_eq!(
        plan.releases,
        [
            release("feat/held", "wt/held"),
            release("feat/two", "clones/web-2")
        ]
    );
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

#[tokio::test(flavor = "multi_thread")]
async fn without_a_dry_run_the_import_is_refused() {
    let f = fixture();
    let error = chauffeurd::claudecode::import(
        &f.daemon,
        ClaudeCodeImportParams {
            claude_dir: Some(f.claude.to_string_lossy().into_owned()),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("--dry-run"), "{error}");
}
