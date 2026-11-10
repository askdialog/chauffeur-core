//! Source-side transfer package of one stopped workspace: a Git snapshot per worktree, its notes
//! folder (attachments, plans, designs), the other files of its folder, each chat's row, queue and
//! history, and the native conversation of every chat, under one manifest whose digest the
//! handoff journal records.
//! A backup (`capture_running`) packages a running workspace the same way, without its
//! databases, as after a power cut (docs/cloud-workspace-backups.md).
//!
//! Internal, not an RPC. The package is private plaintext: encrypt and authenticate it before it
//! leaves the Mac. A settled registered stop is required, which is not proof that detached
//! descendants, services or external writers are quiescent. Restoring it is separate work.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chauffeur_protocol::model::{
    AgentKind, ChatInfo, ChatKind, QueuedMessage, TicketRef, WorktreeOrigin,
};
use chauffeur_protocol::sidecar::{ContextCaptureParams, ContextCaptureResult, SidecarCall};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cloud_workspace_handoff::{Phase, State};
use crate::cloud_workspace_snapshot::{
    self as snapshot, copy_hash, open_file, private_dir, private_file, sync_directories, valid_path,
};
use crate::git::Git;
use crate::paths::{DESIGNS_DIR, NOTES_DIR};
use crate::{Daemon, Error, Result};

const MANIFEST: &str = "manifest.json";
const MAX_MANIFEST: u64 = 64 * 1024 * 1024;
const MAX_NOTES_FILES: usize = 100_000;
const MAX_NOTES_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_CONTEXT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_EVENTS_BYTES: u64 = 1024 * 1024 * 1024;
/// Written again by the destination's daemon: they describe its own paths and CLI.
const GENERATED: [&str; 4] = ["CLAUDE.md", "AGENTS.md", ".claude", ".mcp.json"];
/// Folders of the workspace folder's own files that never travel, as a repo's ignored files do
/// not: the destination installs them again.
const LEFT_BEHIND: [&str; 2] = ["node_modules", ".venv"];
const MAX_LOOSE_FILES: usize = 20_000;
const MAX_LOOSE_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u8,
    /// The exact stopping journal state the package was captured under.
    pub source: State,
    pub workspace: WorkspaceMeta,
    pub repos: Vec<RepoEntry>,
    pub chats: Vec<ChatEntry>,
    /// Files of the workspace's notes folder, by path relative to it.
    pub notes: Vec<FileEntry>,
    /// The workspace folder's other files, in no worktree and outside the notes (a tool's output,
    /// a file an agent wrote there), by path relative to it, under `loose/` in the package.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub loose: Vec<FileEntry>,
    /// Source declarations retained as provenance; restoring them grants no local authority.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub development_profiles: Vec<crate::workspace_development_profile::StoredProfile>,
    /// Verified logical snapshots and their source provenance; never automatic restore authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub development_data: Option<crate::cloud_workspace_data::DevelopmentData>,
    /// The source's own profile when it was captured, one of `development_profiles`: what the
    /// destination declares once it runs there (its services and previews), with or without
    /// database exports. Absent from packages written before it, and from a source with none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_profile: Option<crate::workspace_development_profile::ProfileHead>,
    /// A backup of the workspace running under `source`, the committed transfer that brought it
    /// here: its chats may have been mid-turn, and its databases are not carried.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub running: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMeta {
    pub id: String,
    pub slug: String,
    pub title: Option<String>,
    pub branch: String,
    pub tickets: Vec<TicketRef>,
    pub explore: bool,
    /// The source folder: saved attachments name files under it.
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RepoEntry {
    pub repo: String,
    /// The worktree's folder relative to the workspace folder.
    pub path: String,
    pub branch: String,
    pub base_branch: String,
    pub base_commit: Option<String>,
    pub origin: WorktreeOrigin,
    pub group: Option<String>,
    /// The worktree's `origin` URL without credentials, which the destination sets again so it
    /// fetches and pushes with its own. Absent when the repo has none Chauffeur carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// The Git snapshot's folder in the package and its manifest's SHA-256, which covers the
    /// bundle, index and files it lists.
    pub snapshot: FileEntry,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ChatEntry {
    pub info: ChatInfo,
    /// Sessions the conversation was forked from, oldest first.
    pub fork_sessions: Vec<String>,
    /// The folder the chat's agent ran in, relative to the workspace folder ("" for the folder).
    pub cwd: String,
    pub queue: Vec<QueuedMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub send_receipts: Vec<crate::chat_sends::Receipt>,
    /// The chat's stored events, one JSON envelope a line.
    pub events: FileEntry,
    pub contexts: Vec<ContextEntry>,
    /// Why the conversation cannot resume elsewhere; its native history stays on the source.
    pub blocked: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ContextEntry {
    pub session_id: String,
    pub file: FileEntry,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug)]
pub struct Package {
    /// Lowercase hex SHA-256 of `manifest.json`, for the journal's snapshot digest.
    pub digest: String,
    pub manifest: Manifest,
    /// What the workspace held as the package carries it, to compare a copy left behind with.
    pub contents: Contents,
}

/// A workspace's carried contents: each worktree's, by its folder, and each notes file's and
/// other file's SHA-256.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contents {
    pub repos: BTreeMap<String, snapshot::Contents>,
    pub notes: BTreeMap<String, String>,
    /// The workspace folder's files in no worktree and outside the notes, by path from it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub loose: BTreeMap<String, String>,
    /// Those a package cannot carry (a link, past its limits): a transfer refuses them, so none
    /// when the workspace left; the first hundred when read again.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub others: BTreeSet<String>,
}

/// Reads what the workspace in `root` holds now, as a package would carry it, writing nothing.
/// Only the worktrees in `repo_paths` are read; one gone is missing from the result.
pub async fn observe<'a>(
    git: &Git,
    root: &Path,
    repo_paths: impl IntoIterator<Item = &'a String>,
) -> anyhow::Result<Contents> {
    let mut repos = BTreeMap::new();
    let mut paths = Vec::new();
    for path in repo_paths {
        let worktree = root.join(path);
        if worktree.exists() {
            repos.insert(path.clone(), snapshot::observe(git, &worktree).await?);
        }
        paths.push(path.clone());
    }
    let notes = copy_notes(&root.join(NOTES_DIR), None)?
        .into_iter()
        .map(|file| (file.path, file.sha256))
        .collect();
    let found = loose(root, &paths)?;
    let loose = copy_loose(root, &found.files, None)?
        .into_iter()
        .map(|file| (file.path, file.sha256))
        .collect();
    let others = found.uncarried().take(100).cloned().collect();
    Ok(Contents {
        repos,
        notes,
        loose,
        others,
    })
}

/// A digest of what a backup of the workspace would carry now: its worktrees' and notes' files,
/// and its chats' rows, queues and last events. Equal digests: nothing to back up again.
pub(crate) async fn fingerprint(daemon: &Daemon, workspace: &str) -> anyhow::Result<String> {
    let row = daemon.workspaces.row(workspace)?;
    let root = PathBuf::from(&row.path).canonicalize()?;
    let mut paths = Vec::new();
    for worktree in daemon.store.worktrees_of(workspace)? {
        if !worktree.removed {
            let path = Path::new(&worktree.path).canonicalize()?;
            paths.push(path.strip_prefix(&root)?.to_string_lossy().into_owned());
        }
    }
    let contents = observe(&Git::new(daemon.shell_env.clone()), &root, &paths).await?;
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(&contents)?);
    digest.update(chat_state(daemon, workspace)?);
    Ok(hex::encode(digest.finalize()))
}

/// What differs between two readings of a workspace, by path from its folder: a file changed,
/// added or removed, or a worktree whose commit, branch or staged changes moved.
pub fn differences(before: &Contents, now: &Contents) -> Vec<String> {
    let mut changed = Vec::new();
    let repos: BTreeSet<_> = before.repos.keys().chain(now.repos.keys()).collect();
    for path in repos {
        let (Some(a), Some(b)) = (before.repos.get(path), now.repos.get(path)) else {
            changed.push(format!("{path}/ (the whole worktree)"));
            continue;
        };
        if a.head != b.head || a.branch != b.branch {
            changed.push(format!("{path}/ (its commit or branch)"));
        }
        if a.index_digest != b.index_digest {
            changed.push(format!("{path}/ (its staged changes)"));
        }
        let files: BTreeSet<_> = a.files.keys().chain(b.files.keys()).collect();
        changed.extend(
            files
                .into_iter()
                .filter(|file| {
                    a.files.get(*file) != b.files.get(*file)
                        || a.symlinks.contains(*file) != b.symlinks.contains(*file)
                })
                .map(|file| format!("{path}/{file}")),
        );
    }
    let notes: BTreeSet<_> = before.notes.keys().chain(now.notes.keys()).collect();
    changed.extend(
        notes
            .into_iter()
            .filter(|file| before.notes.get(*file) != now.notes.get(*file))
            .map(|file| format!("{NOTES_DIR}/{file}")),
    );
    let loose: BTreeSet<_> = before.loose.keys().chain(now.loose.keys()).collect();
    changed.extend(
        loose
            .into_iter()
            .filter(|file| before.loose.get(*file) != now.loose.get(*file))
            .cloned(),
    );
    changed.extend(now.others.difference(&before.others).cloned());
    changed
}

pub(crate) fn journal_digest(hex: &str) -> anyhow::Result<String> {
    ensure!(
        hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid package digest"
    );
    let bytes = (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// The caller authenticates current journal state; a digest or file from storage cannot supply it.
pub(crate) fn verify_published(folder: &Path, published: &State) -> anyhow::Result<Package> {
    published.validate()?;
    let transfer = published
        .transfer
        .as_ref()
        .context("The snapshot has no transfer operation")?;
    ensure!(
        transfer.phase == Phase::SnapshotReady,
        "The snapshot is not published"
    );
    let verified = verify(folder)?;
    let source = &verified.manifest.source;
    let original = source
        .transfer
        .as_ref()
        .context("The package has no stopping operation")?;
    ensure!(
        source.binding == published.binding
            && original.phase == Phase::Stopping
            && original.operation_id == transfer.operation_id
            && original.source_epoch == transfer.source_epoch
            && original.source_peer_id == transfer.source_peer_id
            && original.target_peer_id == transfer.target_peer_id
            && source.revision < published.revision
            && transfer.snapshot_digest.as_deref()
                == Some(journal_digest(&verified.digest)?.as_str()),
        "The package does not match the published operation and snapshot digest"
    );
    Ok(verified)
}

fn refused(message: String) -> Error {
    Error::Conflict(message)
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn hash_file(root: &Path, path: &str, budget: u64) -> anyhow::Result<FileEntry> {
    let file = open_file(root, path)?.with_context(|| format!("{path} is missing"))?;
    let (sha256, size) = copy_hash(file, None, budget)?;
    Ok(FileEntry {
        path: path.to_owned(),
        sha256,
        size,
    })
}

fn write_new(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut file = private_file(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Captures the stopped workspace into `output`, a new folder outside it. A chat whose native
/// conversation cannot be captured is marked blocked, never dropped; a file the package cannot
/// carry (a link, a Git repository the workspace does not have) refuses the whole capture.
pub async fn capture(daemon: &Arc<Daemon>, stopping: &State, output: &Path) -> Result<Package> {
    daemon.execution.require_settled_stop(stopping)?;
    Ok(capture_as(daemon, stopping, output, false).await?.0)
}

/// How many times a backup reads a worktree again when it changed while it was read.
const RUNNING_ATTEMPTS: usize = 3;

/// Captures the workspace while it runs here under `committed`, for a backup: nothing stops.
/// Each chat's history is taken up to the event it had when the capture began and each native
/// transcript up to its last complete line; a worktree that keeps changing while it is read fails
/// the capture, to try again later. Files of the folder a transfer would refuse are left out, and
/// returned (the first ten). The caller checks that the workspace runs here.
pub async fn capture_running(
    daemon: &Arc<Daemon>,
    committed: &State,
    output: &Path,
) -> Result<(Package, Vec<String>)> {
    capture_as(daemon, committed, output, true).await
}

async fn capture_as(
    daemon: &Arc<Daemon>,
    stopping: &State,
    output: &Path,
    running: bool,
) -> Result<(Package, Vec<String>)> {
    let workspace = daemon.workspaces.row(&stopping.binding.workspace_id)?;
    if workspace.state != chauffeur_protocol::model::WorkspaceState::Ready {
        return Err(refused(format!(
            "Workspace {} is not ready; only a ready workspace can be transferred",
            workspace.slug
        )));
    }
    if !output.is_absolute() || output.exists() {
        return Err(Error::BadRequest(
            "The package folder must be a new absolute path".into(),
        ));
    }
    let root = PathBuf::from(&workspace.path).canonicalize()?;
    if output
        .parent()
        .context("The package folder has no parent")?
        .canonicalize()?
        .starts_with(&root)
    {
        return Err(Error::BadRequest(
            "The package folder must be outside the workspace".into(),
        ));
    }
    let worktrees: Vec<_> = daemon
        .store
        .worktrees_of(&workspace.id)?
        .into_iter()
        .filter(|w| !w.removed)
        .collect();
    let repo_paths = repo_paths(&root, &worktrees)?;
    let loose_files = loose(&root, &repo_paths)?;
    let left_out = if running {
        loose_files.uncarried().take(10).cloned().collect()
    } else {
        check_layout(&loose_files)?;
        Vec::new()
    };
    let development_profiles =
        crate::cloud_workspace_profile::capture(&daemon.store, &workspace.id)?;
    let data_before = if running {
        None
    } else {
        Some(crate::cloud_workspace_data::inventory(
            daemon,
            stopping,
            &development_profiles,
        )?)
    };

    private_dir(output)?;
    let result = async {
        let git = Git::new(daemon.shell_env.clone());
        private_dir(&output.join("repos"))?;
        let mut repos = Vec::new();
        for (index, (worktree, path)) in worktrees.iter().zip(&repo_paths).enumerate() {
            let dir = format!("repos/{index}");
            let mut attempt = 1;
            // A failed snapshot leaves no folder behind, so it can be taken again.
            while let Err(error) = snapshot::capture(&git, &root.join(path), &output.join(&dir)).await {
                if !running || attempt == RUNNING_ATTEMPTS {
                    return Err(error.context(format!("could not snapshot {}", worktree.repo)).into());
                }
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
            let manifest = hash_file(output, &format!("{dir}/manifest.json"), MAX_MANIFEST)?;
            repos.push(RepoEntry {
                repo: worktree.repo.clone(),
                path: path.clone(),
                branch: worktree.branch.clone(),
                base_branch: worktree.base_branch.clone(),
                base_commit: worktree.base_commit.clone(),
                origin: worktree.origin,
                group: worktree.group.clone(),
                remote: origin_url(&git, &root.join(path)).await?,
                snapshot: FileEntry {
                    path: dir,
                    ..manifest
                },
            });
        }
        let notes = copy_notes(&root.join(NOTES_DIR), Some(&output.join("notes")))?;
        let loose = copy_loose(&root, &loose_files.files, Some(&output.join("loose")))?;
        // User edits of a fenced workspace's chats are refused; this catches the daemon's own.
        let before = chat_state(daemon, &workspace.id)?;
        let chats = capture_chats(daemon, &workspace, output, running).await?;
        let development_data = if running {
            None
        } else {
            let data_daemon = daemon.clone();
            let data_stopping = stopping.clone();
            let data_profiles = development_profiles.clone();
            let data_output = output.to_owned();
            tokio::task::spawn_blocking(move || {
                crate::cloud_workspace_data::capture(&data_daemon, &data_stopping, &data_output, &data_profiles)
            }).await.map_err(|error| Error::Internal(error.into()))??
        };
        let manifest = Manifest {
            version: if development_data.is_some() { 3 } else if development_profiles.is_empty() { 1 } else { 2 },
            source: stopping.clone(),
            workspace: WorkspaceMeta {
                id: workspace.id.clone(),
                slug: workspace.slug.clone(),
                title: workspace.title.clone(),
                branch: workspace.branch.clone(),
                tickets: workspace.tickets.clone(),
                explore: workspace.explore,
                path: workspace.path.clone(),
            },
            repos,
            chats,
            notes,
            loose,
            development_profiles,
            development_data,
            source_profile: crate::workspace_development_profile::load(&daemon.store, &workspace.id)?
                .map(|p| p.head),
            running,
        };
        let bytes = serde_json::to_vec_pretty(&manifest)?;
        if bytes.len() as u64 > MAX_MANIFEST {
            return Err(refused("The package manifest is too large".into()));
        }
        write_new(&output.join(MANIFEST), &bytes)?;
        sync_directories(output)?;
        // The fence may not have moved while the package was written.
        if !running {
            daemon.execution.require_settled_stop(stopping)?;
        }
        let verify_output = output.to_owned();
        let package = tokio::task::spawn_blocking(move || verify(&verify_output))
            .await.map_err(|error| Error::Internal(error.into()))??;
        if package.manifest != manifest {
            return Err(anyhow::anyhow!("the package changed while it was written").into());
        }
        if running {
            return Ok(package);
        }
        if chat_state(daemon, &workspace.id)? != before {
            return Err(refused(
                "The workspace's chats changed while they were packaged; transfer again".into(),
            ));
        }
        if crate::cloud_workspace_profile::capture(&daemon.store, &workspace.id)? != manifest.development_profiles {
            return Err(refused("The workspace's development profiles changed while they were packaged; transfer again".into()));
        }
        if Some(crate::cloud_workspace_data::inventory(daemon, stopping, &manifest.development_profiles)?) != data_before {
            return Err(refused("The workspace's database exports changed while they were packaged; retain their source and package again".into()));
        }
        Ok::<_, Error>(package)
    }
    .await;
    if result.is_err() {
        // Only the folder this call created; the workspace and its native histories stay.
        if let Err(error) = fs::remove_dir_all(output) {
            tracing::warn!(%error, path = %output.display(), "could not remove an incomplete package");
        }
    }
    Ok((result?, left_out))
}

/// The worktree's `origin` as the destination can use it: credentials in an HTTP(S) URL are
/// dropped, and a remote another machine cannot reach the same way (a local path, a helper
/// transport) is not carried.
async fn origin_url(git: &Git, worktree: &Path) -> anyhow::Result<Option<String>> {
    let url = git
        .run_allowing(worktree, &["config", "--get", "remote.origin.url"], &[1])
        .await?;
    Ok(portable_remote(url.trim_end_matches('\n')))
}

/// `url` in a form the destination accepts as `origin`: `https://`, `http://`, `ssh://` or
/// `git://` without user information in HTTP(S), or scp-like `[user@]host:path`. The destination
/// requires a carried remote to come back unchanged from this function.
pub(crate) fn portable_remote(url: &str) -> Option<String> {
    if url.is_empty() || url.len() > 2048 || url.chars().any(|c| c.is_control() || c == ' ') {
        return None;
    }
    for scheme in ["https://", "http://"] {
        if let Some(rest) = url.strip_prefix(scheme) {
            let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            let address = match rest[..end].rfind('@') {
                Some(at) => &rest[at + 1..],
                None => rest,
            };
            return (!address.is_empty() && !address.starts_with('/'))
                .then(|| format!("{scheme}{address}"));
        }
    }
    if ["ssh://", "git://"].iter().any(|s| url.starts_with(s)) {
        return Some(url.to_owned());
    }
    let (authority, path) = url.split_once(':')?;
    let host = authority.rsplit('@').next()?;
    // Another `scheme://` or a transport helper (`ext::`) is not scp-like.
    (!path.is_empty()
        && !path.starts_with(['/', ':'])
        && !authority.contains('/')
        && host
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-_[]".contains(c)))
    .then(|| url.to_owned())
}

/// Each worktree's folder relative to the workspace's; refuses one outside it.
fn repo_paths(root: &Path, worktrees: &[crate::store::WorktreeRow]) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    for worktree in worktrees {
        let path = Path::new(&worktree.path).canonicalize()?;
        let relative = path
            .strip_prefix(root)
            .ok()
            .and_then(|p| p.to_str())
            .filter(|p| valid_path(p).is_ok())
            .ok_or_else(|| {
                refused(format!(
                    "The worktree of {} is outside its workspace folder; it cannot be transferred",
                    worktree.repo
                ))
            })?;
        paths.push(relative.to_owned());
    }
    Ok(paths)
}

/// Refuses, before anything of it stops, a workspace whose folder the capture would refuse: a
/// file of its folder it cannot carry, an unsupported repo entry, or a worktree outside the folder.
pub async fn check_folder(daemon: &Daemon, workspace_id: &str) -> Result<u64> {
    let workspace = daemon.workspaces.row(workspace_id)?;
    let root = PathBuf::from(&workspace.path).canonicalize()?;
    let worktrees: Vec<_> = daemon
        .store
        .worktrees_of(&workspace.id)?
        .into_iter()
        .filter(|w| !w.removed)
        .collect();
    let paths = repo_paths(&root, &worktrees)?;
    let loose = loose(&root, &paths)?;
    check_layout(&loose)?;
    let mut bytes = 0u64;
    for path in &loose.files {
        bytes = bytes.saturating_add(fs::symlink_metadata(root.join(path))?.len());
    }
    let notes = root.join(NOTES_DIR);
    if notes.exists() {
        for entry in walkdir::WalkDir::new(notes)
            .follow_links(false)
            .min_depth(1)
        {
            let entry = entry.context("Could not estimate workspace notes")?;
            if entry.file_type().is_file() {
                bytes = bytes.saturating_add(
                    entry
                        .metadata()
                        .context("Could not estimate a notes file")?
                        .len(),
                );
            }
        }
    }
    let events: i64 = daemon.store.read(|c| {
        c.query_row(
            "SELECT COALESCE(SUM(LENGTH(payload)), 0) FROM events WHERE workspace_id=?1",
            [&workspace.id],
            |r| r.get(0),
        )
    })?;
    bytes = bytes.saturating_add(events.max(0) as u64);
    let git = Git::new(daemon.shell_env.clone());
    for path in paths {
        bytes = bytes.saturating_add(
            snapshot::check_source(&git, &root.join(&path))
                .await
                .map_err(|error| refused(format!("Cannot transfer {path}: {error:#}")))?,
        );
    }
    Ok(bytes)
}

/// Refuses a folder with a file the package cannot carry, or more than it carries: anything else
/// would be left behind.
fn check_layout(loose: &Loose) -> Result<()> {
    if let Some((path, why)) = loose.refused.first() {
        return Err(refused(format!(
            "{path} in the workspace folder is {why}, which a transfer cannot carry; move it into a repo or remove it, before transferring"
        )));
    }
    if let Some(path) = loose.over.first() {
        return Err(refused(format!(
            "The workspace folder holds more than {MAX_LOOSE_FILES} files or {} GiB outside its repos and notes ({path} among them); move some into a repo or remove them, before transferring",
            MAX_LOOSE_BYTES >> 30
        )));
    }
    Ok(())
}

/// The workspace folder's files in no worktree, outside the notes and not generated: a tool's
/// output written where the agents start (Playwright MCP's `.playwright-mcp/`), a file an agent
/// left there.
#[derive(Debug, Default)]
struct Loose {
    /// The regular files the package carries, by path from the workspace folder.
    files: Vec<String>,
    /// What it cannot carry, and why: a link, a special file, a Git repository the workspace does
    /// not have, a name a package cannot hold.
    refused: Vec<(String, &'static str)>,
    /// Files past what it carries (`MAX_LOOSE_FILES`, `MAX_LOOSE_BYTES`).
    over: Vec<String>,
}

impl Loose {
    fn uncarried(&self) -> impl Iterator<Item = &String> {
        self.refused.iter().map(|(path, _)| path).chain(&self.over)
    }
}

fn loose(root: &Path, repo_paths: &[String]) -> anyhow::Result<Loose> {
    let mut found = Loose::default();
    let mut bytes = 0u64;
    let mut walk = walkdir::WalkDir::new(root)
        .follow_links(false)
        .sort_by_file_name()
        .min_depth(1)
        .into_iter();
    while let Some(entry) = walk.next() {
        let entry = entry.context("could not read the workspace folder")?;
        let relative = entry
            .path()
            .strip_prefix(root)
            .ok()
            .and_then(|p| p.to_str())
            .context("Non-UTF8 workspace path")?
            .to_owned();
        let kind = entry.file_type();
        let top_level = entry.depth() == 1;
        let kept = repo_paths.contains(&relative)
            || (top_level && (relative == NOTES_DIR || GENERATED.contains(&relative.as_str())))
            || (kind.is_dir() && LEFT_BEHIND.iter().any(|name| entry.file_name() == *name));
        if kept {
            if kind.is_dir() {
                walk.skip_current_dir();
            }
            continue;
        }
        if entry.file_name().eq_ignore_ascii_case(".git") {
            found
                .refused
                .push((relative, "a Git repository the workspace does not have"));
            if kind.is_dir() {
                walk.skip_current_dir();
            }
            continue;
        }
        if kind.is_dir() || entry.file_name() == ".DS_Store" {
            continue;
        }
        if kind.is_symlink() {
            found.refused.push((relative, "a link"));
        } else if !kind.is_file() {
            found.refused.push((relative, "not a regular file"));
        } else if valid_path(&relative).is_err() {
            found
                .refused
                .push((relative, "named in a way a package cannot hold"));
        } else {
            bytes += entry.metadata()?.len();
            if found.files.len() < MAX_LOOSE_FILES && bytes <= MAX_LOOSE_BYTES {
                found.files.push(relative);
            } else {
                found.over.push(relative);
            }
        }
    }
    Ok(found)
}

/// Copies the listed files of the workspace folder into `output`, hashing them; one gone since it
/// was listed is skipped. Without `output`, only hashes them.
fn copy_loose(root: &Path, files: &[String], output: Option<&Path>) -> Result<Vec<FileEntry>> {
    if let Some(output) = output
        && !files.is_empty()
    {
        private_dir(output)?;
    }
    let mut entries = Vec::new();
    let mut bytes = 0u64;
    for relative in files {
        let Some(file) = open_file(root, relative)? else {
            continue;
        };
        let (sha256, size) = match output {
            Some(output) => {
                let target = output.join(relative);
                fs::create_dir_all(target.parent().context("Missing parent")?)?;
                let mut copy = private_file(&target)?;
                let hashed = copy_hash(file, Some(&mut copy), MAX_LOOSE_BYTES - bytes)?;
                copy.sync_all()?;
                hashed
            }
            None => copy_hash(file, None, MAX_LOOSE_BYTES - bytes)?,
        };
        bytes += size;
        entries.push(FileEntry {
            path: relative.clone(),
            sha256,
            size,
        });
    }
    Ok(entries)
}

/// Regular files only: a symlink or a special file refuses the capture rather than be skipped.
/// Without `output`, only hashes them.
fn copy_notes(source: &Path, output: Option<&Path>) -> Result<Vec<FileEntry>> {
    if let Some(output) = output {
        private_dir(output)?;
    }
    let mut files = Vec::new();
    match fs::symlink_metadata(source) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(files),
        Ok(meta) if meta.is_dir() => {}
        _ => {
            return Err(refused(format!(
                "{NOTES_DIR} in the workspace folder is not a folder; replace it with one before transferring"
            )));
        }
    }
    let mut bytes = 0u64;
    for entry in walkdir::WalkDir::new(source)
        .follow_links(false)
        .sort_by_file_name()
        .min_depth(1)
    {
        let entry = entry.context("could not read the notes folder")?;
        let relative = entry
            .path()
            .strip_prefix(source)
            .ok()
            .and_then(|p| p.to_str())
            .context("Non-UTF8 notes path")?
            .to_owned();
        let kind = entry.file_type();
        if kind.is_dir() {
            continue;
        }
        if !kind.is_file() || valid_path(&relative).is_err() {
            return Err(refused(format!(
                "{NOTES_DIR}/{relative} is a link or a name that cannot be carried; replace or remove it before transferring"
            )));
        }
        if files.len() >= MAX_NOTES_FILES {
            return Err(refused(format!(
                "The notes folder has more than {MAX_NOTES_FILES} files; remove some before transferring"
            )));
        }
        let file = open_file(source, &relative)?.context("A notes file vanished")?;
        let (sha256, size) = match output {
            Some(output) => {
                let target = output.join(&relative);
                fs::create_dir_all(target.parent().context("Missing parent")?)?;
                let mut copy = private_file(&target)?;
                let hashed = copy_hash(file, Some(&mut copy), MAX_NOTES_BYTES - bytes)?;
                copy.sync_all()?;
                hashed
            }
            None => copy_hash(file, None, MAX_NOTES_BYTES - bytes)?,
        };
        bytes += size;
        files.push(FileEntry {
            path: relative,
            sha256,
            size,
        });
    }
    Ok(files)
}

async fn capture_chats(
    daemon: &Arc<Daemon>,
    workspace: &crate::store::WorkspaceRow,
    output: &Path,
    running: bool,
) -> anyhow::Result<Vec<ChatEntry>> {
    private_dir(&output.join("events"))?;
    private_dir(&output.join("contexts"))?;
    let mut chats = Vec::new();
    for info in daemon.store.chats_of(&workspace.id)? {
        ensure!(safe_id(&info.id), "Unsupported chat id");
        ensure!(
            running || !daemon.chats.is_live(&info.id),
            "Chat {} still has a running session; stop it before transferring",
            info.id
        );
        // Running, its history goes on growing: it is taken up to where it is now.
        let up_to = daemon.store.chat_last_seq(&info.id)?.unwrap_or(0);
        let cwd = agent_folder(&info);
        let events_path = format!("events/{}.jsonl", info.id);
        write_events(daemon, &info.id, &output.join(&events_path), up_to)?;
        let events = hash_file(output, &events_path, MAX_EVENTS_BYTES)?;
        let fork_sessions = daemon.store.chat_fork_sessions(&info.id)?;
        let sessions = sessions_of(&fork_sessions, &info);
        let env = daemon
            .workspaces
            .agent_env(workspace, info.kind == ChatKind::Design)?;
        // Exactly the folder the session ran in: Claude keys its sessions by that string, as
        // the system reports it, so through no symbolic link (`/tmp` is `/private/tmp` on macOS).
        // A folder that is gone stays as named: its capture is then blocked, not the package.
        let ran_in = match cwd.as_str() {
            "" => PathBuf::from(&workspace.path),
            dir => Path::new(&workspace.path).join(dir),
        };
        let ran_in = ran_in.canonicalize().unwrap_or(ran_in);
        // A capture failure fails the package, to retry; only a conversation the sidecar could
        // not read is blocked, its native history kept on the source.
        let (contexts, blocked) =
            match capture_contexts(daemon, &info, &sessions, &ran_in, &env, output, running).await?
            {
                Ok(contexts) => (contexts, None),
                Err(reason) => {
                    fs::remove_dir_all(output.join("contexts").join(&info.id))?;
                    (Vec::new(), Some(reason))
                }
            };
        chats.push(ChatEntry {
            queue: daemon.store.queue_of(&info.id)?,
            send_receipts: crate::chat_sends::receipts(&daemon.store, &info.id)?,
            info,
            fork_sessions,
            cwd,
            events,
            contexts,
            blocked,
        });
    }
    Ok(chats)
}

/// Where the chat's agent runs, relative to the workspace folder: a design chat in its own.
fn agent_folder(info: &ChatInfo) -> String {
    match info.kind {
        ChatKind::Design => format!("{NOTES_DIR}/{DESIGNS_DIR}/{}", info.id),
        ChatKind::Chat => String::new(),
    }
}

/// The sessions whose conversations a chat carries: those it was forked from, then its own.
fn sessions_of(fork_sessions: &[String], info: &ChatInfo) -> Vec<String> {
    let mut sessions: Vec<String> = Vec::new();
    for session in fork_sessions.iter().chain(&info.agent_session_id) {
        if !sessions.contains(session) {
            sessions.push(session.clone());
        }
    }
    sessions
}

/// Page by page up to the event `up_to`, refusing before an event is loaded past the limit.
fn write_events(daemon: &Daemon, chat: &str, path: &Path, up_to: i64) -> anyhow::Result<()> {
    let mut file = std::io::BufWriter::new(private_file(path)?);
    let (mut after, mut written) = (0, 0u64);
    loop {
        let page = daemon
            .store
            .chat_event_page(chat, after, 100, MAX_EVENTS_BYTES - written)?;
        if page.first().is_none_or(|(seq, ..)| *seq > up_to) {
            break;
        }
        for (seq, size, payload) in page.into_iter().take_while(|(seq, ..)| *seq <= up_to) {
            let payload = payload
                .with_context(|| format!("The chat's history is over {MAX_EVENTS_BYTES} bytes"))?;
            let mut event: chauffeur_protocol::events::EventEnvelope =
                serde_json::from_str(&payload)?;
            event.seq = Some(seq);
            let mut line = serde_json::to_vec(&event)?;
            line.push(b'\n');
            written += (line.len() as u64).max(size as u64);
            ensure!(
                written <= MAX_EVENTS_BYTES,
                "The chat's history is over {MAX_EVENTS_BYTES} bytes"
            );
            file.write_all(&line)?;
            after = seq;
        }
    }
    file.into_inner()?.sync_all()?;
    Ok(())
}

/// What capture may not see change: chat rows, their queues and their last stored event.
fn chat_state(daemon: &Daemon, workspace: &str) -> anyhow::Result<Vec<u8>> {
    let mut state = Vec::new();
    for chat in daemon.store.chats_of(workspace)? {
        let queue = daemon.store.queue_of(&chat.id)?;
        let last = daemon.store.chat_last_seq(&chat.id)?;
        state.push(serde_json::json!([chat, queue, last]));
    }
    Ok(serde_json::to_vec(&state)?)
}

async fn capture_contexts(
    daemon: &Arc<Daemon>,
    info: &ChatInfo,
    sessions: &[String],
    cwd: &Path,
    env: &std::collections::BTreeMap<String, String>,
    output: &Path,
    live: bool,
) -> anyhow::Result<std::result::Result<Vec<ContextEntry>, String>> {
    if sessions.is_empty() {
        return Ok(Ok(Vec::new()));
    }
    let dir = format!("contexts/{}", info.id);
    private_dir(&output.join(&dir))?;
    let mut contexts = Vec::new();
    for session in sessions {
        ensure!(safe_id(session), "Unsupported native session id");
        let path = format!("{dir}/{session}.json");
        let value = daemon
            .chats
            .sidecar
            .call(SidecarCall::ContextCapture(ContextCaptureParams {
                chat_id: info.id.clone(),
                agent: info.agent,
                session_id: session.clone(),
                cwd: cwd.to_string_lossy().into_owned(),
                env: env.clone(),
                output: output.join(&path).to_string_lossy().into_owned(),
                live,
            }))
            .await?;
        let (sha256, bytes) = match serde_json::from_value(value)? {
            ContextCaptureResult::Captured { sha256, bytes } => (sha256, bytes),
            ContextCaptureResult::Blocked { reason } => return Ok(Err(reason)),
        };
        let file = hash_file(output, &path, MAX_CONTEXT_BYTES)?;
        ensure!(
            file.sha256 == sha256 && file.size == bytes,
            "The captured conversation differs from what the sidecar wrote"
        );
        contexts.push(ContextEntry {
            session_id: session.clone(),
            file,
        });
    }
    Ok(Ok(contexts))
}

/// Reads `manifest.json` and checks every file it lists, and that the package holds nothing
/// else outside the Git snapshots (whose own manifests list their contents, checked on restore).
pub fn verify(package: &Path) -> anyhow::Result<Package> {
    let file = open_file(package, MANIFEST)?.context("Package manifest missing")?;
    ensure!(
        file.metadata()?.len() <= MAX_MANIFEST,
        "Package manifest too large"
    );
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST + 1).read_to_end(&mut bytes)?;
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    verify_archive_limits(package)?;
    ensure!(
        (manifest.version == 1
            && manifest.development_profiles.is_empty()
            && manifest.development_data.is_none())
            // Declarations without exports this stop took: an earlier profile may have declared
            // some, which stay where they were taken.
            || (manifest.version == 2
                && !manifest.development_profiles.is_empty()
                && manifest.development_data.is_none())
            || (manifest.version == 3
                && !manifest.development_profiles.is_empty()
                && manifest
                    .development_data
                    .as_ref()
                    .is_some_and(|d| !d.exports.is_empty())),
        "Unsupported package version or development profile layout"
    );
    crate::cloud_workspace_profile::validate(
        &manifest.development_profiles,
        &manifest.workspace.id,
    )?;
    if let Some(head) = &manifest.source_profile {
        ensure!(
            manifest
                .development_profiles
                .iter()
                .any(|p| p.head == *head)
                && manifest
                    .development_data
                    .as_ref()
                    .is_none_or(|d| d.source_profile.as_ref() == Some(head)),
            "The source's profile is not the one packaged"
        );
    }
    manifest.source.validate()?;
    ensure!(
        manifest.source.binding.workspace_id == manifest.workspace.id,
        "Package names another workspace"
    );
    let mut expected = BTreeSet::from([MANIFEST.to_owned()]);
    let mut check = |entry: &FileEntry, budget: u64| -> anyhow::Result<()> {
        valid_path(&entry.path)?;
        let found = hash_file(package, &entry.path, budget)?;
        ensure!(found == *entry, "{} is corrupted", entry.path);
        ensure!(
            expected.insert(entry.path.clone()),
            "Duplicate package path"
        );
        Ok(())
    };
    let mut snapshots = BTreeSet::new();
    let mut contents = Contents::default();
    for (index, repo) in manifest.repos.iter().enumerate() {
        ensure!(
            repo.snapshot.path == format!("repos/{index}"),
            "Unexpected snapshot folder"
        );
        valid_path(&repo.path)?;
        ensure!(
            repo.remote
                .as_deref()
                .is_none_or(|url| portable_remote(url).as_deref() == Some(url)),
            "A worktree's origin cannot be set on another machine"
        );
        check(
            &FileEntry {
                path: format!("{}/manifest.json", repo.snapshot.path),
                ..repo.snapshot.clone()
            },
            MAX_MANIFEST,
        )?;
        let verified = snapshot::verify(&package.join(&repo.snapshot.path))
            .with_context(|| format!("the snapshot of {} is damaged", repo.repo))?;
        contents
            .repos
            .insert(repo.path.clone(), verified.contents());
        snapshots.insert(repo.snapshot.path.clone());
    }
    let mut chats = BTreeSet::new();
    for chat in &manifest.chats {
        let mut sends = BTreeSet::new();
        for receipt in &chat.send_receipts {
            uuid::Uuid::parse_str(&receipt.id)?;
            ensure!(
                sends.insert(&receipt.id)
                    && receipt.fingerprint.len() == 64
                    && receipt.fingerprint.bytes().all(|b| b.is_ascii_hexdigit()),
                "Invalid or duplicate send receipt"
            );
        }
        ensure!(
            safe_id(&chat.info.id) && chats.insert(&chat.info.id),
            "Unexpected chat"
        );
        ensure!(
            chat.info.workspace_id == manifest.workspace.id,
            "Chat of another workspace"
        );
        ensure!(
            chat.events.path == format!("events/{}.jsonl", chat.info.id),
            "Unexpected events file"
        );
        check(&chat.events, MAX_EVENTS_BYTES)?;
        ensure!(
            chat.cwd == agent_folder(&chat.info),
            "Unexpected chat folder"
        );
        let carried = chat.contexts.iter().map(|c| &c.session_id);
        if chat.blocked.is_some() {
            ensure!(
                chat.contexts.is_empty(),
                "A blocked chat carries a conversation"
            );
        } else {
            ensure!(
                carried.eq(sessions_of(&chat.fork_sessions, &chat.info).iter()),
                "A chat lacks the conversation of one of its sessions"
            );
        }
        for context in &chat.contexts {
            ensure!(
                safe_id(&context.session_id)
                    && context.file.path
                        == format!("contexts/{}/{}.json", chat.info.id, context.session_id),
                "Unexpected conversation file"
            );
            ensure!(
                chat.info.agent == AgentKind::Claude || chat.info.agent == AgentKind::Codex,
                "Unsupported agent"
            );
            check(&context.file, MAX_CONTEXT_BYTES)?;
        }
    }
    let mut notes_bytes = 0;
    for note in &manifest.notes {
        valid_path(&note.path)?;
        notes_bytes += note.size;
        ensure!(notes_bytes <= MAX_NOTES_BYTES, "Notes too large");
        check(
            &FileEntry {
                path: format!("notes/{}", note.path),
                ..note.clone()
            },
            MAX_NOTES_BYTES,
        )?;
    }
    ensure!(
        manifest.loose.len() <= MAX_LOOSE_FILES,
        "Too many files outside the repos"
    );
    let mut loose_bytes = 0;
    for file in &manifest.loose {
        valid_path(&file.path)?;
        let first = file.path.split('/').next().unwrap_or_default();
        ensure!(
            first != NOTES_DIR
                && !GENERATED.contains(&first)
                && !manifest.repos.iter().any(|repo| {
                    Path::new(&file.path).starts_with(&repo.path)
                        || Path::new(&repo.path).starts_with(&file.path)
                }),
            "A file outside the repos overlaps the workspace's own"
        );
        loose_bytes += file.size;
        ensure!(
            loose_bytes <= MAX_LOOSE_BYTES,
            "Files outside the repos too large"
        );
        check(
            &FileEntry {
                path: format!("loose/{}", file.path),
                ..file.clone()
            },
            MAX_LOOSE_BYTES,
        )?;
    }
    if let Some(data) = &manifest.development_data {
        crate::cloud_workspace_data::verify(
            package,
            data,
            &manifest.development_profiles,
            &manifest.source,
        )?;
        for export in &data.exports {
            for name in ["manifest.json", "database.dump"] {
                ensure!(
                    expected.insert(format!("{}/{name}", export.path)),
                    "Duplicate database export path"
                );
            }
        }
    }
    for entry in walkdir::WalkDir::new(package)
        .follow_links(false)
        .min_depth(1)
    {
        let entry = entry?;
        let relative = entry
            .path()
            .strip_prefix(package)?
            .to_str()
            .context("Non-UTF8 package path")?
            .to_owned();
        let in_snapshot = snapshots
            .iter()
            .any(|dir| relative == *dir || relative.starts_with(&format!("{dir}/")));
        if in_snapshot {
            ensure!(
                !entry.file_type().is_symlink(),
                "Unexpected package symlink"
            );
            continue;
        }
        if entry.file_type().is_dir() {
            continue;
        }
        ensure!(
            expected.contains(&relative),
            "Unexpected package file {relative}"
        );
    }
    contents.notes = manifest
        .notes
        .iter()
        .map(|note| (note.path.clone(), note.sha256.clone()))
        .collect();
    contents.loose = manifest
        .loose
        .iter()
        .map(|file| (file.path.clone(), file.sha256.clone()))
        .collect();
    Ok(Package {
        digest: hex::encode(Sha256::digest(&bytes)),
        manifest,
        contents,
    })
}

// Match package-crypto.ts framing before hashing large payloads: a 12-byte file header,
// UTF-8 path, file contents, then the final four-byte end marker; ciphertext adds 36 bytes.
fn verify_archive_limits(package: &Path) -> anyhow::Result<()> {
    let mut bytes = 4u64;
    let mut files = 0usize;
    let mut directories = 1usize;
    let mut file_paths = 0usize;
    let mut directory_paths = 0usize;
    for entry in walkdir::WalkDir::new(package)
        .follow_links(false)
        .min_depth(1)
    {
        let entry = entry?;
        let path = entry
            .path()
            .strip_prefix(package)?
            .to_str()
            .context("Non-UTF8 package path")?;
        if entry.file_type().is_dir() {
            directories += 1;
            directory_paths = directory_paths
                .checked_add(path.len())
                .context("Package directory path budget overflow")?;
            ensure!(
                directories <= 200_000 && directory_paths <= 64 * 1024 * 1024,
                "Package exceeds encrypted archive directory limits"
            );
        } else {
            ensure!(
                entry.file_type().is_file(),
                "Unsupported encrypted archive entry"
            );
            files += 1;
            file_paths = file_paths
                .checked_add(path.len())
                .context("Package file path budget overflow")?;
            let size = entry.metadata()?.len();
            bytes = bytes
                .checked_add(12 + path.len() as u64)
                .and_then(|total| total.checked_add(size))
                .context("Package byte budget overflow")?;
            ensure!(
                files <= 200_000
                    && file_paths <= 64 * 1024 * 1024
                    && bytes <= 16 * 1024 * 1024 * 1024,
                "Package exceeds encrypted archive limits"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_retained_link_replaced_with_identical_regular_bytes_is_a_local_edit() {
        let repo = snapshot::Contents {
            head: "head".into(),
            branch: None,
            index_digest: "index".into(),
            files: BTreeMap::from([("link".into(), (Some("digest".into()), false))]),
            symlinks: BTreeSet::from(["link".into()]),
        };
        let before = Contents {
            repos: BTreeMap::from([("repo".into(), repo)]),
            ..Contents::default()
        };
        let mut now = before.clone();
        now.repos.get_mut("repo").unwrap().symlinks.clear();
        assert_eq!(differences(&before, &now), ["repo/link"]);
    }

    #[test]
    fn archive_limits_include_framing_and_utf8_paths_before_reading_payloads() {
        let root = tempfile::tempdir().unwrap();
        let file = private_file(&root.path().join("é")).unwrap();
        let limit = 16 * 1024 * 1024 * 1024u64;
        // A sparse file exercises the metadata preflight without allocating or reading 16 GiB.
        file.set_len(limit - 4 - 12 - "é".len() as u64).unwrap();
        verify_archive_limits(root.path()).unwrap();
        file.set_len(limit - 4 - 12 - "é".len() as u64 + 1).unwrap();
        assert!(verify_archive_limits(root.path()).is_err());
        file.set_len(0).unwrap();
        std::os::unix::fs::symlink("é", root.path().join("alias")).unwrap();
        assert!(verify_archive_limits(root.path()).is_err());
    }

    #[test]
    fn a_remote_travels_without_credentials_only_when_another_machine_can_reach_it() {
        for (url, carried) in [
            (
                "git@github.com:acme/repo.git",
                Some("git@github.com:acme/repo.git"),
            ),
            ("github.com:org/repo", Some("github.com:org/repo")),
            (
                "ssh://git@host:22/org/repo",
                Some("ssh://git@host:22/org/repo"),
            ),
            (
                "https://github.com/org/repo.git",
                Some("https://github.com/org/repo.git"),
            ),
            (
                "https://x-access-token:ghp_secret@github.com/org/repo",
                Some("https://github.com/org/repo"),
            ),
            (
                "http://token@host/path@with-at",
                Some("http://host/path@with-at"),
            ),
            ("https://", None),
            ("/Users/me/repo", None),
            ("../repo", None),
            ("file:///tmp/repo", None),
            ("ext::sh -c touch% /tmp/pwned", None),
            ("ext::helper", None),
            ("-uhost:repo", None),
            ("host:", None),
            ("https://host/re\npo", None),
            ("", None),
        ] {
            assert_eq!(portable_remote(url).as_deref(), carried, "{url}");
            if let Some(carried) = carried {
                assert_eq!(portable_remote(carried).as_deref(), Some(carried));
            }
        }
    }
}
