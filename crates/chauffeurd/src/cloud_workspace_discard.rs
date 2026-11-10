//! What a machine keeps of a workspace once it moved away for good. After a verified return, the
//! machine the workspace left deletes its retired copy (approved scope: "after verified return,
//! delete the cloud workspace"): folder, Git links, restoration folder, the conversation files its
//! restorations wrote into the native homes (those only) and rows. A deletion that
//! fails stays pending and is retried; the workspace already runs where it went, so nothing waits
//! for it. Separately, both sides of a committed transfer remove its packages and archives, which
//! the restoration made redundant.
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, ensure};
use chauffeur_protocol::events::RegistryEvent;
use chauffeur_protocol::sidecar::ContextRestoreResult;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::cloud_workspace_handoff::{Phase, State};
use crate::{Daemon, Error, Result};

/// How often pending deletions are tried again.
const RETRY_EVERY: Duration = Duration::from_secs(10 * 60);

/// Tables holding a workspace's rows, by `workspace_id`; chats' own rows go first.
const WORKSPACE_TABLES: [&str; 20] = [
    "events",
    "chats",
    "worktrees",
    "port_blocks",
    "workspace_agent_launches",
    "workspace_database_exports",
    "workspace_database_restores",
    "workspace_database_stops",
    "workspace_departures",
    "workspace_development_profile_heads",
    "workspace_development_profiles",
    "workspace_imported_database_exports",
    "workspace_imported_development_profiles",
    "workspace_preparation_attempts",
    "workspace_process_sweeps",
    "workspace_restores",
    "workspace_returns",
    "workspace_service_stops",
    "workspace_source_stops",
    "workspace_stop_attempts",
];

/// What the restorations of a copy added to this machine's native homes (`~/.claude/projects`,
/// `~/.codex/sessions`), recorded with its restoration: deleting the copy removes exactly that.
/// A file that was already there, kept or extended, is the machine's own and never listed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NativeFiles {
    /// The homes it wrote into, resolved: nothing outside them is ever removed.
    pub homes: Vec<String>,
    pub files: Vec<String>,
    /// Outermost first; removed only once empty.
    pub folders: Vec<String>,
}

impl NativeFiles {
    pub(crate) fn new(homes: [String; 2], added: ContextRestoreResult) -> NativeFiles {
        NativeFiles {
            homes: homes.into(),
            files: added.files,
            folders: added.folders,
        }
    }

    /// These and what `old`, the restoration of the copy they replace, had added.
    pub(crate) fn merged(mut self, old: Option<&NativeFiles>) -> NativeFiles {
        if let Some(old) = old {
            for (mine, theirs) in [
                (&mut self.homes, &old.homes),
                (&mut self.files, &old.files),
                (&mut self.folders, &old.folders),
            ] {
                let new = std::mem::replace(mine, theirs.clone());
                for path in new {
                    if !mine.contains(&path) {
                        mine.push(path);
                    }
                }
            }
        }
        self
    }

    /// These but for what `old` lists too.
    pub(crate) fn without(&self, old: Option<&NativeFiles>) -> NativeFiles {
        let kept = |paths: &[String], theirs: Option<&Vec<String>>| {
            paths
                .iter()
                .filter(|path| theirs.is_none_or(|theirs| !theirs.contains(path)))
                .cloned()
                .collect()
        };
        NativeFiles {
            homes: self.homes.clone(),
            files: kept(&self.files, old.map(|old| &old.files)),
            folders: kept(&self.folders, old.map(|old| &old.folders)),
        }
    }

    /// Removes each file, then each folder left empty. Only a regular file inside one of the
    /// homes, reached through no symlink: anything else now at that path is not what the
    /// restoration wrote, and stays. One already gone is fine.
    pub(crate) fn remove(&self) -> anyhow::Result<()> {
        let inside = |path: &Path| {
            path.components()
                .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
                && self
                    .homes
                    .iter()
                    .any(|home| path.starts_with(home) && path != Path::new(home))
        };
        for file in &self.files {
            let path = Path::new(file);
            if !inside(path) {
                tracing::warn!(
                    path = file,
                    "a recorded conversation file is outside the native homes; it stays"
                );
                continue;
            }
            let parent = path.parent().context("A conversation file has no folder")?;
            match parent.canonicalize() {
                Ok(resolved) if resolved == parent => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                _ => {
                    tracing::warn!(
                        path = file,
                        "a recorded conversation file is reached through a symlink; it stays"
                    );
                    continue;
                }
            }
            match std::fs::symlink_metadata(path) {
                Ok(meta) if meta.file_type().is_file() => std::fs::remove_file(path)
                    .or_else(|error| match error.kind() {
                        std::io::ErrorKind::NotFound => Ok(()),
                        _ => Err(error),
                    })
                    .with_context(|| format!("could not remove {file}"))?,
                Ok(_) => {
                    tracing::warn!(
                        path = file,
                        "a recorded conversation file is no longer a file; it stays"
                    )
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).with_context(|| format!("could not read {file}")),
            }
        }
        for folder in self.folders.iter().rev() {
            let path = Path::new(folder);
            // Through no symlink either: an ancestor's would lead outside the home.
            if inside(path)
                && path
                    .canonicalize()
                    .is_ok_and(|resolved| resolved.as_path() == path)
                && std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
            {
                // Not empty: something else is in it now, which stays with it.
                let _ = std::fs::remove_dir(path);
            }
        }
        Ok(())
    }
}

/// The committed transfer's packages, archive and preparation folder on this machine, plaintext
/// included: once the destination restored and the journal committed, nothing reads them again.
/// What cannot be removed is logged and left.
pub(crate) fn remove_artifacts(daemon: &Daemon, committed: &State) {
    let Some(operation) = committed
        .transfer
        .as_ref()
        .filter(|t| t.phase == Phase::Committed)
        .map(|t| t.operation_id.as_str())
    else {
        return;
    };
    let home = &daemon.paths.home;
    for path in [
        home.join("transfer-packages").join(operation),
        home.join("workspace-archives").join(operation),
        home.join(format!("workspace-preparation-{operation}")),
    ] {
        match std::fs::remove_dir_all(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                tracing::warn!(%error, path = %path.display(), "could not remove a committed transfer's files");
            }
            _ => {}
        }
    }
}

/// Deletes the copy of a workspace that `committed` retired here, now that it is back where it
/// came from. Returns why the deletion is still pending, if it is; the copy is then tried again
/// in the background. Refused unless the copy is retired by exactly that commit.
pub async fn discard(daemon: &Arc<Daemon>, committed: &State) -> Result<Option<String>> {
    committed.validate()?;
    let workspace = &committed.binding.workspace_id;
    let pending: Option<String> = daemon.store.with(|c| {
        c.query_row(
            "SELECT fence FROM workspace_discards WHERE workspace_id=?1",
            [workspace],
            |r| r.get(0),
        )
        .optional()
    })?;
    if let Some(raw) = pending {
        let pending: State = serde_json::from_str(&raw).map_err(anyhow::Error::from)?;
        if pending != *committed {
            return Err(Error::Conflict(
                "Another deletion of this workspace is pending here".into(),
            ));
        }
    } else {
        let Some(retired) = crate::cloud_workspace_return::committed_fence(daemon, workspace)
            .map_err(Error::Internal)?
        else {
            // Already deleted: a retry finds nothing.
            return match daemon.store.find_workspace(workspace)? {
                None => Ok(None),
                Some(_) => Err(Error::Conflict(
                    "This workspace is not retired here; only a moved copy can be deleted".into(),
                )),
            };
        };
        if retired.1 != *committed {
            return Err(Error::Conflict(
                "This machine's copy was not retired by that transfer; reconcile the journal first"
                    .into(),
            ));
        }
        daemon.store.durable(|c| {
            c.execute(
                "INSERT OR IGNORE INTO workspace_discards(workspace_id,fence,attempts) VALUES(?1,?2,0)",
                params![workspace, retired.0],
            )?;
            Ok(())
        })?;
    }
    Ok(attempt(daemon, workspace).await)
}

/// One try at a pending deletion; why it is still pending, if it is.
async fn attempt(daemon: &Arc<Daemon>, workspace: &str) -> Option<String> {
    let outcome = delete(daemon, workspace).await;
    match outcome {
        Ok(()) => None,
        Err(error) => {
            let message = format!("{error:#}");
            tracing::warn!(
                error = message,
                workspace,
                "could not delete a moved workspace's copy; it is retried"
            );
            let _ = daemon.store.with(|c| {
                c.execute(
                    "UPDATE workspace_discards SET attempts=attempts+1,last_error=?2 WHERE workspace_id=?1",
                    params![workspace, message],
                )
            });
            Some(message)
        }
    }
}

async fn delete(daemon: &Arc<Daemon>, workspace: &str) -> anyhow::Result<()> {
    let fence: Option<String> = daemon.store.with(|c| {
        c.query_row(
            "SELECT fence FROM workspace_discards WHERE workspace_id=?1",
            [workspace],
            |r| r.get(0),
        )
        .optional()
    })?;
    let Some(fence) = fence else {
        return Ok(());
    };
    let Some(row) = daemon.store.find_workspace(workspace)? else {
        forget(daemon, workspace)?;
        return Ok(());
    };
    // Came back here meanwhile (its return replaced the copy): nothing to delete any more.
    if daemon.store.workspace_fence(workspace)?.as_deref() != Some(fence.as_str()) {
        forget(daemon, workspace)?;
        return Ok(());
    }
    // Never while a return is restoring it here: that one sets the same services aside.
    let _restoring =
        crate::cloud_workspace_restore::Restoring::claim(&daemon.paths.home, workspace)?;
    // Its services' containers and volumes go first; a failure keeps the deletion pending.
    crate::workspace_service_provision::remove_declared(daemon, workspace).await?;
    remove_workspace(daemon, &row, &fence).await
}

/// Removes a copy of a workspace this machine holds under `fence`: its folder, its links in the
/// clones it was not restored into, its restoration folder and every row, in one transaction
/// guarded by the fence; the apps are told it is gone.
async fn remove_workspace(
    daemon: &Arc<Daemon>,
    row: &crate::store::WorkspaceRow,
    fence: &str,
) -> anyhow::Result<()> {
    let workspace = row.id.as_str();
    let workspaces = daemon.paths.workspaces_dir.canonicalize()?;
    // Resolved as the workspaces folder is. Already removed (a retry): its recorded parent must
    // resolve to that folder, never a same-named folder elsewhere.
    let recorded = PathBuf::from(&row.path);
    let folder = match recorded.canonicalize() {
        Ok(folder) => folder,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => recorded
            .parent()
            .context("The copy has no parent folder")?
            .canonicalize()?
            .join(recorded.file_name().context("The copy has no name")?),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        folder.parent() == Some(workspaces.as_path()),
        "The copy is not in the workspaces folder: {}",
        folder.display()
    );
    let restorations = daemon.paths.home.canonicalize()?.join("restorations");
    let restoration: Option<(String, Option<String>)> = daemon.store.with(|c| {
        c.query_row(
            "SELECT native_root,native_files FROM workspace_restores WHERE workspace_id=?1",
            [workspace],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
    })?;
    let (restoration, native_files) = restoration.unzip();
    let mut clones: Vec<PathBuf> = daemon
        .store
        .worktrees_of(workspace)?
        .into_iter()
        .map(|w| PathBuf::from(w.clone_path))
        .filter(|clone| !clone.starts_with(&restorations))
        .collect();
    clones.sort();
    clones.dedup();
    remove(&folder).context("could not remove the copy's folder")?;
    let git = crate::git::Git::new(daemon.shell_env.clone());
    for clone in clones.iter().filter(|clone| clone.exists()) {
        git.run(clone, &["worktree", "prune"])
            .await
            .with_context(|| format!("could not unlink the copy from {}", clone.display()))?;
    }
    if let Some(root) = restoration.map(PathBuf::from)
        && root.starts_with(&restorations)
    {
        remove(&root).context("could not remove the copy's restoration folder")?;
    }
    // The conversations its restorations wrote into this machine's native homes, and nothing else.
    if let Some(raw) = native_files.flatten() {
        serde_json::from_str::<NativeFiles>(&raw)
            .context("unreadable record of the copy's conversation files")?
            .remove()
            .context("could not remove the copy's conversation files")?;
    }
    daemon.store.durable(|c| {
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let chats = "SELECT id FROM chats WHERE workspace_id=?1";
        for table in ["chat_queue", "restored_chat_blocks"] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE chat_id IN ({chats})"),
                [workspace],
            )?;
        }
        for table in WORKSPACE_TABLES {
            tx.execute(
                &format!("DELETE FROM {table} WHERE workspace_id=?1"),
                [workspace],
            )?;
        }
        tx.execute(
            "DELETE FROM workspaces WHERE id=?1 AND execution_fence=?2",
            params![workspace, fence],
        )?;
        tx.execute(
            "DELETE FROM workspace_discards WHERE workspace_id=?1",
            [workspace],
        )?;
        tx.commit()?;
        Ok(())
    })?;
    daemon.events.registry(RegistryEvent::WorkspaceRemoved {
        workspace_id: workspace.to_owned(),
        slug: row.slug.clone(),
        error: None,
    });
    Ok(())
}

/// A cancelled transfer on its destination: the workspace it restored here, still quarantined
/// (never activated), and what it received go. A return's import had replaced the copy retired
/// here; the workspace runs on the source again, which keeps it.
pub async fn abandon(daemon: &Arc<Daemon>, cancelled: &State) -> Result<()> {
    cancelled.validate()?;
    let t = cancelled
        .transfer
        .as_ref()
        .filter(|t| t.phase == Phase::Cancelled)
        .ok_or_else(|| Error::Conflict("The transfer is not cancelled".into()))?;
    let workspace = &cancelled.binding.workspace_id;
    let _restoring =
        crate::cloud_workspace_restore::Restoring::claim(&daemon.paths.home, workspace)
            .map_err(|e| Error::Conflict(format!("{e:#}")))?;
    let restored: Option<(String, Option<String>)> = daemon.store.with(|c| {
        c.query_row(
            "SELECT operation_id,activated FROM workspace_restores WHERE workspace_id=?1",
            [workspace],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
    })?;
    if let Some((operation, activated)) = restored
        && operation == t.operation_id
    {
        if activated.is_some() {
            return Err(Error::Conflict(
                "The workspace already runs here; it cannot be abandoned".into(),
            ));
        }
        drop_restored(daemon, workspace)
            .await
            .map_err(Error::Internal)?;
    }
    let home = &daemon.paths.home;
    for path in [
        home.join("transfer-packages").join(&t.operation_id),
        home.join("workspace-archives").join(&t.operation_id),
    ] {
        remove(&path).map_err(|e| Error::Internal(e.into()))?;
    }
    Ok(())
}

/// Removes a restoration of `workspace` that was never activated: what a cancelled transfer
/// restored here, or an incomplete restoration its retry replaces. The source still holds it all.
pub(crate) async fn drop_restored(daemon: &Arc<Daemon>, workspace: &str) -> anyhow::Result<()> {
    // The services it started, with their fresh volumes, go first: nothing ran there yet.
    crate::workspace_service_provision::remove(daemon, workspace).await?;
    let Some(row) = daemon.store.find_workspace(workspace)? else {
        return Ok(());
    };
    let fence = daemon
        .store
        .workspace_fence(workspace)?
        .context("The restored copy has no fence")?;
    remove_workspace(daemon, &row, &fence).await
}

/// A cancelled transfer on its source: the fence goes, the workspace runs here again, and what
/// the transfer staged or held here goes.
pub fn release(daemon: &Daemon, cancelled: &State) -> Result<()> {
    // Cancelled while it was preparing, the source was never fenced: it runs already.
    if daemon
        .store
        .workspace_fence(&cancelled.binding.workspace_id)?
        .is_some()
    {
        daemon.execution.release_cancelled(cancelled)?;
    }
    let operation = &cancelled
        .transfer
        .as_ref()
        .ok_or_else(|| Error::Conflict("No transfer".into()))?
        .operation_id;
    let home = &daemon.paths.home;
    for path in [
        home.join("workspace-archives").join(operation),
        home.join(format!("workspace-preparation-{operation}")),
    ] {
        if let Err(error) = remove(&path) {
            tracing::warn!(%error, path = %path.display(), "could not remove a cancelled transfer's files");
        }
    }
    daemon.workspaces.publish(&cancelled.binding.workspace_id);
    Ok(())
}

fn remove(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn forget(daemon: &Daemon, workspace: &str) -> Result<()> {
    daemon.store.with(|c| {
        c.execute(
            "DELETE FROM workspace_discards WHERE workspace_id=?1",
            [workspace],
        )
    })?;
    Ok(())
}

/// Tries pending deletions again at start, then every ten minutes.
pub fn spawn_retries(daemon: Arc<Daemon>) {
    tokio::spawn(async move {
        loop {
            let pending: Vec<String> = daemon
                .store
                .with(|c| {
                    c.prepare("SELECT workspace_id FROM workspace_discards")?
                        .query_map([], |r| r.get(0))?
                        .collect()
                })
                .unwrap_or_default();
            for workspace in pending {
                attempt(&daemon, &workspace).await;
            }
            tokio::time::sleep(RETRY_EVERY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleting_a_copy_removes_only_the_conversation_files_its_restorations_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let home = root.join("home/.claude");
        let project = home.join("projects/key");
        std::fs::create_dir_all(&project).unwrap();
        let outside = root.join("elsewhere");
        std::fs::create_dir_all(&outside).unwrap();
        let text = |path: &Path| path.to_str().unwrap().to_owned();
        let written = project.join("restored.jsonl");
        std::fs::write(&written, "restored, then continued here\n").unwrap();
        std::fs::write(project.join("own.jsonl"), "the machine's own\n").unwrap();
        // What now stands where a restored file was, and is not a file it wrote, stays.
        std::fs::write(outside.join("target"), "someone else's\n").unwrap();
        std::os::unix::fs::symlink(outside.join("target"), project.join("link.jsonl")).unwrap();
        std::os::unix::fs::symlink(&outside, home.join("projects/through")).unwrap();
        std::fs::write(outside.join("reached.jsonl"), "behind a symlink\n").unwrap();
        std::fs::create_dir(outside.join("empty")).unwrap();
        let empty = home.join("projects/emptied");
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::write(empty.join("restored.jsonl"), "x").unwrap();
        let old = NativeFiles {
            homes: vec![text(&home)],
            files: vec![text(&empty.join("restored.jsonl"))],
            folders: vec![text(&empty)],
        };
        let files = NativeFiles::new(
            [text(&home), text(&root.join("home/.codex"))],
            ContextRestoreResult {
                files: vec![
                    text(&written),
                    text(&project.join("link.jsonl")),
                    text(&home.join("projects/through/reached.jsonl")),
                    text(&outside.join("target")),
                    text(&project.join("gone.jsonl")),
                ],
                folders: vec![
                    text(&home.join("projects")),
                    text(&project),
                    text(&home.join("projects/through/empty")),
                ],
            },
        )
        .merged(Some(&old));
        assert_eq!(files.homes.len(), 2, "each home once");
        assert_eq!(
            files.without(Some(&old)).files.len(),
            5,
            "what the replaced copy had added is not the attempt's"
        );
        files.remove().unwrap();
        assert!(!written.exists() && !empty.exists());
        assert!(project.join("own.jsonl").exists() && project.is_dir());
        assert!(std::fs::symlink_metadata(project.join("link.jsonl")).is_ok());
        assert!(outside.join("target").exists() && outside.join("reached.jsonl").exists());
        assert!(
            outside.join("empty").is_dir(),
            "an empty folder behind a symlink stays"
        );
        assert!(home.is_dir(), "a home is never removed");
        files.remove().unwrap();
    }
}
