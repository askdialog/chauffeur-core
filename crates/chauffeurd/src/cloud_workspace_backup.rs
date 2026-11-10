//! Encrypted backups of a workspace while it runs on a GCP worker (docs/cloud-workspace-backups.md).
//!
//! On the worker: every five minutes, at a moment no agent turn runs if one comes within the last
//! minute, else live, the workspace is packaged as it runs (`capture_running`), sealed with the
//! Mac's key for it and written with `gcloud storage` to the worker's prefix of the backup bucket.
//! The three latest successful backups are kept; one is deleted only once a newer one is written
//! and read back, so a failed or slow backup never costs the last good one. Nothing is uploaded
//! when nothing changed since the last backup.
//!
//! On the Mac: the key is made when the workspace leaves for a worker (`configure_worker`), and
//! where its backups stand is kept as the worker last said (`relay_events` mirrors it), so the
//! last good backup still shows once the worker no longer answers. Restoring one is later work.
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chauffeur_protocol::cloud_backup::{
    BackupHealth, BackupRef, BackupStatus, CloudBackupConfigureParams,
};
use chauffeur_protocol::model::ChatStatus;
use chauffeur_protocol::sidecar::PackageTransferContext;
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::cloud_workspace_archive::{self as archive, ArchiveKey};
use crate::cloud_workspace_handoff::{Phase, State};
use crate::cloud_workspace_package as package;
use crate::secrets::{SecretName, Secrets};
use crate::{Daemon, Error, Result};

/// The target between two backups (`docs/cloud-workspaces.md`).
const EVERY: Duration = Duration::from_secs(5 * 60);
/// From then on, a backup is taken as soon as no agent turn runs.
const QUIET_FROM: Duration = Duration::from_secs(4 * 60);
const TICK: Duration = Duration::from_secs(30);
const KEPT: usize = 3;
/// Past this without a success, backups are stale: three missed in a row.
const STALE_AFTER: chrono::TimeDelta = chrono::TimeDelta::minutes(15);
/// A conversation a backup could not carry is tried again this long after it, even with nothing
/// else changed; each try replaces a backup, which storage keeps seven days.
const INCOMPLETE_RETRY: chrono::TimeDelta = chrono::TimeDelta::hours(1);
/// Writing a backup to storage; any other storage call.
const UPLOAD_LIMIT: Duration = Duration::from_secs(30 * 60);
const CALL_LIMIT: Duration = Duration::from_secs(2 * 60);

/// Where a workspace running here is backed up.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Config {
    bucket: String,
    prefix: String,
}

/// A workspace's backups as this daemon keeps them: what clients see, and what only it needs.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct Saved {
    last_success_at: Option<String>,
    checked_at: Option<String>,
    last_attempt_at: Option<String>,
    error: Option<String>,
    backups: Vec<BackupRef>,
    left_out: Vec<String>,
    #[serde(default)]
    incomplete: Vec<String>,
    /// What the newest backup holds (`package::fingerprint`); the worker's own only.
    #[serde(default)]
    fingerprint: Option<String>,
    /// The object being written, recorded first: one a failure or a stop left behind is deleted
    /// with the old ones.
    #[serde(default)]
    uploading: Option<String>,
    /// Objects to delete (older backups, uploads that did not count), tried again after each
    /// success until gone.
    #[serde(default)]
    doomed: Vec<String>,
    #[serde(default)]
    configured_at: Option<String>,
    /// The epoch of the transfer this stay here runs the workspace under; the worker's own only.
    #[serde(default)]
    epoch: Option<u64>,
}

pub struct Backups {
    secrets: Secrets,
    /// One backup at a time per workspace: held while it runs.
    running: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Backups {
    pub fn new(secrets: Secrets) -> Self {
        Backups {
            secrets,
            running: Mutex::default(),
        }
    }

    pub(crate) fn key(&self, workspace: &str) -> Result<Option<ArchiveKey>> {
        let name = key_name(workspace);
        let Some(encoded) = self
            .secrets
            .read_protected(&secret(&name))
            .map_err(Error::Internal)?
            .map(Zeroizing::new)
        else {
            return Ok(None);
        };
        Ok(Some(decode_key(&encoded)?))
    }

    /// Keeps `key` as the workspace's, replacing another one.
    fn keep_key(&self, workspace: &str, key: &ArchiveKey) -> Result<()> {
        let name = key_name(workspace);
        let encoded = Zeroizing::new(URL_SAFE_NO_PAD.encode(key.expose_bytes()));
        let kept = self
            .secrets
            .read_protected(&secret(&name))
            .map_err(Error::Internal)?
            .map(Zeroizing::new);
        if kept.as_ref().map(|k| k.as_str()) == Some(encoded.as_str()) {
            return Ok(());
        }
        if kept.is_some() {
            self.secrets.delete(&secret(&name))?;
        }
        self.secrets.create_protected(&secret(&name), &encoded)?;
        Ok(())
    }
}

fn key_name(workspace: &str) -> String {
    format!("chauffeur-backup-v1-{workspace}")
}

fn secret(name: &str) -> SecretName<'_> {
    SecretName {
        service: name,
        file: name,
        label: "Chauffeur workspace backup key",
    }
}

fn decode_key(encoded: &str) -> Result<ArchiveKey> {
    let bytes: [u8; 32] = URL_SAFE_NO_PAD
        .decode(encoded)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| Error::BadRequest("A backup key is 32 bytes in base64url".into()))?;
    Ok(ArchiveKey::from_bytes(bytes))
}

/// A bucket name as GCS accepts it, without dots (no domain-named buckets).
fn valid_bucket(bucket: &str) -> bool {
    (3..=63).contains(&bucket.len())
        && bucket
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        && bucket.starts_with(|c: char| c.is_ascii_alphanumeric())
        && bucket.ends_with(|c: char| c.is_ascii_alphanumeric())
}

fn valid_prefix(prefix: &str) -> bool {
    (1..=63).contains(&prefix.len())
        && prefix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn load_config(daemon: &Daemon, workspace: &str) -> Result<Option<Config>> {
    let raw: Option<String> = daemon.store.with(|c| {
        c.query_row(
            "SELECT info FROM workspace_backup_configs WHERE workspace_id=?1",
            [workspace],
            |r| r.get(0),
        )
        .optional()
    })?;
    raw.map(|raw| serde_json::from_str(&raw).map_err(|e| Error::Internal(e.into())))
        .transpose()
}

fn configured(daemon: &Daemon) -> Result<Vec<String>> {
    daemon.store.with(|c| {
        c.prepare("SELECT workspace_id FROM workspace_backup_configs ORDER BY workspace_id")?
            .query_map([], |r| r.get(0))?
            .collect()
    })
}

fn load(daemon: &Daemon, workspace: &str) -> Result<Option<Saved>> {
    let raw: Option<String> = daemon.store.with(|c| {
        c.query_row(
            "SELECT info FROM workspace_backup_status WHERE workspace_id=?1",
            [workspace],
            |r| r.get(0),
        )
        .optional()
    })?;
    raw.map(|raw| serde_json::from_str(&raw).map_err(|e| Error::Internal(e.into())))
        .transpose()
}

fn save(daemon: &Daemon, workspace: &str, saved: &Saved) -> Result<()> {
    let info = serde_json::to_string(saved).map_err(|e| Error::Internal(e.into()))?;
    daemon.store.with(|c| {
        c.execute(
            "INSERT INTO workspace_backup_status (workspace_id, info) VALUES (?1, ?2)
             ON CONFLICT(workspace_id) DO UPDATE SET info=excluded.info",
            params![workspace, info],
        )
    })?;
    Ok(())
}

fn parse_time(time: Option<&str>) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(time?)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn health(saved: &Saved, now: DateTime<Utc>) -> BackupHealth {
    if saved.error.is_some() {
        return BackupHealth::Failed;
    }
    match parse_time(saved.checked_at.as_deref()) {
        Some(checked) if now - checked > STALE_AFTER => BackupHealth::Stale,
        Some(_) => BackupHealth::Current,
        // None yet: awaited from the setup, and late like any other.
        None => match parse_time(saved.configured_at.as_deref()) {
            Some(configured) if now - configured > STALE_AFTER => BackupHealth::Stale,
            _ => BackupHealth::Pending,
        },
    }
}

fn status(saved: &Saved, now: DateTime<Utc>) -> BackupStatus {
    BackupStatus {
        health: health(saved, now),
        last_success_at: saved.last_success_at.clone(),
        checked_at: saved.checked_at.clone(),
        last_attempt_at: saved.last_attempt_at.clone(),
        configured_at: saved.configured_at.clone(),
        error: saved.error.clone(),
        backups: saved.backups.clone(),
        left_out: saved.left_out.clone(),
        incomplete: saved.incomplete.clone(),
    }
}

/// What a workspace's info shows of its backups: here, while it runs here backed up; moved, as
/// the machine it runs on last said (or why it is not backed up). Stale counts from now.
pub fn shown(daemon: &Daemon, workspace: &str, moved: bool) -> Option<BackupStatus> {
    // Back on the Mac, what the worker said of it is history: it is not backed up here.
    if !moved
        && (load_config(daemon, workspace).ok().flatten().is_none()
            || running_here(daemon, workspace).ok().flatten().is_none())
    {
        return None;
    }
    let saved = load(daemon, workspace).ok().flatten()?;
    Some(status(&saved, Utc::now()))
}

/// Keeps what the machine a moved workspace runs on says of its backups. Its fingerprint and
/// pending deletions stay there.
pub fn mirror(daemon: &Daemon, workspace: &str, shown: &BackupStatus) -> bool {
    let saved = Saved {
        last_success_at: shown.last_success_at.clone(),
        checked_at: shown.checked_at.clone(),
        last_attempt_at: shown.last_attempt_at.clone(),
        configured_at: shown.configured_at.clone(),
        error: shown.error.clone(),
        backups: shown.backups.clone(),
        left_out: shown.left_out.clone(),
        incomplete: shown.incomplete.clone(),
        ..Saved::default()
    };
    let before = load(daemon, workspace).ok().flatten();
    if before.as_ref().is_some_and(|b| same(b, &saved)) {
        return false;
    }
    if let Err(error) = save(daemon, workspace, &saved) {
        tracing::warn!(%error, workspace, "could not keep the backups' status");
        return false;
    }
    true
}

fn same(a: &Saved, b: &Saved) -> bool {
    a.last_success_at == b.last_success_at
        && a.checked_at == b.checked_at
        && a.last_attempt_at == b.last_attempt_at
        && a.configured_at == b.configured_at
        && a.error == b.error
        && a.backups == b.backups
        && a.left_out == b.left_out
        && a.incomplete == b.incomplete
}

/// Forgets what was said of a workspace's backups: it came back, or left for a machine that is
/// not backed up, or for another worker, which starts afresh.
pub fn forget(daemon: &Daemon, workspace: &str) {
    if let Err(error) = daemon.store.with(|c| {
        c.execute(
            "DELETE FROM workspace_backup_status WHERE workspace_id=?1",
            [workspace],
        )
    }) {
        tracing::warn!(%error, workspace, "could not forget the backups' status");
    }
}

/// On a worker the workspace leaves: its backups stop here and its key goes. The objects its
/// backups left stay listed, to go once a later stay here has a backup of its own.
pub fn leave(daemon: &Daemon, workspace: &str) -> Result<()> {
    if load_config(daemon, workspace)?.is_none() {
        // Not backed up here (the Mac, whose key opens the backups): nothing to stop.
        return Ok(());
    }
    daemon.store.with(|c| {
        c.execute(
            "DELETE FROM workspace_backup_configs WHERE workspace_id=?1",
            [workspace],
        )
    })?;
    let name = key_name(workspace);
    if daemon
        .backups
        .secrets
        .read_protected(&secret(&name))
        .map_err(Error::Internal)?
        .is_some()
    {
        daemon.backups.secrets.delete(&secret(&name))?;
    }
    Ok(())
}

/// The backups of the stay under the transfer of `epoch`: as kept when they are this stay's;
/// else none taken yet, and what an earlier stay kept or left to delete goes once this one has a
/// backup of its own (sealed under another transfer, the Mac no longer opens them).
fn for_stay(saved: Saved, epoch: u64, now: DateTime<Utc>) -> Saved {
    if saved.epoch == Some(epoch) {
        return saved;
    }
    let mut doomed = saved.doomed;
    doomed.extend(saved.uploading);
    doomed.extend(saved.backups.into_iter().map(|b| b.object));
    Saved {
        configured_at: Some(now.to_rfc3339()),
        epoch: Some(epoch),
        doomed,
        ..Saved::default()
    }
}

/// The committed transfer the workspace runs here under, when it does: activated here, ready, and
/// with no departure pending. A cancelled departure keeps its fence to reject delayed stop
/// messages, but resumes the same stay and its backups.
fn running_here(daemon: &Daemon, workspace: &str) -> Result<Option<State>> {
    let row: Option<(Option<String>, Option<String>, String)> = daemon.store.with(|c| {
        c.query_row(
            "SELECT r.activated, w.execution_fence, w.state FROM workspaces w
             JOIN workspace_restores r ON r.workspace_id = w.id WHERE w.id=?1",
            [workspace],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
    })?;
    let Some((Some(activated), fence, state)) = row else {
        return Ok(None);
    };
    if state != "ready" {
        return Ok(None);
    }
    let committed: State =
        serde_json::from_str(&activated).map_err(|e| Error::Internal(e.into()))?;
    if let Some(fence) = fence {
        let fence: State = serde_json::from_str(&fence).map_err(|e| Error::Internal(e.into()))?;
        fence.validate()?;
        if fence.transfer.as_ref().map(|t| t.phase) != Some(Phase::Cancelled)
            || !fence.continues_execution_of(&committed)
        {
            return Ok(None);
        }
    }
    let local = crate::cloud_transfer::peer(daemon)?.peer_id;
    Ok(
        (committed.transfer.as_ref().map(|t| t.phase) == Some(Phase::Committed)
            && committed.execution_peer_id == local)
            .then_some(committed),
    )
}

fn turn_running(daemon: &Daemon, workspace: &str) -> Result<bool> {
    Ok(daemon
        .store
        .chats_of(workspace)?
        .iter()
        .any(|c| !c.closed && matches!(c.status, ChatStatus::Working | ChatStatus::Starting)))
}

/// `cloud.backup.configure`, on the worker: where the workspace's backups go, and their key.
pub fn configure(daemon: &Daemon, params: CloudBackupConfigureParams) -> Result<()> {
    if !valid_bucket(&params.bucket) || !valid_prefix(&params.prefix) {
        return Err(Error::BadRequest(format!(
            "gs://{}/{}/ is not a backup bucket and prefix Chauffeur accepts: lowercase letters, digits, - and _",
            params.bucket, params.prefix
        )));
    }
    let row = daemon.workspaces.row(&params.workspace)?;
    let Some(committed) = running_here(daemon, &row.id)? else {
        return Err(Error::Conflict(format!(
            "{} does not run on this machine after a transfer; only a workspace moved here is backed up",
            row.slug
        )));
    };
    let key = decode_key(&params.key)?;
    daemon.backups.keep_key(&row.id, &key)?;
    let config = serde_json::to_string(&Config {
        bucket: params.bucket,
        prefix: params.prefix,
    })
    .map_err(|e| Error::Internal(e.into()))?;
    daemon.store.with(|c| {
        c.execute(
            "INSERT INTO workspace_backup_configs (workspace_id, info) VALUES (?1, ?2)
             ON CONFLICT(workspace_id) DO UPDATE SET info=excluded.info",
            params![row.id, config],
        )
    })?;
    let earlier = load(daemon, &row.id)?.unwrap_or_default();
    save(
        daemon,
        &row.id,
        &for_stay(earlier, committed.epoch, Utc::now()),
    )?;
    daemon.workspaces.publish(&row.id);
    Ok(())
}

/// On the Mac, once a workspace is active on `worker`: its backup key (made the first time) and
/// the bucket of `[workers]` go to the worker. A line for the transfer's log says what happened;
/// a failure leaves the workspace's backups failed here with why, the move itself stands.
pub async fn configure_worker(
    daemon: &Arc<Daemon>,
    workspace: &str,
    worker: &str,
    call: impl AsyncFnOnce(CloudBackupConfigureParams) -> anyhow::Result<()>,
) -> String {
    let outcome = async {
        // Where the worker was given access, not where the settings say now.
        let (bucket, prefix) = crate::workers::backup_place(daemon, worker)?;
        let key = match daemon.backups.key(workspace)? {
            Some(key) => key,
            None => {
                let key = ArchiveKey::generate();
                daemon.backups.keep_key(workspace, &key)?;
                key
            }
        };
        call(CloudBackupConfigureParams {
            workspace: workspace.to_owned(),
            key: URL_SAFE_NO_PAD.encode(key.expose_bytes()),
            bucket: bucket.clone(),
            prefix: prefix.clone(),
        })
        .await?;
        Ok::<_, anyhow::Error>(format!("gs://{bucket}/{prefix}/"))
    }
    .await;
    // Another worker's backups, or a declared machine's absence of any, are not this one's.
    let mut saved = Saved {
        configured_at: Some(Utc::now().to_rfc3339()),
        ..Saved::default()
    };
    let line = match outcome {
        Ok(place) => {
            saved.error = None;
            format!("backing it up every five minutes to {place}")
        }
        Err(error) => {
            let message = format!("{worker} could not be set up to back it up: {error:#}");
            saved.error = Some(message.clone());
            format!("  {message}")
        }
    };
    if let Err(error) = save(daemon, workspace, &saved) {
        tracing::warn!(%error, workspace, "could not keep the backups' status");
    }
    line
}

/// `cloud.backup.run`: backs the workspace up now, here, and says where its backups stand. One
/// already running is waited for, and its outcome returned: it is the backup of now.
pub async fn run(daemon: &Arc<Daemon>, workspace: &str) -> Result<BackupStatus> {
    run_when(daemon, workspace, true).await
}

/// Runs a backup unless one runs already; then waits for it and returns its outcome when `wait`,
/// or returns at once with the status as it is.
async fn run_when(daemon: &Arc<Daemon>, workspace: &str, wait: bool) -> Result<BackupStatus> {
    let row = daemon.workspaces.row(workspace)?;
    let Some(config) = load_config(daemon, &row.id)? else {
        return Err(Error::Conflict(format!(
            "{} is not backed up on this machine: only a workspace moved to a GCP worker is",
            row.slug
        )));
    };
    let Some(committed) = running_here(daemon, &row.id)? else {
        return Err(Error::Conflict(format!(
            "{} no longer runs on this machine; its backups stopped here",
            row.slug
        )));
    };
    let lock = daemon
        .backups
        .running
        .lock()
        .unwrap()
        .entry(row.id.clone())
        .or_default()
        .clone();
    let _held = match lock.try_lock() {
        Ok(held) => held,
        Err(_) => {
            if wait {
                drop(lock.lock().await);
            }
            let saved = load(daemon, &row.id)?.unwrap_or_default();
            return Ok(status(&saved, Utc::now()));
        }
    };
    // A stay here whose configuration missed the transfer's epoch still starts afresh.
    let mut saved = for_stay(
        load(daemon, &row.id)?.unwrap_or_default(),
        committed.epoch,
        Utc::now(),
    );
    // Left by a daemon that stopped while it was written: it never counted.
    if let Some(object) = saved.uploading.take() {
        saved.doomed.push(object);
    }
    let now = Utc::now();
    saved.last_attempt_at = Some(now.to_rfc3339());
    match attempt(daemon, &row.id, &config, &committed, &mut saved).await {
        Ok(()) => {
            saved.error = None;
            // When it ended: the backup it found or took holds the workspace as of then.
            saved.checked_at = Some(Utc::now().to_rfc3339());
        }
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), workspace = row.slug, "could not back up the workspace");
            saved.error = Some(format!("{error:#}"));
        }
    }
    save(daemon, &row.id, &saved)?;
    daemon.workspaces.publish(&row.id);
    Ok(status(&saved, Utc::now()))
}

async fn attempt(
    daemon: &Arc<Daemon>,
    workspace: &str,
    config: &Config,
    committed: &State,
    saved: &mut Saved,
) -> anyhow::Result<()> {
    let key = daemon
        .backups
        .key(workspace)?
        .context("the workspace's backup key is missing on this machine; transfer it again")?;
    let fingerprint = package::fingerprint(daemon, workspace).await?;
    if still_holds(saved, &fingerprint, Utc::now()) {
        // Current only while storage still has it: deleted behind the worker's back, and its
        // seven days of soft delete gone, it would recover nothing.
        let newest = saved.backups[0].clone();
        match held(daemon, &newest.object, newest.bytes).await {
            Ok(()) => {
                purge(daemon, saved).await;
                return Ok(());
            }
            Err(error) if gone(&format!("{error:#}")) => {
                tracing::warn!(
                    object = newest.object,
                    "the newest backup is gone from storage; taking another"
                );
                saved.backups.remove(0);
            }
            Err(error) => return Err(error),
        }
    }
    let live = turn_running(daemon, workspace)?;
    let id = uuid::Uuid::now_v7().to_string();
    let staging = daemon.paths.home.join("backups").join(&id);
    std::os::unix::fs::DirBuilderExt::mode(std::fs::DirBuilder::new().recursive(true), 0o700)
        .create(&staging)
        .context("could not make the backup's staging folder")?;
    let object = format!(
        "gs://{}/{}/{workspace}/{id}.bin",
        config.bucket, config.prefix
    );
    saved.uploading = Some(object.clone());
    save(daemon, workspace, saved)?;
    let taken = take(daemon, committed, &key, &id, live, &staging, &object).await;
    if let Err(error) = std::fs::remove_dir_all(&staging) {
        tracing::warn!(%error, path = %staging.display(), "could not remove a backup's staging folder");
    }
    saved.uploading = None;
    let taken = match taken {
        Ok(taken) => taken,
        Err(error) => {
            // A cut upload leaves nothing; one that failed after it was written leaves an object.
            saved.doomed.push(object);
            return Err(error);
        }
    };
    saved.last_success_at = Some(taken.backup.taken_at.clone());
    saved.fingerprint = Some(fingerprint);
    saved.left_out = taken.left_out;
    saved.incomplete = taken.incomplete;
    saved.backups.insert(0, taken.backup);
    // Only now that a newer one is written and read back may the oldest go.
    let extra = saved.backups.split_off(saved.backups.len().min(KEPT));
    saved.doomed.extend(extra.into_iter().map(|old| old.object));
    purge(daemon, saved).await;
    Ok(())
}

/// Whether the newest backup still holds the workspace: nothing changed since, and it carried
/// every conversation, or one it missed was tried less than an hour ago.
fn still_holds(saved: &Saved, fingerprint: &str, now: DateTime<Utc>) -> bool {
    let Some(newest) = saved.backups.first() else {
        return false;
    };
    saved.fingerprint.as_deref() == Some(fingerprint)
        && (saved.incomplete.is_empty()
            || parse_time(Some(&newest.taken_at)).is_some_and(|t| now - t < INCOMPLETE_RETRY))
}

/// Deletes the objects due to go, keeping those that would not for the next time.
async fn purge(daemon: &Daemon, saved: &mut Saved) {
    let mut still = Vec::new();
    for object in std::mem::take(&mut saved.doomed) {
        if let Err(error) = remove(daemon, &object).await {
            tracing::warn!(error = %format!("{error:#}"), object, "could not delete an old backup");
            still.push(object);
        }
    }
    saved.doomed = still;
}

/// One backup written and read back, with what it could not carry.
struct Taken {
    backup: BackupRef,
    left_out: Vec<String>,
    incomplete: Vec<String>,
}

/// What a backup is authenticated under: the committed transfer the workspace runs under on the
/// worker (which the Mac keeps as its retired copy's fence) and the backup's id (its object's
/// name). Nothing else is needed to open one, so a backup the Mac never heard of still opens.
pub fn backup_context(committed: &State, id: &str) -> PackageTransferContext {
    PackageTransferContext {
        workspace_id: committed.binding.workspace_id.clone(),
        owner_peer_id: committed.binding.owner_peer_id.clone(),
        controller_peer_id: committed.binding.controller_peer_id.clone(),
        operation_id: id.to_owned(),
        source_peer_id: committed.execution_peer_id.clone(),
        target_peer_id: String::new(),
        source_epoch: committed.epoch,
        snapshot_digest: String::new(),
        backup: true,
    }
}

async fn take(
    daemon: &Arc<Daemon>,
    committed: &State,
    key: &ArchiveKey,
    id: &str,
    live: bool,
    staging: &Path,
    object: &str,
) -> anyhow::Result<Taken> {
    let taken_at = Utc::now().to_rfc3339();
    let folder = staging.join("package");
    let (package, left_out) = package::capture_running(daemon, committed, &folder).await?;
    let incomplete = package
        .manifest
        .chats
        .iter()
        .filter_map(|chat| {
            let reason = chat.blocked.as_ref()?;
            let name = chat.info.title.as_deref().unwrap_or(&chat.info.id);
            Some(format!("{name}: {reason}"))
        })
        .collect();
    let worker = committed.execution_peer_id.clone();
    let manifest_digest = package::journal_digest(&package.digest)?;
    let context = backup_context(committed, id);
    let archive = staging.join("archive.bin");
    let sealed = archive::seal_backup(daemon, &folder, &archive, &context, key).await?;
    // The plaintext goes before anything is sent.
    std::fs::remove_dir_all(&folder)?;
    upload(daemon, &archive, object, sealed.bytes).await?;
    Ok(Taken {
        backup: BackupRef {
            id: id.to_owned(),
            object: object.to_owned(),
            taken_at,
            live,
            digest: sealed.digest,
            bytes: sealed.bytes,
            manifest_digest,
            worker_peer_id: worker,
            epoch: committed.epoch,
        },
        left_out,
        incomplete,
    })
}

async fn gcloud(daemon: &Daemon, args: &[&str], limit: Duration) -> anyhow::Result<String> {
    let what = format!("gcloud {}", args[..args.len().min(2)].join(" "));
    run_within(crate::workers::gcloud(daemon, args), &what, limit).await
}

/// Runs `command`, killed past `limit`: a storage call that hangs would hold every backup of the
/// machine, and `cloud.backup.run`, behind it.
async fn run_within(
    command: tokio::process::Command,
    what: &str,
    limit: Duration,
) -> anyhow::Result<String> {
    tokio::time::timeout(limit, crate::workers::run(command, what))
        .await
        .map_err(|_| anyhow::anyhow!("{what} did not end within {} s", limit.as_secs()))?
}

/// Writes the archive and reads its size back: a backup counts only once storage holds it whole.
/// `gcloud storage cp` checks the upload's hash; an upload cut short leaves no object.
async fn upload(daemon: &Daemon, archive: &Path, object: &str, bytes: u64) -> anyhow::Result<()> {
    let file = archive.to_str().context("non-UTF8 archive path")?;
    gcloud(
        daemon,
        &["storage", "cp", "--quiet", file, object],
        UPLOAD_LIMIT,
    )
    .await?;
    held(daemon, object, bytes).await
}

/// Whether storage holds `object` whole, `bytes` long.
async fn held(daemon: &Daemon, object: &str, bytes: u64) -> anyhow::Result<()> {
    let described = gcloud(
        daemon,
        &["storage", "objects", "describe", object, "--format=json"],
        CALL_LIMIT,
    )
    .await?;
    let described: serde_json::Value =
        serde_json::from_str(&described).context("gcloud described the backup oddly")?;
    let size = match &described["size"] {
        serde_json::Value::String(size) => size.parse().ok(),
        size => size.as_u64(),
    };
    ensure!(
        size == Some(bytes),
        "storage holds {object} with another size than the one written"
    );
    Ok(())
}

/// Deletes an object; one already gone counts as deleted.
async fn remove(daemon: &Daemon, object: &str) -> anyhow::Result<()> {
    match gcloud(daemon, &["storage", "rm", "--quiet", object], CALL_LIMIT).await {
        Err(error) if !gone(&format!("{error:#}")) => Err(error),
        _ => Ok(()),
    }
}

/// gcloud's answer for a missing object. Not a bare "404": an object's name, which the message
/// quotes, may hold one.
fn gone(error: &str) -> bool {
    ["No URLs matched", "matched no objects", "not found: 404"]
        .iter()
        .any(|gone| error.contains(gone))
}

/// What a backup the daemon stopped in the middle of left here: its plaintext package and
/// ciphertext. None runs at start, so all of it goes.
pub fn sweep(daemon: &Daemon) {
    let staging = daemon.paths.home.join("backups");
    let Ok(entries) = std::fs::read_dir(&staging) else {
        return;
    };
    for entry in entries.flatten() {
        if let Err(error) = std::fs::remove_dir_all(entry.path()) {
            tracing::warn!(%error, path = %entry.path().display(), "could not remove a backup left by a stop");
        }
    }
}

/// When a backup is due: five minutes after the last attempt, or from four minutes on as soon as
/// no agent turn runs.
fn due(since_last: Option<Duration>, turn_running: bool) -> bool {
    match since_last {
        None => true,
        Some(elapsed) => elapsed >= EVERY || (elapsed >= QUIET_FROM && !turn_running),
    }
}

/// Backs up each workspace configured here and running here when it is due, one after the other.
pub fn spawn(daemon: Arc<Daemon>) {
    sweep(&daemon);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = daemon.shutdown.cancelled() => return,
                _ = tokio::time::sleep(TICK) => {}
            }
            let workspaces = configured(&daemon).unwrap_or_else(|error| {
                tracing::warn!(%error, "could not read the workspaces to back up");
                Vec::new()
            });
            for workspace in workspaces {
                if !matches!(running_here(&daemon, &workspace), Ok(Some(_))) {
                    continue;
                }
                let last = load(&daemon, &workspace)
                    .ok()
                    .flatten()
                    .and_then(|s| parse_time(s.last_attempt_at.as_deref()));
                let since_last = last.map(|t| (Utc::now() - t).to_std().unwrap_or_default());
                if !due(
                    since_last,
                    turn_running(&daemon, &workspace).unwrap_or(true),
                ) {
                    continue;
                }
                // Its outcome is in its status; one asked for meanwhile ran already.
                let _ = run_when(&daemon, &workspace, false).await;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(minutes: i64) -> String {
        (DateTime::parse_from_rfc3339("2026-10-27T10:00:00Z").unwrap()
            + chrono::TimeDelta::minutes(minutes))
        .to_rfc3339()
    }

    #[test]
    fn a_backup_waits_for_a_quiet_moment_from_four_minutes_and_is_taken_live_at_five() {
        let minutes = |m: u64| Some(Duration::from_secs(m * 60));
        assert!(due(None, true), "the first one goes at once");
        assert!(!due(minutes(3), false));
        assert!(due(minutes(4), false), "quiet from four minutes on");
        assert!(!due(minutes(4), true), "a turn runs: it waits");
        assert!(due(minutes(5), true), "five minutes: taken live");
    }

    #[test]
    fn backups_are_pending_current_stale_or_failed() {
        let now = DateTime::parse_from_rfc3339(&at(0))
            .unwrap()
            .with_timezone(&Utc);
        let mut saved = Saved::default();
        assert_eq!(health(&saved, now), BackupHealth::Pending);
        saved.checked_at = Some(at(-14));
        assert_eq!(health(&saved, now), BackupHealth::Current);
        saved.checked_at = Some(at(-16));
        assert_eq!(health(&saved, now), BackupHealth::Stale);
        saved.error = Some("gcloud storage failed".into());
        assert_eq!(health(&saved, now), BackupHealth::Failed);
    }

    #[test]
    fn a_first_backup_awaited_too_long_is_stale() {
        let now = DateTime::parse_from_rfc3339(&at(0))
            .unwrap()
            .with_timezone(&Utc);
        let mut saved = Saved {
            configured_at: Some(at(-14)),
            ..Saved::default()
        };
        assert_eq!(health(&saved, now), BackupHealth::Pending);
        saved.configured_at = Some(at(-16));
        assert_eq!(health(&saved, now), BackupHealth::Stale);
    }

    fn backup(object: &str, epoch: u64, taken_at: String) -> BackupRef {
        BackupRef {
            id: object.into(),
            object: object.into(),
            taken_at,
            live: false,
            digest: String::new(),
            bytes: 1,
            manifest_digest: String::new(),
            worker_peer_id: "w1".into(),
            epoch,
        }
    }

    #[test]
    fn a_new_stay_starts_afresh_and_deletes_the_earlier_ones_after_its_first_backup() {
        let now = DateTime::parse_from_rfc3339(&at(0))
            .unwrap()
            .with_timezone(&Utc);
        let earlier = Saved {
            last_success_at: Some(at(-60)),
            checked_at: Some(at(-1)),
            error: Some("storage is down".into()),
            backups: vec![backup("gs://b/w1/x/1.bin", 2, at(-60))],
            fingerprint: Some("unchanged".into()),
            uploading: Some("gs://b/w1/x/2.bin".into()),
            doomed: vec!["gs://b/w1/x/0.bin".into()],
            epoch: Some(2),
            ..Saved::default()
        };
        let same = for_stay(earlier.clone(), 2, now);
        assert_eq!(same.backups, earlier.backups, "the same stay keeps its own");
        let fresh = for_stay(earlier, 4, now);
        assert!(fresh.backups.is_empty() && fresh.fingerprint.is_none());
        assert!(fresh.checked_at.is_none() && fresh.error.is_none());
        assert_eq!(fresh.epoch, Some(4));
        assert_eq!(health(&fresh, now), BackupHealth::Pending);
        assert_eq!(
            fresh.doomed,
            vec![
                "gs://b/w1/x/0.bin".to_string(),
                "gs://b/w1/x/2.bin".into(),
                "gs://b/w1/x/1.bin".into()
            ]
        );
    }

    #[test]
    fn a_conversation_left_out_is_tried_again_hourly_only_when_nothing_else_changed() {
        let now = DateTime::parse_from_rfc3339(&at(0))
            .unwrap()
            .with_timezone(&Utc);
        let mut saved = Saved {
            backups: vec![backup("gs://b/w1/x/1.bin", 2, at(-10))],
            fingerprint: Some("same".into()),
            ..Saved::default()
        };
        assert!(still_holds(&saved, "same", now));
        assert!(!still_holds(&saved, "changed", now));
        saved.incomplete = vec!["chat: too large".into()];
        assert!(still_holds(&saved, "same", now), "not every five minutes");
        saved.backups[0].taken_at = at(-61);
        assert!(!still_holds(&saved, "same", now), "but each hour");
        saved.backups.clear();
        assert!(!still_holds(&saved, "same", now));
    }

    #[test]
    fn only_gclouds_answer_for_a_missing_object_says_it_is_gone() {
        assert!(gone(
            "ERROR: (gcloud.storage.objects.describe) gs://b/w1/x/1.bin not found: 404."
        ));
        assert!(gone("The following URLs matched no objects or files"));
        assert!(!gone(
            "ERROR: gs://b/w1-404a/x/1.bin: 403 Permission denied"
        ));
    }

    #[tokio::test]
    async fn a_storage_call_that_hangs_is_killed_at_its_limit() {
        let started = std::time::Instant::now();
        let mut sleep = tokio::process::Command::new("sleep");
        sleep.arg("30").kill_on_drop(true);
        let error = run_within(sleep, "gcloud storage", Duration::from_millis(200))
            .await
            .unwrap_err();
        assert!(
            format!("{error:#}").contains("did not end within"),
            "{error:#}"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn only_plain_bucket_names_and_prefixes_are_accepted() {
        assert!(valid_bucket("chauffeur-backups"));
        assert!(!valid_bucket("Chauffeur"));
        assert!(!valid_bucket("a.b.example.com"));
        assert!(!valid_bucket("-backups"));
        assert!(valid_prefix("vm-1"));
        assert!(!valid_prefix("vm/../other"));
        assert!(!valid_prefix(""));
    }
}
