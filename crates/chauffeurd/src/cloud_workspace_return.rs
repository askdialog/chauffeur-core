//! A workspace coming back to a machine that still holds its retired copy: the copy a committed
//! transfer left read-only here, now stale. The returning version replaces it (decided with
//! Justin 2026-10-26): its folder is set aside, the restoration takes its place, its rows are
//! replaced in the same transaction as the import, and only then are the old folder and its Git
//! links removed, and the returning worktrees attached to the user's clones the copy's were
//! linked to. A restoration that fails before puts the old copy back as it was.
//!
//! The copy is read-only for Chauffeur, not for an editor or a terminal. What changed in it since
//! it left refuses the return, named, unless the user asks to keep it: it then stays beside the
//! returning version as `<slug>-local-copy`. Its Git-ignored files (an `.env`, dependencies,
//! build output), which never travel, move into the returning worktrees.
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::Daemon;
use crate::cloud_workspace_discard::NativeFiles;
use crate::cloud_workspace_handoff::{Phase, State};
use crate::cloud_workspace_package::{Contents, differences, observe};

/// What the retired copy was, kept durably while it is set aside so it can be put back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Retired {
    pub workspace_id: String,
    /// The committed state that retired it, as stored in its fence.
    pub fence: String,
    pub path: PathBuf,
    pub state: String,
    pub aside: PathBuf,
    /// Its own `workspace_restores` row when it had arrived by transfer itself.
    pub restore: Option<OldRestore>,
    /// Its worktrees, each with the clone it is linked to, whose records go with it.
    pub worktrees: Vec<(PathBuf, PathBuf)>,
    /// The user asked to keep it: `aside` stays, beside the returning workspace.
    #[serde(default)]
    pub kept: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct OldRestore {
    pub operation_id: String,
    pub snapshot_digest: String,
    pub native_root: String,
    pub receipt: Option<String>,
    pub activated: Option<String>,
    /// What it added to the native homes, as `workspace_restores.native_files` holds it.
    #[serde(default)]
    pub native_files: Option<String>,
}

impl OldRestore {
    pub(crate) fn native_files(&self) -> Option<NativeFiles> {
        serde_json::from_str(self.native_files.as_deref()?).ok()
    }
}

/// Whether `committed`, a workspace's fence here, retired it in favour of `source_peer`: the
/// copy a transfer from that peer may replace. The commit must name this machine as the one it
/// moved from, and the coming transfer must be the very next one of the journal.
pub(crate) fn retires_for(committed: &State, published: Option<&State>, source_peer: &str) -> bool {
    let Some(t) = committed.transfer.as_ref() else {
        return false;
    };
    t.phase == Phase::Committed
        && committed.execution_peer_id == source_peer
        && published.is_none_or(|p| {
            p.binding == committed.binding
                && p.transfer.as_ref().is_some_and(|next| {
                    next.source_peer_id == source_peer
                        && next.target_peer_id == t.source_peer_id
                        && next.source_epoch == committed.epoch
                })
        })
}

/// The committed state retiring workspace `id` here, if any.
pub(crate) fn committed_fence(
    daemon: &Daemon,
    id: &str,
) -> anyhow::Result<Option<(String, State)>> {
    let Some(raw) = daemon.store.workspace_fence(id).ok().flatten() else {
        return Ok(None);
    };
    let state: State = serde_json::from_str(&raw)?;
    state.validate()?;
    Ok(
        (state.transfer.as_ref().map(|t| t.phase) == Some(Phase::Committed))
            .then_some((raw, state)),
    )
}

/// The retired copy the published transfer replaces here; an error when this machine has the
/// workspace in any other state.
pub(crate) fn retired_copy(
    daemon: &Daemon,
    published: &State,
    root: &Path,
    keep: bool,
) -> anyhow::Result<Option<Retired>> {
    let id = &published.binding.workspace_id;
    let Some(row) = daemon.store.find_workspace(id)? else {
        return Ok(None);
    };
    let source = &published
        .transfer
        .as_ref()
        .context("The snapshot has no transfer operation")?
        .source_peer_id;
    let (fence, committed) = committed_fence(daemon, id)?.context(
        "This machine already has the workspace, not as a copy retired by the journal; archive it before receiving it again",
    )?;
    ensure!(
        retires_for(&committed, Some(published), source),
        "This machine's retired copy of the workspace is not the one this transfer follows; reconcile the journal first"
    );
    // Recorded as the workspaces folder was given (through a symlink, say), `root` resolved.
    ensure!(
        Path::new(&row.path).canonicalize().ok().as_deref() == Some(root),
        "The retired copy is not in the folder the workspace comes back to"
    );
    let restore = daemon.store.with(|c| {
        c.query_row(
            "SELECT operation_id,snapshot_digest,native_root,receipt,activated,native_files FROM workspace_restores WHERE workspace_id=?1",
            [id],
            |r| {
                Ok(OldRestore {
                    operation_id: r.get(0)?,
                    snapshot_digest: r.get(1)?,
                    native_root: r.get(2)?,
                    receipt: r.get(3)?,
                    activated: r.get(4)?,
                    native_files: r.get(5)?,
                })
            },
        )
        .optional()
    })?;
    let worktrees = daemon
        .store
        .worktrees_of(id)?
        .into_iter()
        .filter(|w| !w.removed)
        // Resolved as `root` is, so each lies under it.
        .map(|w| {
            let path = PathBuf::from(w.path);
            (
                PathBuf::from(w.clone_path),
                path.canonicalize().unwrap_or(path),
            )
        })
        .collect();
    let operation = &published
        .transfer
        .as_ref()
        .context("No transfer")?
        .operation_id;
    let name = root
        .file_name()
        .and_then(|n| n.to_str())
        .context("Invalid workspace folder")?;
    Ok(Some(Retired {
        workspace_id: id.clone(),
        fence,
        path: root.to_owned(),
        state: row.state.as_str().to_owned(),
        aside: if keep {
            root.with_file_name(format!("{name}-local-copy"))
        } else {
            root.with_file_name(format!(".{name}.replaced-{operation}"))
        },
        restore,
        worktrees,
        kept: keep,
    }))
}

/// What the workspace held when it was captured to move from here.
pub(crate) fn departure(daemon: &Daemon, workspace: &str) -> anyhow::Result<Contents> {
    let raw: Option<String> = daemon.store.with(|c| {
        c.query_row(
            "SELECT contents FROM workspace_departures WHERE workspace_id=?1",
            [workspace],
            |r| r.get(0),
        )
        .optional()
    })?;
    Ok(serde_json::from_str(&raw.context(
        "there is no record of what it held when it moved",
    )?)?)
}

/// The refusal to overwrite a retired copy edited since it left, `problem` saying what changed:
/// its data tells a driver that the same return asked to keep that copy goes on.
pub(crate) fn local_copy_changed(problem: String) -> crate::Error {
    crate::Error::ConflictWith(problem, serde_json::json!({ "local_copy_changed": true }))
}

/// Whether `error`, here or passed on from another daemon, is that refusal.
pub(crate) fn is_local_copy_changed(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<crate::Error>()
        .and_then(|error| error.to_rpc().data)
        .is_some_and(|data| data["local_copy_changed"] == true)
}

/// Why the retired copy of `workspace` may not be replaced: what changed in it since it was
/// captured to move (files, commits, staged changes, notes), which the returning version would
/// overwrite. None when nothing did. `machine` names this one in the message.
pub(crate) async fn unsaved_changes(
    daemon: &Daemon,
    workspace: &str,
    machine: &str,
) -> Option<String> {
    const KEEP: &str = "transfer again keeping the local copy (--keep-local-copy) to have it beside the returning one";
    let changes = async {
        let before = departure(daemon, workspace)?;
        let row = daemon
            .store
            .find_workspace(workspace)?
            .context("the workspace is gone")?;
        let git = crate::git::Git::new(daemon.shell_env.clone());
        let now = observe(&git, Path::new(&row.path), before.repos.keys()).await?;
        anyhow::Ok(differences(&before, &now))
    }
    .await;
    match changes {
        Ok(changes) if changes.is_empty() => None,
        Ok(changes) => {
            let mut named = changes
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            if changes.len() > 5 {
                named.push_str(&format!(" and {} more", changes.len() - 5));
            }
            Some(format!(
                "The copy on {machine} changed since the workspace moved ({named}); {KEEP}"
            ))
        }
        Err(error) => Some(format!(
            "Could not compare the copy on {machine} with what moved ({error:#}); {KEEP}"
        )),
    }
}

/// In the reservation's transaction: the retired row becomes the arriving one (under the
/// transfer's fence and a new restoration row), and what it was is kept to put it back.
pub(crate) fn reserve(
    tx: &rusqlite::Transaction<'_>,
    retired: &Retired,
    operation: &str,
    arriving_fence: &str,
) -> anyhow::Result<()> {
    ensure!(
        tx.execute(
            "UPDATE workspaces SET state='creating',execution_fence=?3 WHERE id=?1 AND execution_fence=?2",
            params![retired.workspace_id, retired.fence, arriving_fence],
        )? == 1,
        "The retired copy changed; retry the transfer"
    );
    tx.execute(
        "DELETE FROM workspace_restores WHERE workspace_id=?1",
        [&retired.workspace_id],
    )?;
    tx.execute(
        "INSERT INTO workspace_returns(operation_id,workspace_id,retired) VALUES(?1,?2,?3)",
        params![
            operation,
            retired.workspace_id,
            serde_json::to_string(retired)?
        ],
    )?;
    Ok(())
}

/// Sets the retired folder aside, out of the way of the arriving one.
pub(crate) fn set_aside(retired: &Retired) -> anyhow::Result<()> {
    ensure!(
        fs::symlink_metadata(&retired.aside).is_err(),
        "{} already exists; move it away, then retry",
        retired.aside.display()
    );
    fs::rename(&retired.path, &retired.aside)?;
    Ok(())
}

/// In the import's transaction: the retired copy's rows give way to the arriving ones.
pub(crate) fn replace_rows(
    tx: &rusqlite::Transaction<'_>,
    manifest: &crate::cloud_workspace_package::Manifest,
    operation: &str,
) -> anyhow::Result<()> {
    let workspace = &manifest.workspace.id;
    // What was renamed or retitled while it ran elsewhere comes back too.
    tx.execute(
        "UPDATE workspaces SET branch=?2,tickets=?3,title=?4,explore=?5 WHERE id=?1",
        params![
            workspace,
            manifest.workspace.branch,
            serde_json::to_string(&manifest.workspace.tickets)?,
            manifest.workspace.title,
            manifest.workspace.explore
        ],
    )?;
    let chats = "SELECT id FROM chats WHERE workspace_id=?1";
    tx.execute(
        &format!("DELETE FROM chat_send_receipts WHERE chat_id IN ({chats})"),
        [workspace],
    )?;
    tx.execute(
        &format!("DELETE FROM chat_queue WHERE chat_id IN ({chats})"),
        [workspace],
    )?;
    tx.execute(
        &format!("DELETE FROM restored_chat_blocks WHERE chat_id IN ({chats})"),
        [workspace],
    )?;
    for table in [
        "events",
        "chats",
        "worktrees",
        "workspace_imported_database_exports",
        "workspace_imported_development_profiles",
        // What the copy held when it left: the returning version is the workspace now.
        "workspace_departures",
        // A deletion of the copy still pending: it is the workspace again.
        "workspace_discards",
        // The retired copy's own database restores: the returning exports are restored afresh.
        "workspace_database_restores",
    ] {
        tx.execute(
            &format!("DELETE FROM {table} WHERE workspace_id=?1"),
            [workspace],
        )?;
    }
    ensure!(
        tx.execute(
            "DELETE FROM workspace_returns WHERE operation_id=?1 AND workspace_id=?2",
            params![operation, workspace],
        )? == 1,
        "The return's record changed during restoration"
    );
    Ok(())
}

/// After the import committed: the old folder and its Git links go. What cannot be removed is
/// left and logged; the workspace already runs from its new folder. The copy is read once more
/// first: changed since the check (an editor still writing there), or holding an ignored file
/// that could not move, it is kept beside the workspace instead, as on request. Returns where a
/// kept copy is.
pub(crate) async fn clean_up(
    daemon: &Daemon,
    retired: &Retired,
    departure: Option<&Contents>,
) -> Option<PathBuf> {
    let git = crate::git::Git::new(daemon.shell_env.clone());
    let mut keep = retired.kept;
    if !keep {
        let unchanged = match departure {
            Some(before) => observe(&git, &retired.aside, before.repos.keys())
                .await
                .is_ok_and(|now| differences(before, &now).is_empty()),
            None => false,
        };
        if !unchanged {
            tracing::warn!(path = %retired.aside.display(), "a replaced copy changed during the return; it is kept");
            keep = true;
        }
    }
    if !carry_ignored(&git, retired).await {
        keep = true;
    }
    // Kept without being asked: under the name a kept copy has, when it is free.
    let mut aside = retired.aside.clone();
    let visible = retired.path.with_file_name(format!(
        "{}-local-copy",
        retired.path.file_name()?.to_string_lossy()
    ));
    if keep && !retired.kept && fs::symlink_metadata(&visible).is_err() {
        match fs::rename(&aside, &visible) {
            Ok(()) => aside = visible,
            Err(error) => {
                tracing::warn!(%error, path = %aside.display(), "could not rename a kept copy")
            }
        }
    }
    // The arriving worktrees sit at the very same paths, linked to other clones: each old record
    // is first pointed at the folder set aside, so pruning after its removal finds it gone.
    for (clone, path) in &retired.worktrees {
        let Ok(relative) = path.strip_prefix(&retired.path) else {
            continue;
        };
        let moved = aside.join(relative);
        if clone.exists()
            && moved.exists()
            && let Some(moved) = moved.to_str()
            && let Err(error) = git.run(clone, &["worktree", "repair", moved]).await
        {
            tracing::warn!(%error, clone = %clone.display(), "could not relink a replaced worktree");
        }
    }
    if keep {
        // Kept for the user, with its worktrees and the clones they are linked to.
        return Some(aside);
    }
    if let Err(error) = fs::remove_dir_all(&aside) {
        tracing::warn!(%error, path = %aside.display(), "could not remove a replaced workspace folder");
    }
    let restorations = daemon.paths.home.join("restorations");
    let mut clones: Vec<&PathBuf> = retired.worktrees.iter().map(|(clone, _)| clone).collect();
    clones.sort();
    clones.dedup();
    for clone in clones {
        if !clone.exists() {
            continue;
        }
        if let Err(error) = git.run(clone, &["worktree", "prune"]).await {
            tracing::warn!(%error, clone = %clone.display(), "could not prune a replaced workspace's worktrees");
        }
    }
    if let Some(old) = &retired.restore {
        let root = PathBuf::from(&old.native_root);
        if root.starts_with(&restorations)
            && let Err(error) = fs::remove_dir_all(&root)
        {
            tracing::warn!(%error, path = %root.display(), "could not remove a replaced restoration folder");
        }
    }
    None
}

/// After a return whose copy is gone: each returning worktree, linked to a private clone of the
/// restoration, is attached to the user's clone the copy's worktree was linked to (for one the
/// copy did not have, the repo's clone here), so that Git, pull requests and archiving work as
/// for a local workspace, and its private clone is removed. The branch's commits, and the
/// staged changes' objects, are fetched into that clone; the worktree's files are not touched.
/// A worktree that cannot be attached stays on its private clone, as before, and is logged.
pub(crate) async fn attach(daemon: &Daemon, retired: &Retired, workspace: &str) {
    let git = crate::git::Git::new(daemon.shell_env.clone());
    let Ok(restorations) = daemon
        .paths
        .home
        .canonicalize()
        .map(|home| home.join("restorations"))
    else {
        return;
    };
    let rows = match daemon.store.worktrees_of(workspace) {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, workspace, "could not list a returned workspace's worktrees");
            return;
        }
    };
    for row in rows.into_iter().filter(|row| !row.removed) {
        let (private, path) = (PathBuf::from(&row.clone_path), PathBuf::from(&row.path));
        let Some(scratch) = private
            .parent()
            .and_then(Path::parent)
            .filter(|_| private.starts_with(&restorations))
            .map(|restoration| restoration.join(format!("attaching-{}", row.id)))
        else {
            continue;
        };
        // The copy's own clone holds the workspace's branch, which the returning one replaces; any
        // other clone may have a branch of that name of its own, only fast-forwarded.
        let copy = retired
            .worktrees
            .iter()
            .find(|(clone, old)| *old == path && !clone.starts_with(&restorations))
            .map(|(clone, _)| clone.clone());
        let (clone, replace) = match copy {
            Some(clone) => (clone, true),
            None => match daemon.repos.find_clone(&row.repo) {
                Some(clone) => (clone, false),
                None => continue,
            },
        };
        if !clone.join(".git").is_dir() {
            continue;
        }
        let lock = daemon.repos.lock_for(&clone);
        let _guard = lock.lock().await;
        let attached = attach_worktree(&git, &path, &clone, replace, &scratch).await;
        let _ = fs::remove_dir_all(&scratch);
        if let Err(error) = attached {
            tracing::warn!(%error, worktree = %path.display(), clone = %clone.display(), "could not attach a returned worktree to the clone here; it stays on its restoration's clone");
            continue;
        }
        let Some(clone_path) = clone.to_str() else {
            continue;
        };
        if let Err(error) = daemon.store.with(|c| {
            c.execute(
                "UPDATE worktrees SET clone_path=?2 WHERE id=?1",
                params![row.id, clone_path],
            )
        }) {
            tracing::warn!(%error, worktree = %path.display(), "could not record a returned worktree's clone");
            continue;
        }
        if let Err(error) = fs::remove_dir_all(&private) {
            tracing::warn!(%error, path = %private.display(), "could not remove a returned worktree's private clone");
        }
    }
}

/// Moves the linked worktree at `path` from its clone to `clone`, its files, index and HEAD (a
/// branch, or detached) as they were. Undone when any step fails: the worktree is linked as
/// before and `clone`'s branch is back where it was.
async fn attach_worktree(
    git: &crate::git::Git,
    path: &Path,
    clone: &Path,
    replace: bool,
    scratch: &Path,
) -> anyhow::Result<()> {
    // Untracked files are left out: which are ignored depends on each clone's own excludes.
    const STATUS: [&str; 4] = ["status", "--porcelain=v1", "-z", "--untracked-files=no"];
    const STAGED: &str = "refs/chauffeur-attach/staged";
    let status = git.run_bytes(path, &STATUS).await?;
    let head = git.rev_parse(path, "HEAD").await?;
    let named = git
        .run_allowing(path, &["symbolic-ref", "-q", "HEAD"], &[1])
        .await?;
    let branch = named.trim().strip_prefix("refs/heads/");
    // Staged changes are in no commit: the index's tree travels as one, its blobs with it.
    let tree = git.run(path, &["write-tree"]).await?;
    let staged = git
        .run(
            path,
            &[
                "-c",
                "user.name=Chauffeur",
                "-c",
                "user.email=chauffeur@localhost",
                "commit-tree",
                &tree,
                "-p",
                &head,
                "-m",
                "staged changes",
            ],
        )
        .await?;
    git.run(path, &["update-ref", STAGED, &staged]).await?;
    let source = git
        .run(
            path,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .await?;
    let index = PathBuf::from(
        git.run(
            path,
            &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        )
        .await?,
    );
    let link = path.join(".git");
    let linked = fs::read(&link)?;
    let before = match branch {
        Some(branch) => git
            .rev_parse(clone, &format!("refs/heads/{branch}"))
            .await
            .ok(),
        None => None,
    };
    let mut admin = None;
    let outcome = async {
        // A worktree folder removed by hand (a kept copy, once compared) still holds its branch.
        git.run(clone, &["worktree", "prune"]).await?;
        let mut refspecs = vec![format!("+{STAGED}:{STAGED}")];
        if let Some(branch) = branch {
            let force = if replace { "+" } else { "" };
            refspecs.push(format!("{force}refs/heads/{branch}:refs/heads/{branch}"));
        }
        let mut fetch = vec![
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-write-fetch-head",
            "--no-recurse-submodules",
            "--end-of-options",
            source.as_str(),
        ];
        fetch.extend(refspecs.iter().map(String::as_str));
        git.run(clone, &fetch).await?;
        // A worktree registered in `clone` at a scratch path, without files, takes the link.
        fs::create_dir_all(scratch)?;
        let name = path.file_name().context("A worktree has no name")?;
        let registered = scratch.join(name);
        let registered = registered.to_str().context("Non-UTF8 worktree path")?;
        let mut add = vec![
            "-c",
            "core.hooksPath=/dev/null",
            "worktree",
            "add",
            "--no-checkout",
        ];
        match branch {
            Some(branch) => add.extend(["--end-of-options", registered, branch]),
            None => add.extend(["--detach", "--end-of-options", registered, head.as_str()]),
        }
        git.run(clone, &add).await?;
        let pointer = fs::read_to_string(Path::new(registered).join(".git"))?;
        let dir = PathBuf::from(
            pointer
                .trim()
                .strip_prefix("gitdir: ")
                .context("A worktree link is unreadable")?,
        );
        admin = Some(dir.clone());
        fs::copy(&index, dir.join("index"))?;
        // Never over a file of the worktree's own that bears that name.
        let next = path.join(".git.attaching");
        std::io::Write::write_all(
            &mut fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&next)?,
            format!("gitdir: {}\n", dir.display()).as_bytes(),
        )?;
        fs::rename(&next, &link)?;
        git.run(
            clone,
            &[
                "worktree",
                "repair",
                path.to_str().context("Non-UTF8 worktree path")?,
            ],
        )
        .await?;
        ensure!(
            git.rev_parse(path, "HEAD").await? == head
                && git.run(path, &["write-tree"]).await? == tree
                && git.run_bytes(path, &STATUS).await? == status
                && git
                    .run_allowing(path, &["symbolic-ref", "-q", "HEAD"], &[1])
                    .await?
                    == named,
            "the worktree reads differently from the clone here"
        );
        anyhow::Ok(())
    }
    .await;
    let _ = git.run(clone, &["update-ref", "-d", STAGED]).await;
    let _ = git.run(path, &["update-ref", "-d", STAGED]).await;
    if outcome.is_err() {
        if fs::read(&link).ok().as_deref() != Some(linked.as_slice()) {
            fs::write(&link, &linked)?;
        }
        if let Some(admin) = admin {
            let _ = fs::remove_dir_all(admin);
        }
        if let Some(branch) = branch {
            let name = format!("refs/heads/{branch}");
            let restored = match &before {
                Some(before) => git.run(clone, &["update-ref", &name, before]).await,
                None => git.run(clone, &["update-ref", "-d", &name]).await,
            };
            if let Err(error) = restored {
                tracing::warn!(%error, clone = %clone.display(), "could not put a branch back after a failed attach");
            }
        }
    }
    outcome
}

/// The retired copy's Git-ignored files (an `.env`, dependencies, build output) never travel:
/// each moves into the returning worktree, unless something is already in its place there.
/// False when one could not move: the copy holding it must not be removed.
async fn carry_ignored(git: &crate::git::Git, retired: &Retired) -> bool {
    let mut carried = true;
    for (_, path) in &retired.worktrees {
        let Ok(relative) = path.strip_prefix(&retired.path) else {
            continue;
        };
        let old = retired.aside.join(relative);
        if !old.is_dir() {
            continue;
        }
        // A worktree that did not come back: its ignored files have nowhere to go.
        if !path.is_dir() {
            carried = false;
            continue;
        }
        let entries = match git.ignored_entries(&old).await {
            Ok(entries) => entries,
            Err(error) => {
                tracing::warn!(%error, path = %old.display(), "could not list a replaced worktree's ignored files");
                carried = false;
                continue;
            }
        };
        for entry in entries {
            let entry = entry.trim_end_matches('/');
            if crate::cloud_workspace_snapshot::valid_path(entry).is_err() {
                continue;
            }
            let (from, to) = (old.join(entry), path.join(entry));
            if fs::symlink_metadata(&to).is_ok() {
                continue;
            }
            let moved = to
                .parent()
                .map_or(Ok(()), fs::create_dir_all)
                .and_then(|()| fs::rename(&from, &to));
            if let Err(error) = moved {
                tracing::warn!(%error, path = %from.display(), "could not carry an ignored file into the returning workspace");
                carried = false;
            }
        }
    }
    carried
}

/// A restoration that failed before its import: what it made of the folder is kept aside as
/// evidence, and the retired copy comes back as it was, folder, row and restoration record.
/// `moved` says whether the copy was set aside; if not, its folder never left and stays.
pub(crate) fn put_back(
    daemon: &Daemon,
    retired: &Retired,
    operation: &str,
    moved: bool,
) -> anyhow::Result<()> {
    if moved && fs::symlink_metadata(&retired.path).is_ok() {
        let name = retired
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .context("Invalid workspace folder")?;
        fs::rename(
            &retired.path,
            retired
                .path
                .with_file_name(format!(".{name}.failed-{operation}")),
        )?;
    }
    if moved && fs::symlink_metadata(&retired.aside).is_ok() {
        fs::rename(&retired.aside, &retired.path)?;
    }
    // The conversation files the attempt added go, as a refusal of the sidecar takes them back;
    // those of the copy itself stay.
    let added: Option<String> = daemon.store.with(|c| {
        c.query_row(
            "SELECT native_files FROM workspace_restores WHERE workspace_id=?1 AND operation_id=?2",
            params![retired.workspace_id, operation],
            |r| r.get(0),
        )
        .optional()
        .map(Option::flatten)
    })?;
    if let Some(added) = added.and_then(|raw| serde_json::from_str::<NativeFiles>(&raw).ok()) {
        let old = retired.restore.as_ref().and_then(OldRestore::native_files);
        if let Err(error) = added.without(old.as_ref()).remove() {
            tracing::warn!(%error, "could not remove the conversation files a failed return added");
        }
    }
    // The attempt's private folder is kept as evidence, out of the way of a retry.
    let attempt = daemon.paths.home.join("restorations").join(operation);
    if fs::symlink_metadata(&attempt).is_ok() {
        fs::rename(
            &attempt,
            attempt.with_file_name(format!("{operation}.failed")),
        )?;
    }
    daemon.store.durable(|c| {
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE workspaces SET state=?2,execution_fence=?3,last_error=NULL WHERE id=?1",
            params![retired.workspace_id, retired.state, retired.fence],
        )?;
        tx.execute("DELETE FROM workspace_restores WHERE workspace_id=?1", [&retired.workspace_id])?;
        if let Some(old) = &retired.restore {
            tx.execute(
                "INSERT INTO workspace_restores(workspace_id,operation_id,snapshot_digest,native_root,receipt,activated,native_files) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![retired.workspace_id, old.operation_id, old.snapshot_digest, old.native_root, old.receipt, old.activated, old.native_files],
            )?;
        }
        tx.execute(
            "DELETE FROM workspace_returns WHERE operation_id=?1",
            [operation],
        )?;
        tx.commit()?;
        Ok(())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_workspace_handoff::{Binding, Transfer};

    fn peer(c: char) -> String {
        format!("12D3KooW{}", c.to_string().repeat(44))
    }

    fn committed() -> State {
        State {
            version: 1,
            binding: Binding {
                workspace_id: uuid::Uuid::now_v7().to_string(),
                owner_peer_id: peer('1'),
                controller_peer_id: peer('2'),
            },
            revision: 7,
            epoch: 2,
            execution_peer_id: peer('3'),
            transfer: Some(Transfer {
                operation_id: uuid::Uuid::now_v7().to_string(),
                source_peer_id: peer('1'),
                target_peer_id: peer('3'),
                source_epoch: 1,
                phase: Phase::Committed,
                source_stop_digest: None,
                snapshot_digest: None,
                destination_receipt_digest: None,
            }),
        }
    }

    #[test]
    fn only_the_next_transfer_from_the_new_owner_back_here_replaces_a_retired_copy() {
        let committed = committed();
        let mut next = committed.clone();
        next.revision = 9;
        next.transfer = Some(Transfer {
            operation_id: uuid::Uuid::now_v7().to_string(),
            source_peer_id: peer('3'),
            target_peer_id: peer('1'),
            source_epoch: 2,
            phase: Phase::SnapshotReady,
            source_stop_digest: None,
            snapshot_digest: None,
            destination_receipt_digest: None,
        });
        assert!(retires_for(&committed, None, &peer('3')));
        assert!(retires_for(&committed, Some(&next), &peer('3')));
        assert!(
            !retires_for(&committed, Some(&next), &peer('4')),
            "another source"
        );
        let mut later = next.clone();
        later.transfer.as_mut().unwrap().source_epoch = 3;
        assert!(
            !retires_for(&committed, Some(&later), &peer('3')),
            "a later generation"
        );
        let mut elsewhere = next.clone();
        elsewhere.transfer.as_mut().unwrap().target_peer_id = peer('4');
        assert!(
            !retires_for(&committed, Some(&elsewhere), &peer('3')),
            "not back here"
        );
        let mut cancelled = committed.clone();
        cancelled.transfer.as_mut().unwrap().phase = Phase::Cancelled;
        assert!(!retires_for(&cancelled, None, &peer('3')));
    }

    #[test]
    fn an_edited_copy_is_told_apart_here_and_from_another_daemon_whatever_the_context() {
        let here = anyhow::Error::from(local_copy_changed("a.txt changed".into()));
        assert!(is_local_copy_changed(&here.context("restoring")));
        // Passed through a /peer answer: the driver keeps the other daemon's error whole.
        let remote = anyhow::Error::from(crate::Error::Remote(
            local_copy_changed("a.txt changed".into()).to_rpc(),
        ))
        .context("the destination could not restore it");
        assert!(is_local_copy_changed(&remote));
        let other = anyhow::Error::from(crate::Error::Conflict("busy".into()));
        assert!(!is_local_copy_changed(&other));
        assert!(!is_local_copy_changed(&anyhow::anyhow!("a.txt changed")));
    }
}
