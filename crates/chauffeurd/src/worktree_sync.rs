//! Keeps the store's worktrees in step with what git says of a workspace: the branch each
//! worktree is on (a branch cut with `git switch -c` in a worktree is the one its PR is looked up
//! under), and the worktrees git has inside the workspace's folder that nobody registered (made
//! with a plain `git worktree add`), which archiving would remove all the same. It never changes
//! git: it only reads, and changes the store.
//!
//! A worktree outside the workspace's folder is the user's own and is left alone, and so is an
//! exploration, whose checkouts are detached on purpose.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chauffeur_protocol::model::{WorkspaceState, WorktreeOrigin};
use tokio::sync::Mutex;

use crate::git::Git;
use crate::paths::AGENT_WORKTREES_DIR;
use crate::store::{Store, WorkspaceRow, WorktreeRow, now};
use crate::{Daemon, Result};

/// What a pass changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Synced {
    /// A worktree found on another branch than the one the store had: repo, from, to.
    pub switched: Vec<(String, String, String)>,
    /// A worktree git had in the workspace's folder, now registered: its path.
    pub adopted: Vec<String>,
}

impl Synced {
    pub fn is_empty(&self) -> bool {
        self.switched.is_empty() && self.adopted.is_empty()
    }
}

/// Reads the workspace's worktrees from git and brings the store in step; the app is told when
/// something changed. A failure leaves the store as it was, and the caller goes on with it.
pub async fn sync(daemon: &Arc<Daemon>, workspace: &str) -> Result<()> {
    let row = daemon.workspaces.row(workspace)?;
    if row.explore || row.state != WorkspaceState::Ready {
        return Ok(());
    }
    let git = Git::new(daemon.shell_env.clone());
    let changed = sync_workspace(&daemon.store, &git, &row, |clone| {
        daemon.repos.lock_for(clone)
    })
    .await?;
    for (repo, from, to) in &changed.switched {
        tracing::info!(workspace = %row.id, %repo, %from, %to, "a worktree is on another branch");
    }
    for path in &changed.adopted {
        daemon.workspaces.warn(
            &row.id,
            format!(
                "{path} was made without Chauffeur: it is now one of this workspace's worktrees"
            ),
        );
    }
    if !changed.is_empty() {
        daemon.workspaces.publish(&row.id);
    }
    Ok(())
}

pub async fn sync_workspace(
    store: &Store,
    git: &Git,
    row: &WorkspaceRow,
    lock_for: impl Fn(&Path) -> Arc<Mutex<()>>,
) -> Result<Synced> {
    let mut synced = Synced::default();
    follow_branches(store, git, row, &mut synced).await?;
    adopt_unknown(store, git, row, &lock_for, &mut synced).await?;
    Ok(synced)
}

/// Each worktree's branch is the one its HEAD is on. A detached HEAD (a rebase under way, say)
/// leaves what the store has.
async fn follow_branches(
    store: &Store,
    git: &Git,
    row: &WorkspaceRow,
    synced: &mut Synced,
) -> Result<()> {
    for worktree in store.worktrees_of(&row.id)? {
        let dir = Path::new(&worktree.path);
        if worktree.removed || !dir.exists() {
            continue;
        }
        let head = git
            .run_allowing(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"], &[1])
            .await
            .ok()
            .map(|branch| branch.trim().to_string())
            .filter(|branch| !branch.is_empty());
        if let Some(branch) = head.filter(|branch| *branch != worktree.branch) {
            store.set_worktree_branch(&worktree.id, &branch)?;
            synced
                .switched
                .push((worktree.repo.clone(), worktree.branch.clone(), branch));
        }
    }
    Ok(())
}

/// A worktree of one of the workspace's repos that git has inside the workspace's folder and the
/// store does not know is registered as an agent's would be: no ports, no setup script, nothing
/// that a creation through Chauffeur does, only the tracking and the cleanup.
async fn adopt_unknown(
    store: &Store,
    git: &Git,
    row: &WorkspaceRow,
    lock_for: &impl Fn(&Path) -> Arc<Mutex<()>>,
    synced: &mut Synced,
) -> Result<()> {
    let folder = canonical(Path::new(&row.path));
    // One template per clone: what a worktree of that repo is named and cut from.
    let mut clones: BTreeMap<String, WorktreeRow> = BTreeMap::new();
    for worktree in store.worktrees_of(&row.id)? {
        if !worktree.removed && worktree.origin == WorktreeOrigin::Workspace {
            clones
                .entry(worktree.clone_path.clone())
                .or_insert(worktree);
        }
    }
    for (clone, template) in clones {
        // The lock a creation holds, so that a worktree git has made but the store has not yet
        // recorded is not mistaken for an unknown one.
        let lock = lock_for(Path::new(&clone));
        let _guard = lock.lock().await;
        let known: HashSet<PathBuf> = store
            .worktrees_of(&row.id)?
            .iter()
            .filter(|w| !w.removed)
            .map(|w| canonical(Path::new(&w.path)))
            .collect();
        let listed = match git.worktree_list(Path::new(&clone)).await {
            Ok(listed) => listed,
            Err(error) => {
                tracing::warn!(%clone, "could not list the worktrees: {error:#}");
                continue;
            }
        };
        for entry in listed {
            let path = canonical(&entry.path);
            if entry.prunable
                || path == canonical(Path::new(&clone))
                || !path.starts_with(&folder)
                || known.contains(&path)
            {
                continue;
            }
            store.insert_worktree(&WorktreeRow {
                id: uuid::Uuid::now_v7().to_string(),
                workspace_id: row.id.clone(),
                repo: template.repo.clone(),
                clone_path: clone.clone(),
                path: entry.path.to_string_lossy().into_owned(),
                branch: entry.branch.unwrap_or_default(),
                base_branch: template.base_branch.clone(),
                base_commit: None,
                origin: WorktreeOrigin::Agent,
                group: Some(group_of(&folder, &path)),
                removed: false,
                snapshot_ref: None,
                created_at: now(),
            })?;
            synced
                .adopted
                .push(entry.path.to_string_lossy().into_owned());
        }
    }
    Ok(())
}

/// The group an adopted worktree belongs to: its folder under `.worktrees/` when it is where
/// `chauffeur worktree add` would have put it, else the name of its own folder.
fn group_of(workspace: &Path, worktree: &Path) -> String {
    let agents = workspace.join(AGENT_WORKTREES_DIR);
    worktree
        .strip_prefix(&agents)
        .ok()
        .and_then(|inside| inside.components().next())
        .or_else(|| worktree.components().next_back())
        .map(|name| name.as_os_str().to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A path as git reports it (symlinks resolved), so that two spellings of one folder compare equal.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shellenv::ShellEnv;
    use std::process::Command;

    fn git_in(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// A clone with one commit, and a workspace whose folder holds a worktree of it on `ws-branch`.
    struct Fixture {
        _root: tempfile::TempDir,
        clone: PathBuf,
        folder: PathBuf,
        store: Store,
        row: WorkspaceRow,
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        let clone = base.join("clone");
        let folder = base.join("workspace");
        std::fs::create_dir_all(&clone).unwrap();
        std::fs::create_dir_all(&folder).unwrap();
        git_in(&clone, &["init", "-q", "-b", "main"]);
        git_in(&clone, &["commit", "-q", "--allow-empty", "-m", "first"]);
        let own = folder.join("clone");
        git_in(
            &clone,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "ws-branch",
                own.to_str().unwrap(),
            ],
        );
        let store = Store::open_in_memory().unwrap();
        let row = WorkspaceRow {
            id: "w1".into(),
            slug: "w1".into(),
            path: folder.to_string_lossy().into_owned(),
            branch: "ws-branch".into(),
            parent_id: None,
            state: WorkspaceState::Ready,
            tickets: Vec::new(),
            created_at: now(),
            archived_at: None,
            last_error: None,
            attention_at: None,
            seen_at: None,
            explore: false,
            title: None,
            auto_title: false,
            title_source: None,
            pinned: false,
            position: 0.0,
            section_id: None,
            house_position: None,
        };
        store.insert_workspace(&row).unwrap();
        store
            .insert_worktree(&WorktreeRow {
                id: "t1".into(),
                workspace_id: "w1".into(),
                repo: "clone".into(),
                clone_path: clone.to_string_lossy().into_owned(),
                path: own.to_string_lossy().into_owned(),
                branch: "ws-branch".into(),
                base_branch: "main".into(),
                base_commit: None,
                origin: WorktreeOrigin::Workspace,
                group: None,
                removed: false,
                snapshot_ref: None,
                created_at: now(),
            })
            .unwrap();
        Fixture {
            _root: root,
            clone,
            folder,
            store,
            row,
        }
    }

    async fn sync_it(f: &Fixture) -> Synced {
        let git = Git::new(ShellEnv::new());
        sync_workspace(&f.store, &git, &f.row, |_| Arc::new(Mutex::new(())))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_branch_cut_in_a_worktree_becomes_the_one_the_workspace_follows() {
        let f = fixture();
        git_in(
            &f.folder.join("clone"),
            &["switch", "-q", "-c", "follow-up"],
        );
        let synced = sync_it(&f).await;
        assert_eq!(
            synced.switched,
            [(
                "clone".to_string(),
                "ws-branch".to_string(),
                "follow-up".to_string()
            )]
        );
        assert_eq!(f.store.worktrees_of("w1").unwrap()[0].branch, "follow-up");
        assert!(
            sync_it(&f).await.is_empty(),
            "a second pass finds nothing to do"
        );
    }

    #[tokio::test]
    async fn a_detached_head_leaves_the_branch_the_store_has() {
        let f = fixture();
        git_in(&f.folder.join("clone"), &["switch", "-q", "--detach"]);
        assert!(sync_it(&f).await.is_empty());
        assert_eq!(f.store.worktrees_of("w1").unwrap()[0].branch, "ws-branch");
    }

    #[tokio::test]
    async fn a_worktree_made_with_plain_git_in_the_workspace_folder_is_adopted_once() {
        let f = fixture();
        let made = f
            .folder
            .join(AGENT_WORKTREES_DIR)
            .join("by-hand")
            .join("clone");
        std::fs::create_dir_all(made.parent().unwrap()).unwrap();
        git_in(
            &f.clone,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "by-hand",
                made.to_str().unwrap(),
            ],
        );
        let synced = sync_it(&f).await;
        assert_eq!(synced.adopted, [made.to_string_lossy().into_owned()]);
        let adopted: Vec<_> = f
            .store
            .worktrees_of("w1")
            .unwrap()
            .into_iter()
            .filter(|w| w.origin == WorktreeOrigin::Agent)
            .collect();
        assert_eq!(adopted.len(), 1);
        assert_eq!(adopted[0].repo, "clone");
        assert_eq!(adopted[0].branch, "by-hand");
        assert_eq!(adopted[0].base_branch, "main");
        assert_eq!(adopted[0].group.as_deref(), Some("by-hand"));
        assert!(sync_it(&f).await.is_empty(), "it is registered once");
    }

    #[tokio::test]
    async fn a_worktree_outside_the_workspace_folder_is_left_alone() {
        let f = fixture();
        let elsewhere = f.folder.parent().unwrap().join("mine");
        git_in(
            &f.clone,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "mine",
                elsewhere.to_str().unwrap(),
            ],
        );
        assert!(sync_it(&f).await.is_empty());
        assert_eq!(f.store.worktrees_of("w1").unwrap().len(), 1);
    }

    #[test]
    fn an_adopted_worktree_is_grouped_by_its_folder_under_the_agents_directory() {
        let workspace = Path::new("/ws");
        assert_eq!(
            group_of(workspace, Path::new("/ws/.worktrees/a/clone")),
            "a"
        );
        assert_eq!(
            group_of(workspace, Path::new("/ws/somewhere/clone")),
            "clone"
        );
    }
}
