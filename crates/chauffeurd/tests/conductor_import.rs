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

/// Clones `alpha` and `beta` (origin `github.com/fixture/<name>`, fetched from a local bare repo)
/// that Chauffeur already knows; Conductor's `lima` on alpha's `fix-1-pricing` with uncommitted
/// work and `oslo` on `feat/linked` with beta as an /add-dir folder.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for name in ["alpha", "beta"] {
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
           ('r-alpha', 'alpha', 'git@github.com:fixture/alpha.git', '{alpha}'),
           ('r-beta', 'beta', 'https://github.com/fixture/beta', '{beta}');
         INSERT INTO workspaces (local_id, repository_id, directory_name, workspace_name, branch, state, workspace_path, linked_directory_paths)
         VALUES ('w-lima', 'r-alpha', 'lima', 'Pricing filters', 'fix-1-pricing', 'ready', '{lima}', NULL),
                ('w-oslo', 'r-alpha', 'oslo', 'Linked work', 'feat/linked', 'ready', '{oslo}', '[\"{beta}\"]');
         INSERT INTO sessions (id, workspace_id, agent_type, title, created_at)
         VALUES ('s-1', 'w-lima', 'claude', 'Price filter', '2026-10-02T05:37:21.545Z');
         INSERT INTO session_messages (id, session_id, role, content, created_at)
         VALUES ('m-1', 's-1', 'user', 'Add a price filter', '2026-10-02T05:37:21.545Z');",
        alpha = alpha.display(),
        beta = root.join("clones/beta").display(),
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
            "github_org = \"fixture\"\ngithub_sources = false\nbranch_prefix = \"t\"\nclone_roots = [\"{}\"]\nport_range_start = 46900\nlinear_enabled = false\n",
            root.join("clones").display()
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
