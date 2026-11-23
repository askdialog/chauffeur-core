//! The import from Conductor against real git clones shared by Conductor and Chauffeur (as on a
//! Mac where both use the same clones) and a database of Conductor 0.93's schema.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use chauffeur_protocol::conductor::{ConductorClone, ConductorImportParams, ConductorOutcome};
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

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    db: PathBuf,
    daemon: Arc<Daemon>,
}

fn fixture() -> Fixture {
    fixture_on(false)
}

/// Clones `alpha` and `beta` (origin `github.com/fixture/<name>`, fetched from a local bare repo)
/// that Chauffeur already knows; Conductor's `lima` on alpha's `fix-1-pricing` with uncommitted
/// work and `oslo` on `feat/linked` with beta as an /add-dir folder. With `gitlab`, the
/// installation is set to `gitlab.example.com`, main group `team`: alpha is `team/alpha` there,
/// beta `team/shop/beta`, and a third clone, `gamma`, is on GitHub.
fn fixture_on(gitlab: bool) -> Fixture {
    let url_of = |name: &str| match (gitlab, name) {
        (false, _) | (true, "gamma") => format!("https://github.com/fixture/{name}.git"),
        (true, "beta") => "https://gitlab.example.com/team/shop/beta.git".to_string(),
        (true, _) => format!("https://gitlab.example.com/team/{name}.git"),
    };
    let names: &[&str] = if gitlab {
        &["alpha", "beta", "gamma"]
    } else {
        &["alpha", "beta"]
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for &name in names {
        let seed = root.join("seed").join(name);
        std::fs::create_dir_all(&seed).unwrap();
        git(&seed, &["init", "-q", "-b", "main"]);
        std::fs::write(seed.join("README.md"), format!("{name}\n")).unwrap();
        git(&seed, &["add", "-A"]);
        git(&seed, &["commit", "-q", "-m", "init"]);
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
        let url = url_of(name);
        git(&clone, &["remote", "set-url", "origin", &url]);
        git(
            &clone,
            &[
                "config",
                &format!("url.{}.insteadOf", origin.display()),
                &url,
            ],
        );
    }
    let alpha = root.join("clones/alpha");
    let lima = root.join("conductor/lima");
    let oslo = root.join("conductor/oslo");
    git(
        &alpha,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "fix-1-pricing",
            lima.to_str().unwrap(),
        ],
    );
    std::fs::write(lima.join("README.md"), "edited\n").unwrap();
    std::fs::write(lima.join("new.txt"), "untracked\n").unwrap();
    git(
        &alpha,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/linked",
            oslo.to_str().unwrap(),
        ],
    );

    let db = root.join("conductor.db");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(include_str!("fixtures/conductor-0.93.sql"))
        .unwrap();
    conn.execute_batch(&format!(
        "INSERT INTO repos (id, name, remote_url, root_path) VALUES
           ('r-alpha', 'alpha', '{alpha_url}', '{alpha}'),
           ('r-beta', 'beta', '{beta_url}', '{beta}');
         INSERT INTO workspaces (local_id, repository_id, directory_name, workspace_name, branch, state, workspace_path, linked_directory_paths)
         VALUES ('w-lima', 'r-alpha', 'lima', 'Pricing filters', 'fix-1-pricing', 'ready', '{lima}', NULL),
                ('w-oslo', 'r-alpha', 'oslo', 'Linked work', 'feat/linked', 'ready', '{oslo}', '[\"{beta}\"]');
         INSERT INTO sessions (id, workspace_id, agent_type, title, created_at)
         VALUES ('s-1', 'w-lima', 'claude', 'Price filter', '2026-10-02T05:37:21.545Z');
         INSERT INTO session_messages (id, session_id, role, content, created_at)
         VALUES ('m-1', 's-1', 'user', 'Add a price filter', '2026-10-02T05:37:21.545Z');",
        alpha = alpha.display(),
        beta = root.join("clones/beta").display(),
        // As Conductor records them: SSH for alpha, HTTPS without `.git` for beta.
        alpha_url = url_of("alpha")
            .replace("https://", "git@")
            .replacen(".com/", ".com:", 1),
        beta_url = url_of("beta").trim_end_matches(".git"),
        lima = lima.display(),
        oslo = oslo.display(),
    ))
    .unwrap();
    drop(conn);

    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("settings.toml"),
        format!(
            "github_org = \"fixture\"\ngithub_sources = false\nbranch_prefix = \"t\"\nclone_roots = [\"{}\"]\nport_range_start = 46900\nlinear_enabled = false\n{}",
            root.join("clones").display(),
            if gitlab {
                "forge = \"gitlab\"\ngitlab_host = \"gitlab.example.com\"\ngitlab_group = \"team\"\n"
            } else {
                ""
            }
        ),
    )
    .unwrap();
    let paths = Paths::new(home, root.join("workspaces"), root.join("repos"));
    Fixture {
        daemon: Daemon::open(paths).unwrap(),
        _dir: dir,
        root,
        db,
    }
}

fn params(f: &Fixture, dry_run: bool, workspaces: &[&str]) -> ConductorImportParams {
    ConductorImportParams {
        db: Some(f.db.to_string_lossy().into_owned()),
        workspaces: workspaces.iter().map(|w| w.to_string()).collect(),
        include_archived: false,
        dry_run,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_workspace_on_a_shared_clone_is_copied_and_its_branch_released_in_conductor() {
    let f = fixture();
    let lima = f.root.join("conductor/lima");
    let head = git(&lima, &["rev-parse", "HEAD"]);

    let plan = chauffeurd::conductor::import(&f.daemon, params(&f, true, &["Pricing filters"]))
        .await
        .unwrap();
    assert_eq!(plan.repos[0].clone, ConductorClone::Shared);
    assert_eq!(plan.repos[0].name.as_deref(), Some("alpha"));
    let planned = &plan.workspaces[0];
    assert_eq!(planned.outcome, ConductorOutcome::Import);
    assert!(planned.release);
    assert_eq!(planned.changes, 2);
    assert_eq!(
        git(&lima, &["symbolic-ref", "--short", "HEAD"]),
        "fix-1-pricing"
    );

    let done = chauffeurd::conductor::import(&f.daemon, params(&f, false, &["Pricing filters"]))
        .await
        .unwrap();
    let imported = &done.workspaces[0];
    assert_eq!(imported.outcome, ConductorOutcome::Imported, "{imported:?}");
    let copy = f.root.join("workspaces/pricing-filters/alpha");
    assert_eq!(git(&copy, &["branch", "--show-current"]), "fix-1-pricing");
    assert_eq!(
        std::fs::read_to_string(copy.join("README.md")).unwrap(),
        "edited\n"
    );
    assert_eq!(
        std::fs::read_to_string(copy.join("new.txt")).unwrap(),
        "untracked\n"
    );
    assert!(
        f.root
            .join("workspaces/pricing-filters/.chauffeur/conductor/Price filter.md")
            .is_file()
    );
    // Conductor's worktree: detached on the same commit, its work untouched.
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&lima)
            .args(["symbolic-ref", "-q", "HEAD"])
            .status()
            .unwrap()
            .code()
            == Some(1)
    );
    assert_eq!(git(&lima, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        std::fs::read_to_string(lima.join("README.md")).unwrap(),
        "edited\n"
    );
    assert!(lima.join("new.txt").is_file());

    let again = chauffeurd::conductor::import(&f.daemon, params(&f, false, &[]))
        .await
        .unwrap();
    let lima_again = again
        .workspaces
        .iter()
        .find(|w| w.conductor_id == "w-lima")
        .unwrap();
    assert_eq!(lima_again.outcome, ConductorOutcome::AlreadyImported);
    assert_eq!(lima_again.workspace.as_deref(), Some("pricing-filters"));
}

#[tokio::test(flavor = "multi_thread")]
async fn on_a_gitlab_clones_are_matched_by_their_project_on_its_host() {
    let f = fixture_on(true);
    let gamma = f.root.join("clones/gamma");
    let conn = rusqlite::Connection::open(&f.db).unwrap();
    conn.execute(
        "INSERT INTO repos (id, name, remote_url, root_path) VALUES ('r-gamma', 'gamma', 'https://github.com/fixture/gamma.git', ?1)",
        [gamma.to_string_lossy()],
    )
    .unwrap();
    drop(conn);

    let plan = chauffeurd::conductor::import(&f.daemon, params(&f, true, &[]))
        .await
        .unwrap();
    let gamma = plan.repos.iter().find(|r| r.conductor_name == "gamma");
    assert!(gamma.unwrap().skipped.is_some(), "{gamma:?}");
    // Conductor's record and an /add-dir clone, both read on the GitLab, named as the picker does.
    let linked = plan
        .workspaces
        .iter()
        .find(|w| w.name == "Linked work")
        .unwrap();
    assert_eq!(linked.repos, ["alpha", "team/shop/beta"], "{linked:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_creation_that_fails_gives_conductor_its_branch_back() {
    let f = fixture();
    // Beta's clone holds the branch elsewhere: Chauffeur refuses to create the workspace.
    let elsewhere = f.root.join("elsewhere");
    git(
        &f.root.join("clones/beta"),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/linked",
            elsewhere.to_str().unwrap(),
        ],
    );
    let oslo = f.root.join("conductor/oslo");
    let done = chauffeurd::conductor::import(&f.daemon, params(&f, false, &["oslo"]))
        .await
        .unwrap();
    let failed = &done.workspaces[0];
    assert_eq!(failed.repos, ["alpha", "beta"]);
    assert_eq!(failed.outcome, ConductorOutcome::Failed, "{failed:?}");
    assert_eq!(
        git(&oslo, &["symbolic-ref", "--short", "HEAD"]),
        "feat/linked"
    );
    assert!(!f.root.join("workspaces/linked-work").exists());
}
