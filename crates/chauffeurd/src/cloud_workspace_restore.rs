//! Destination restoration, quarantined independently of the source's cancellation fence.
//! Trusted callers authenticate the snapshot-ready journal state and the local destination peer.
//! A receipt proves restoration only; no process, setup script or ownership commit is started.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chauffeur_protocol::events::{ChatEvent, Event, EventEnvelope};
use chauffeur_protocol::model::{Attachment, ChatStatus, WorktreeOrigin};
use chauffeur_protocol::sidecar::{
    ContextRestoreEntry, ContextRestoreParams, ContextRestoreResult, SidecarCall,
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cloud_workspace_handoff::{Phase, State};
use crate::cloud_workspace_package::{self as package, Manifest};
use crate::cloud_workspace_snapshot::{
    self as snapshot, copy_hash, open_file, private_dir, sync_worktree_directories, valid_path,
};
use crate::{Daemon, Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub version: u8,
    pub workspace_id: String,
    pub operation_id: String,
    pub source_epoch: u64,
    pub target_peer_id: String,
    /// The journal uses base64url digests; the source package exposes the same SHA-256 in hex.
    pub snapshot_digest: String,
    pub workspace_path: String,
    pub native_root: String,
    pub blocked_chats: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Restored {
    pub receipt: Receipt,
    pub digest: String,
    /// Where the retired copy the user asked to keep now is, beside the returning workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept_copy: Option<String>,
    /// It replaced a copy retired here: the workspace came back.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub returned: bool,
}

/// Restores into new daemon-owned folders and atomically publishes fenced metadata. Repeating a
/// completed operation returns its durable receipt. An incomplete/uncertain operation retains its
/// files and quarantine for explicit reconciliation, never overwriting or deleting source data.
pub async fn restore(
    daemon: &Arc<Daemon>,
    folder: &Path,
    published: &State,
    destination_peer: &str,
    keep_local_copy: bool,
) -> Result<Restored> {
    let (daemon, folder, published, destination_peer) = (
        daemon.clone(),
        folder.canonicalize()?,
        published.clone(),
        destination_peer.to_owned(),
    );
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        runtime.block_on(restore_inner(
            &daemon,
            &folder,
            &published,
            &destination_peer,
            keep_local_copy,
        ))
    })
    .await
    .map_err(|error| Error::Internal(error.into()))?
    .map_err(Error::Internal)
}

/// The workspaces this process is restoring, by daemon home. A reservation without a receipt is
/// an abandoned restoration only while none of that workspace runs here.
static RESTORING: std::sync::Mutex<BTreeSet<(PathBuf, String)>> =
    std::sync::Mutex::new(BTreeSet::new());

/// One workspace's restoration in this process, released however it ends; abandoning one claims
/// it too.
pub(crate) struct Restoring((PathBuf, String));

impl Restoring {
    pub(crate) fn claim(home: &Path, workspace: &str) -> anyhow::Result<Restoring> {
        let key = (home.to_path_buf(), workspace.to_owned());
        ensure!(
            RESTORING
                .lock()
                .map_err(|_| anyhow::anyhow!("restorations poisoned"))?
                .insert(key.clone()),
            "This workspace is being restored here already; wait for it to end"
        );
        Ok(Restoring(key))
    }
}

impl Drop for Restoring {
    fn drop(&mut self) {
        if let Ok(mut restoring) = RESTORING.lock() {
            restoring.remove(&self.0);
        }
    }
}

async fn restore_inner(
    daemon: &Arc<Daemon>,
    folder: &Path,
    published: &State,
    destination_peer: &str,
    keep_local_copy: bool,
) -> anyhow::Result<Restored> {
    published.validate()?;
    let _restoring = Restoring::claim(&daemon.paths.home, &published.binding.workspace_id)?;
    let transfer = published
        .transfer
        .as_ref()
        .context("The snapshot has no transfer operation")?;
    ensure!(
        transfer.phase == Phase::SnapshotReady && transfer.target_peer_id == destination_peer,
        "The snapshot is not published for this destination"
    );
    let verified = package::verify_published(folder, published)?;
    let manifest = &verified.manifest;
    let snapshot_digest = package::journal_digest(&verified.digest)?;
    validate_layout(manifest)?;
    for repo in &manifest.repos {
        let state = snapshot::verify(&folder.join(&repo.snapshot.path))?;
        ensure!(
            state.branch.as_deref()
                == if manifest.workspace.explore {
                    None
                } else {
                    Some(repo.branch.as_str())
                },
            "A worktree's recorded branch differs from its snapshot"
        );
    }
    let previous: Option<(String, String, Option<String>)> = daemon.store.with(|c| {
        c.query_row("SELECT operation_id,snapshot_digest,receipt FROM workspace_restores WHERE workspace_id=?1", [&manifest.workspace.id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()
    })?;
    let mut previous = previous;
    if let Some((operation, digest, receipt)) = &previous
        && *operation == transfer.operation_id
    {
        ensure!(
            *digest == snapshot_digest,
            "Another restoration already owns this workspace; reconcile it before retrying"
        );
        if let Some(raw) = receipt {
            return Ok(serde_json::from_str(raw)?);
        }
        // A return that could not put its retired copy back holds that copy under this
        // workspace: never removed here.
        let returning: bool = daemon.store.with(|c| {
            c.query_row(
                "SELECT EXISTS(SELECT 1 FROM workspace_returns WHERE operation_id=?1)",
                [&transfer.operation_id],
                |r| r.get(0),
            )
        })?;
        ensure!(
            !returning,
            "An incomplete restoration is retained and quarantined beside the copy it was replacing; inspect its private folders before retrying"
        );
        // Retried after it failed (a resumed transfer): the incomplete restoration, never
        // activated, goes; the package restores again from the start.
        crate::cloud_workspace_discard::drop_restored(daemon, &manifest.workspace.id).await?;
        previous = None;
    }
    let root = daemon
        .paths
        .workspaces_dir
        .canonicalize()?
        .join(&manifest.workspace.slug);
    // Coming back to a machine holding its retired copy: the copy is replaced.
    let retired =
        crate::cloud_workspace_return::retired_copy(daemon, published, &root, keep_local_copy)?;
    // What the copy held when it left, read before the import forgets it: the copy is compared
    // with it once more before it is removed.
    let departure = retired.as_ref().and_then(|retired| {
        crate::cloud_workspace_return::departure(daemon, &retired.workspace_id).ok()
    });
    let old_native_files = retired
        .as_ref()
        .and_then(|retired| retired.restore.as_ref())
        .and_then(crate::cloud_workspace_return::OldRestore::native_files);
    // Edits made to the copy since it left are never overwritten unseen.
    if let Some(retired) = &retired
        && !retired.kept
        && let Some(problem) = crate::cloud_workspace_return::unsaved_changes(
            daemon,
            &retired.workspace_id,
            "this machine",
        )
        .await
    {
        return Err(crate::cloud_workspace_return::local_copy_changed(problem).into());
    }
    ensure!(
        previous.is_none() || retired.is_some(),
        "Another restoration already owns this workspace; reconcile it before retrying"
    );
    let restores = daemon.paths.home.canonicalize()?.join("restorations");
    match fs::symlink_metadata(&restores) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => private_dir(&restores)?,
        Ok(meta) => ensure!(
            meta.is_dir() && restores.canonicalize()? == restores,
            "Unsafe restoration parent"
        ),
        Err(error) => return Err(anyhow::Error::from(error)),
    }
    let retained = restores.join(&transfer.operation_id);
    // The receipt's `native_root`: the restoration's private folder (clones, database
    // exports). Conversations go to the machine's own native homes.
    let native = retained.clone();
    ensure!(
        (retired.is_some()
            || fs::symlink_metadata(&root)
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound))
            && fs::symlink_metadata(&retained)
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
        "A restoration folder already exists; preserve it and reconcile before retrying"
    );
    reserve(
        daemon,
        manifest,
        &root,
        &native,
        &transfer.operation_id,
        &snapshot_digest,
        retired.as_ref(),
    )?;
    if let Some(retired) = &retired
        && let Err(error) = crate::cloud_workspace_return::set_aside(retired)
    {
        crate::cloud_workspace_return::put_back(daemon, retired, &transfer.operation_id, false)?;
        return Err(error);
    }
    // From reservation onward, failures retain evidence and can never enable execution.
    let outcome = async {
        // The workspace is made as a local one is, under the user's umask; the restoration's
        // own folder (clones, exports) stays private.
        fs::create_dir(&root)?;
        private_dir(&retained)?;
        let clones = retained.join("repos");
        private_dir(&clones)?;
        let git = crate::git::Git::new(daemon.shell_env.clone());
        for (index, repo) in manifest.repos.iter().enumerate() {
            let source = folder.join(&repo.snapshot.path);
            let clone = clones.join(index.to_string());
            snapshot::restore_clone(&git, &source, &clone).await?;
            if let Some(url) = &repo.remote {
                snapshot::restore_git(&git, &clone, &["remote", "add", "origin", url]).await?;
            }
            let target = root.join(&repo.path);
            fs::create_dir_all(target.parent().context("Missing worktree parent")?)?;
            snapshot::restore_worktree(&git, &source, &clone, &target).await?;
        }
        let notes = root.join(crate::paths::NOTES_DIR);
        fs::create_dir(&notes)?;
        for note in &manifest.notes {
            write_file(folder, &format!("notes/{}", note.path), note, &notes.join(&note.path))?;
        }
        for file in &manifest.loose {
            write_file(folder, &format!("loose/{}", file.path), file, &root.join(&file.path))?;
        }
        if let Some(data) = &manifest.development_data {
            crate::cloud_workspace_data::validate(
                data,
                &manifest.development_profiles,
                &manifest.source,
            )?;
            if !data.exports.is_empty() {
                let exports = retained.join("database_exports");
                private_dir(&exports)?;
                for (index, export) in data.exports.iter().enumerate() {
                    crate::cloud_workspace_data::copy(
                        &folder.join(&export.path),
                        &exports.join(index.to_string()),
                        &export.receipt,
                    )?;
                }
            }
        }
        let mut contexts = Vec::new();
        for chat in &manifest.chats {
            // `join("")` would end the folder with a slash, which no canonical path has.
            let cwd = if chat.cwd.is_empty() {
                root.clone()
            } else {
                root.join(&chat.cwd)
            };
            fs::create_dir_all(&cwd)?;
            for context in &chat.contexts {
                contexts.push(ContextRestoreEntry {
                    agent: chat.info.agent,
                    session_id: context.session_id.clone(),
                    cwd: path_text(&cwd)?,
                    path: path_text(&folder.join(&context.file.path))?,
                    sha256: context.file.sha256.clone(),
                    bytes: context.file.size,
                });
            }
        }
        let homes = native_homes(daemon, contexts)?;
        let (claude_home, codex_home) = (homes.claude_home.clone(), homes.codex_home.clone());
        let added: ContextRestoreResult = serde_json::from_value(
            daemon
                .chats
                .sidecar
                .call(SidecarCall::ContextRestore(homes))
                .await?,
        )
        .context("unexpected answer from the sidecar to a native restoration")?;
        // Recorded at once, with what the copy it replaces had added: discarding this copy one
        // day removes exactly those files, and a failure below takes back this attempt's.
        let native_files = crate::cloud_workspace_discard::NativeFiles::new(
            [claude_home, codex_home],
            added,
        )
        .merged(old_native_files.as_ref());
        daemon.store.durable(|c| {
            c.execute(
                "UPDATE workspace_restores SET native_files=?3 WHERE workspace_id=?1 AND operation_id=?2",
                params![
                    manifest.workspace.id,
                    transfer.operation_id,
                    serde_json::to_string(&native_files).map_err(anyhow::Error::from)?
                ],
            )?;
            Ok(())
        })?;
        let config_repos = manifest
            .repos
            .iter()
            .filter(|repo| repo.origin == WorktreeOrigin::Workspace)
            .map(|repo| crate::genconfig::RepoEntry {
                repo: repo.repo.clone(),
                path: root.join(&repo.path),
                base_branch: repo.base_branch.clone(),
            })
            .collect::<Vec<_>>();
        crate::genconfig::write(&crate::genconfig::WorkspaceConfig {
            slug: &manifest.workspace.slug,
            branch: &manifest.workspace.branch,
            root: &root,
            repos: &config_repos,
            tickets: &manifest.workspace.tickets,
            port_base: 0,
            port_count: 0,
            parent_slug: None,
            cli: &crate::workspaces::cli_path(),
            explore: manifest.workspace.explore,
        })?;
        sync_worktree_directories(&root)?;
        sync_worktree_directories(&retained)?;
        ensure!(
            package::verify(folder)?.digest == verified.digest,
            "The package changed during restoration"
        );
        let receipt = Receipt {
            version: 1,
            workspace_id: manifest.workspace.id.clone(),
            operation_id: transfer.operation_id.clone(),
            source_epoch: transfer.source_epoch,
            target_peer_id: destination_peer.to_owned(),
            snapshot_digest,
            workspace_path: path_text(&root)?,
            native_root: path_text(&native)?,
            blocked_chats: manifest
                .chats
                .iter()
                .filter_map(|chat| {
                    chat.blocked
                        .clone()
                        .map(|reason| (chat.info.id.clone(), reason))
                })
                .collect(),
        };
        let restored = Restored {
            digest: URL_SAFE_NO_PAD.encode(Sha256::digest(serde_json::to_vec(&receipt)?)),
            receipt,
            kept_copy: retired
                .as_ref()
                .filter(|retired| retired.kept)
                .map(|retired| retired.aside.display().to_string()),
            returned: retired.is_some(),
        };
        install(
            daemon,
            manifest,
            folder,
            &root,
            &clones,
            &retained,
            &restored,
            retired.is_some(),
        )?;
        daemon.workspaces.publish(&manifest.workspace.id);
        Ok::<_, anyhow::Error>(restored)
    }
    .await;
    if let Some(retired) = &retired {
        return match outcome {
            Ok(mut restored) => {
                let kept =
                    crate::cloud_workspace_return::clean_up(daemon, retired, departure.as_ref())
                        .await;
                // A kept copy's worktrees still hold the branch in the user's clones.
                if kept.is_none() {
                    crate::cloud_workspace_return::attach(daemon, retired, &manifest.workspace.id)
                        .await;
                }
                restored.kept_copy = kept.map(|path| path.display().to_string());
                Ok(restored)
            }
            Err(error) => {
                crate::cloud_workspace_return::put_back(
                    daemon,
                    retired,
                    &transfer.operation_id,
                    true,
                )?;
                Err(error)
            }
        };
    }
    if let Err(error) = &outcome {
        let _ = daemon.store.set_workspace_state(
            &manifest.workspace.id,
            chauffeur_protocol::model::WorkspaceState::CleanupFailed,
            Some(&format!("Restoration is retained and quarantined: {error}")),
        );
    }
    outcome
}

fn path_text(path: &Path) -> anyhow::Result<String> {
    Ok(path
        .to_str()
        .context("Non-UTF8 restoration path")?
        .to_owned())
}

/// The destination's own Claude and Codex homes, as its agents resolve them: the sidecar and
/// its agents get exactly this scrubbed environment, so `CLAUDE_CONFIG_DIR` when set, else
/// `~/.claude`; and `~/.codex`, since the scrub drops every `CODEX_` variable, `CODEX_HOME`
/// included (looked up anyway, should that change). The user logs in there once per machine.
fn native_homes(
    daemon: &Daemon,
    contexts: Vec<ContextRestoreEntry>,
) -> anyhow::Result<ContextRestoreParams> {
    let vars = daemon.shell_env.vars();
    let set = |name: &str| vars.get(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    let home = set("HOME")
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .context("The destination has no home folder for its agents' native homes")?;
    // The sidecar writes only through canonical folders: resolve a symlinked home (dotfiles).
    let canonical = |path: PathBuf| -> anyhow::Result<PathBuf> {
        if path.exists() {
            return Ok(path.canonicalize()?);
        }
        let parent = path.parent().context("A native home has no parent")?;
        Ok(parent
            .canonicalize()?
            .join(path.file_name().context("A native home has no name")?))
    };
    let claude = canonical(set("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")))?;
    let codex = canonical(set("CODEX_HOME").unwrap_or_else(|| home.join(".codex")))?;
    Ok(ContextRestoreParams {
        claude_home: path_text(&claude)?.to_owned(),
        codex_home: path_text(&codex)?.to_owned(),
        contexts,
    })
}

/// Writes the package's `path` to `target`, a new file, as the user's own (their umask).
fn write_file(
    folder: &Path,
    path: &str,
    entry: &package::FileEntry,
    target: &Path,
) -> anyhow::Result<()> {
    let source = open_file(folder, path)?.with_context(|| format!("{path} is missing"))?;
    fs::create_dir_all(target.parent().context("Missing parent")?)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o666)
        .open(target)?;
    let (digest, size) = copy_hash(source, Some(&mut output), entry.size)?;
    ensure!(
        digest == entry.sha256 && size == entry.size,
        "{path} changed during restoration"
    );
    output.sync_all()?;
    Ok(())
}

fn validate_layout(manifest: &Manifest) -> anyhow::Result<()> {
    ensure!(
        uuid::Uuid::parse_str(&manifest.workspace.id).is_ok(),
        "Invalid workspace id"
    );
    valid_path(&manifest.workspace.slug)?;
    ensure!(
        !manifest.workspace.slug.contains('/'),
        "Invalid workspace slug"
    );
    let reserved = [
        ".chauffeur",
        "CLAUDE.md",
        "AGENTS.md",
        ".claude",
        ".mcp.json",
    ];
    let mut paths = BTreeSet::new();
    for repo in &manifest.repos {
        valid_path(&repo.path)?;
        crate::repos::validate_repo_name(&repo.repo)?;
        match repo.origin {
            WorktreeOrigin::Workspace => ensure!(
                repo.group.is_none() && repo.path == repo.repo,
                "Unexpected workspace worktree layout"
            ),
            WorktreeOrigin::Agent => {
                let group = repo
                    .group
                    .as_deref()
                    .context("An agent worktree has no group")?;
                valid_path(group)?;
                ensure!(
                    !group.contains('/')
                        && repo.path == format!(".worktrees/{group}/{}", repo.repo),
                    "Unexpected agent worktree layout"
                );
            }
        }
        ensure!(
            !reserved.contains(&repo.path.split('/').next().unwrap_or_default()),
            "Worktree overlaps reserved workspace files"
        );
        ensure!(paths.insert(repo.path.clone()), "Duplicate worktree folder");
    }
    for a in &paths {
        ensure!(
            !paths.iter().any(|b| a != b && Path::new(a).starts_with(b)),
            "Overlapping worktree folders"
        );
    }
    Ok(())
}

fn reserve(
    daemon: &Daemon,
    manifest: &Manifest,
    root: &Path,
    native: &Path,
    operation: &str,
    digest: &str,
    retired: Option<&crate::cloud_workspace_return::Retired>,
) -> Result<()> {
    daemon.store.durable(|c| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let fence = serde_json::to_string(&manifest.source)?;
        match retired {
            Some(retired) => crate::cloud_workspace_return::reserve(&tx, retired, operation, &fence)
                .map_err(Error::Internal)?,
            None => {
                tx.execute("INSERT INTO workspaces(id,slug,path,branch,state,tickets,created_at,explore,title,execution_fence) VALUES(?1,?2,?3,?4,'creating',?5,?6,?7,?8,?9)", params![manifest.workspace.id,manifest.workspace.slug,path_text(root)?,manifest.workspace.branch,serde_json::to_string(&manifest.workspace.tickets)?,crate::store::now(),manifest.workspace.explore,manifest.workspace.title,fence])?;
            }
        }
        tx.execute("INSERT INTO workspace_restores(workspace_id,operation_id,snapshot_digest,native_root) VALUES(?1,?2,?3,?4)",params![manifest.workspace.id,operation,digest,path_text(native)?])?;
        tx.commit()?;
        Ok(())
    })
}

fn relocate(
    attachments: &mut [Attachment],
    root: &Path,
    manifest: &Manifest,
) -> anyhow::Result<()> {
    for attachment in attachments {
        valid_path(&attachment.file)?;
        ensure!(!attachment.file.contains('/'), "Invalid attachment name");
        let relative = format!("attachments/{}", attachment.file);
        ensure!(
            manifest.notes.iter().any(|note| note.path == relative),
            "A chat attachment is absent from the package"
        );
        attachment.path = path_text(&root.join(".chauffeur").join(relative))?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)] // The restoration's own pieces, passed through once.
fn install(
    daemon: &Daemon,
    manifest: &Manifest,
    folder: &Path,
    root: &Path,
    clones: &Path,
    retained: &Path,
    restored: &Restored,
    replacing: bool,
) -> Result<()> {
    daemon.store.durable(|c| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if replacing {
            crate::cloud_workspace_return::replace_rows(
                &tx,
                manifest,
                &restored.receipt.operation_id,
            )
            .map_err(Error::Internal)?;
        }
        install_rows(&tx, manifest, folder, root, clones, retained, restored)
            .map_err(Error::Internal)?;
        tx.commit()?;
        Ok(())
    })
}

fn install_rows(
    tx: &rusqlite::Transaction<'_>,
    manifest: &Manifest,
    folder: &Path,
    root: &Path,
    clones: &Path,
    retained: &Path,
    restored: &Restored,
) -> anyhow::Result<()> {
    crate::cloud_workspace_profile::install(
        tx,
        &manifest.workspace.id,
        &manifest.development_profiles,
    )?;
    if let Some(data) = &manifest.development_data {
        crate::cloud_workspace_data::validate(
            data,
            &manifest.development_profiles,
            &manifest.source,
        )?;
        for (ordinal, export) in data.exports.iter().enumerate() {
            let root = retained.join("database_exports").join(ordinal.to_string());
            tx.execute(
                "INSERT INTO workspace_imported_database_exports(workspace_id,ordinal,root,receipt) VALUES(?1,?2,?3,?4)",
                params![manifest.workspace.id, ordinal as i64, path_text(&root)?, serde_json::to_string(&export.receipt)?],
            )?;
        }
    }
    for (index, repo) in manifest.repos.iter().enumerate() {
        let origin = if repo.origin == WorktreeOrigin::Workspace {
            "workspace"
        } else {
            "agent"
        };
        tx.execute("INSERT INTO worktrees(id,workspace_id,repo,clone_path,path,branch,base_branch,base_commit,origin,grp,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![uuid::Uuid::now_v7().to_string(),manifest.workspace.id,repo.repo,path_text(&clones.join(index.to_string()))?,path_text(&root.join(&repo.path))?,repo.branch,repo.base_branch,repo.base_commit,origin,repo.group,crate::store::now()])?;
    }
    for chat in &manifest.chats {
        let mut info = chat.info.clone();
        info.status = if chat.blocked.is_some() {
            ChatStatus::Error
        } else {
            ChatStatus::Idle
        };
        ensure!(
            crate::store::insert_chat_row(tx, &info)? == 1,
            "The restored workspace disappeared"
        );
        tx.execute(
            "UPDATE chats SET fork_sessions=?2 WHERE id=?1",
            params![info.id, serde_json::to_string(&chat.fork_sessions)?],
        )?;
        if let Some(reason) = &chat.blocked {
            tx.execute(
                "INSERT INTO restored_chat_blocks(chat_id,reason) VALUES(?1,?2)",
                params![info.id, reason],
            )?;
        }
        for receipt in &chat.send_receipts {
            uuid::Uuid::parse_str(&receipt.id)?;
            ensure!(
                receipt.fingerprint.len() == 64
                    && receipt.fingerprint.bytes().all(|b| b.is_ascii_hexdigit()),
                "Invalid send receipt fingerprint"
            );
            tx.execute("INSERT INTO chat_send_receipts(chat_id,id,fingerprint,accepted) VALUES(?1,?2,?3,?4)", params![info.id,receipt.id,receipt.fingerprint,receipt.accepted])?;
        }
        for (position, message) in chat.queue.iter().enumerate() {
            let mut attachments = message.attachments.clone();
            relocate(&mut attachments, root, manifest)?;
            tx.execute("INSERT INTO chat_queue(id,chat_id,position,text,attachments,created_at) VALUES(?1,?2,?3,?4,?5,?6)",params![message.id,info.id,position as i64,message.text,serde_json::to_string(&attachments)?,message.created_at])?;
        }
        let mut reader =
            BufReader::new(open_file(folder, &chat.events.path)?.context("Chat events missing")?);
        let mut previous = 0;
        let mut event_hash = Sha256::new();
        let mut event_bytes = 0u64;
        loop {
            let mut line = Vec::new();
            const MAX_EVENT: u64 = 64 * 1024 * 1024;
            if Read::take(&mut reader, MAX_EVENT + 1).read_until(b'\n', &mut line)? == 0 {
                break;
            }
            ensure!(
                line.len() as u64 <= MAX_EVENT && line.last() == Some(&b'\n'),
                "A stored chat event is incomplete or exceeds 64 MiB"
            );
            event_bytes += line.len() as u64;
            ensure!(
                event_bytes <= chat.events.size,
                "Chat events changed during restoration"
            );
            event_hash.update(&line);
            let mut envelope: EventEnvelope = serde_json::from_slice(&line)
                .map_err(|_| anyhow::anyhow!("Invalid stored chat event"))?;
            let seq = envelope.seq.context("Stored event sequence missing")?;
            ensure!(seq > previous, "Stored events are out of order");
            previous = seq;
            let Event::Chat {
                workspace_id,
                chat_id,
                event,
            } = &mut envelope.event
            else {
                anyhow::bail!("Unexpected non-chat event");
            };
            ensure!(
                workspace_id == &manifest.workspace.id && chat_id == &info.id,
                "Stored event belongs to another chat"
            );
            match event {
                ChatEvent::UserMessage {
                    attachments: Some(attachments),
                    ..
                }
                | ChatEvent::ToolResult {
                    attachments: Some(attachments),
                    ..
                } => relocate(attachments, root, manifest)?,
                ChatEvent::QueueChanged { messages } => {
                    for message in messages {
                        relocate(&mut message.attachments, root, manifest)?;
                    }
                }
                _ => {}
            }
            envelope.seq = None;
            tx.execute("INSERT INTO events(ts,workspace_id,chat_id,registry,payload) VALUES(?1,?2,?3,0,?4)",params![envelope.ts,manifest.workspace.id,info.id,serde_json::to_string(&envelope)?])?;
        }
        ensure!(
            event_bytes == chat.events.size
                && hex::encode(event_hash.finalize()) == chat.events.sha256,
            "Chat events changed during restoration"
        );
    }
    tx.execute(
        "UPDATE workspaces SET state='ready' WHERE id=?1",
        [&manifest.workspace.id],
    )?;
    ensure!(
        tx.execute(
            "UPDATE workspace_restores SET receipt=?2 WHERE workspace_id=?1 AND receipt IS NULL",
            params![manifest.workspace.id, serde_json::to_string(restored)?]
        )? == 1,
        "Restoration publication changed"
    );
    Ok(())
}

#[cfg(test)]
mod restoring_tests {
    use super::Restoring;
    use std::path::Path;

    #[test]
    fn one_workspace_restores_one_at_a_time_in_a_daemon() {
        let home = Path::new("/nonexistent/restoring-test-home");
        let first = Restoring::claim(home, "w1").unwrap();
        assert!(Restoring::claim(home, "w1").is_err());
        // Another workspace, or another daemon's home, is not held.
        drop(Restoring::claim(home, "w2").unwrap());
        drop(Restoring::claim(Path::new("/nonexistent/other-home"), "w1").unwrap());
        drop(first);
        drop(Restoring::claim(home, "w1").unwrap());
    }
}
