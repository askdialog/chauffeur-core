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
    for name in ["web", "api"] {
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
    std::fs::write(web.join("README.md"), "web, uncommitted on main\n").unwrap();
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

    // A gh that knows one merged PR, from feat/squashed at its tip.
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let gh = bin.join("gh");
    std::fs::write(
        &gh,
        format!("#!/bin/sh\ncase \"$PWD\" in */api) exit 1 ;; esac\ncase \"$*\" in\n  *--state\\ merged*) echo '[{{\"headRefName\":\"feat/squashed\",\"headRefOid\":\"{squashed}\"}}]' ;;\n  *) exit 1 ;;\nesac\n"),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();

    let claude = root.join("claude");
    let projects = claude.join("projects").join("-code");
    std::fs::create_dir_all(&projects).unwrap();
    let api = root.join("clones/api");
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
        vec![line(
            &api,
            "feat/live",
            &ago(4),
            &edit(&api.join("live.txt")),
        )],
    );
    session(
        "away",
        "Away work",
        vec![line(
            &web,
            "feat/away",
            &ago(7),
            &edit(&web.join("away/x.ts")),
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
                "main",
                &ago(10),
                &edit(&root.join("tmp/scratch/x.ts")),
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
    let daemon = Daemon::open(Paths::new(
        home,
        root.join("workspaces"),
        root.join("repos"),
    ))
    .unwrap();
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
    chauffeurd::claudecode::import(
        &f.daemon,
        ClaudeCodeImportParams {
            since_days,
            all,
            dry_run: true,
            claude_dir: Some(f.claude.to_string_lossy().into_owned()),
        },
    )
    .await
    .unwrap()
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
            "history"
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
    assert_eq!(
        session("simple-api").branches,
        [branch("api", "feat/live", true)]
    );
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
    assert_eq!(plan.histories[0].sessions, ["history"]);

    // A live branch checked out in one of the user's worktrees is released there.
    assert_eq!(
        plan.releases,
        [ClaudeCodeRelease {
            repo: "web".into(),
            branch: "feat/held".into(),
            folder: f.root.join("wt/held").to_string_lossy().into_owned()
        }]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_window_widens_with_since_and_all() {
    let f = fixture();
    let ids =
        |plan: ClaudeCodeImportResult| plan.sessions.into_iter().map(|s| s.id).collect::<Vec<_>>();
    assert!(!ids(plan(&f, None, false).await).contains(&"old".to_string()));
    assert!(ids(plan(&f, Some(100), false).await).contains(&"old".to_string()));
    let all = plan(&f, Some(1), true).await;
    assert_eq!(all.since_days, None);
    assert!(ids(all).contains(&"old".to_string()));
    let narrow = ids(plan(&f, Some(5), false).await);
    assert!(
        narrow.contains(&"simple-api".to_string()) && !narrow.contains(&"simple-web".to_string()),
        "{narrow:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_dry_run_the_import_is_refused_until_it_is_switched_on() {
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
