//! Manual recovery on the Mac from authenticated worker backups. Storage is read before any VM
//! changes; the controller's journal gates activation, and GCP must prove the old VM deleted with
//! its boot disk kept and unattached. The retained disk and backups are never cleaned up here.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, ensure};
use chauffeur_protocol::cloud_backup::{
    BackupRecoverParams, BackupRecovered, BackupRecoveryPlan, RecoveryBackup,
};
use chauffeur_protocol::cloud_transfer::{
    CloudTransferCheckParams, CloudTransferJournal, CloudTransferJournalParams, CloudTransferStep,
};
use chauffeur_protocol::peer::PeerCall;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cloud_transfer_driver::Peer;
use crate::cloud_workspace_handoff::{Phase, State};
use crate::cloud_workspace_package as package;
use crate::{Daemon, Error, Result};

#[derive(Serialize, Deserialize)]
struct Recovery {
    original: State,
    worker: String,
    backup: String,
    operation: String,
    keep_local_copy: bool,
    #[serde(default)]
    started: bool,
    affected_workspaces: Vec<String>,
    result: Option<BackupRecovered>,
}

fn request_error(error: anyhow::Error) -> Error {
    match error.downcast::<Error>() {
        Ok(error) => error,
        Err(error) => Error::Internal(error),
    }
}

fn key(workspace: &str) -> String {
    format!("backup-recovery:{workspace}")
}

fn load(daemon: &Daemon, workspace: &str) -> Result<Option<Recovery>> {
    let saved: Option<Recovery> = daemon
        .store
        .kv_get(&key(workspace))?
        .map(|raw| serde_json::from_str(&raw).map_err(|e| Error::Internal(e.into())))
        .transpose()?;
    if let Some(saved) = &saved
        && saved.result.is_some()
        && crate::cloud_workspace_return::committed_fence(daemon, workspace)
            .map_err(Error::Internal)?
            .is_some_and(|(_, state)| state.epoch != saved.original.epoch)
    {
        return Ok(None);
    }
    Ok(saved)
}

/// The workspace remains remote/quarantined in clients until recovery has activated here. In
/// particular, replacing its retired row must not hide the recovery action after an interruption.
pub(crate) fn pending_worker(daemon: &Daemon, workspace: &str) -> Option<String> {
    load(daemon, workspace)
        .ok()
        .flatten()
        .filter(|saved| saved.started && saved.result.is_none())
        .map(|saved| saved.worker)
}

fn save(daemon: &Daemon, recovery: &Recovery) -> anyhow::Result<()> {
    daemon.store.kv_set(
        &key(&recovery.original.binding.workspace_id),
        &serde_json::to_string(recovery)?,
    )?;
    Ok(())
}

fn origin(daemon: &Daemon, workspace: &str) -> anyhow::Result<(String, State)> {
    if let Some(saved) = load(daemon, workspace)? {
        ensure!(
            saved.result.is_none(),
            Error::Conflict("this recovery completed already; read the workspace here".into())
        );
        return Ok((saved.worker, saved.original));
    }
    let worker = crate::cloud_transfer_driver::moved_to(daemon, workspace).ok_or_else(|| {
        Error::Conflict(
            "This workspace has not moved to a GCP worker; only moved workspaces can be recovered"
                .into(),
        )
    })?;
    ensure!(
        crate::workers::is_worker(daemon, &worker),
        Error::Conflict(format!(
            "{worker} is not a GCP worker; nothing proves it stopped"
        ))
    );
    let (_, committed) = crate::cloud_workspace_return::committed_fence(daemon, workspace)?
        .context("the retired copy has no committed transfer; reconcile it before recovering")?;
    ensure!(
        committed.binding.owner_peer_id == crate::cloud_transfer::peer(daemon)?.peer_id,
        Error::Conflict("only the Mac that owns this workspace can recover its backup".into())
    );
    Ok((worker, committed))
}

fn affected(daemon: &Daemon, worker: &str) -> Result<Vec<String>> {
    let mut names: Vec<String> = daemon
        .store
        .list_workspaces()?
        .into_iter()
        .filter(|w| {
            crate::cloud_transfer_driver::moved_to(daemon, &w.id).as_deref() == Some(worker)
        })
        .map(|w| w.slug)
        .collect();
    names.sort();
    Ok(names)
}

async fn storage(daemon: &Daemon, args: &[&str]) -> anyhow::Result<String> {
    tokio::time::timeout(
        Duration::from_secs(if args.get(1) == Some(&"cp") {
            30 * 60
        } else {
            120
        }),
        crate::workers::run(
            crate::workers::gcloud(daemon, args),
            "reading backup storage",
        ),
    )
    .await
    .context("backup storage timed out; retry when storage is reachable")?
}

fn backup_ids(list: &str, prefix: &str) -> Vec<RecoveryBackup> {
    let mut backups: Vec<_> = list
        .lines()
        .filter_map(|line| {
            let id = line.trim().strip_prefix(prefix)?.strip_suffix(".bin")?;
            let uuid = uuid::Uuid::parse_str(id).ok()?;
            if uuid.to_string() != id || uuid.get_version_num() != 7 {
                return None;
            }
            let (secs, nanos) = uuid.get_timestamp()?.to_unix();
            let taken_at =
                chrono::DateTime::from_timestamp(secs.try_into().ok()?, nanos)?.to_rfc3339();
            Some(RecoveryBackup {
                id: id.into(),
                taken_at,
            })
        })
        .collect();
    backups.sort_by(|a, b| b.id.cmp(&a.id));
    backups.dedup_by(|a, b| a.id == b.id);
    backups
}

pub async fn plan(daemon: &Daemon, workspace: &str) -> Result<BackupRecoveryPlan> {
    plan_inner(daemon, workspace).await.map_err(request_error)
}

async fn plan_inner(daemon: &Daemon, workspace: &str) -> anyhow::Result<BackupRecoveryPlan> {
    let row = daemon.workspaces.row(workspace)?;
    let (worker, original) = origin(daemon, &row.id)?;
    let (bucket, prefix) = crate::workers::backup_place(daemon, &worker)?;
    let prefix = format!("gs://{bucket}/{prefix}/{}/", row.id);
    let listed = match storage(daemon, &["storage", "ls", &format!("{prefix}*.bin")]).await {
        Ok(listed) => listed,
        Err(error)
            if format!("{error:#}").contains("matched no objects")
                || format!("{error:#}").contains("No URLs matched") =>
        {
            String::new()
        }
        Err(error) => return Err(error),
    };
    let mut affected_workspaces = affected(daemon, &worker)?;
    // After the quarantine replaces the retired copy it no longer reads as moved. Its original
    // confirmation is still needed to resume a recovery stopped before activation.
    if let Some(saved) = load(daemon, &row.id)? {
        affected_workspaces.extend(saved.affected_workspaces);
        affected_workspaces.sort();
        affected_workspaces.dedup();
    }
    Ok(BackupRecoveryPlan {
        workspace: row.id,
        epoch: original.epoch,
        affected_workspaces,
        worker,
        backups: backup_ids(&listed, &prefix),
    })
}

static RECOVERING: Mutex<BTreeSet<(PathBuf, String)>> = Mutex::new(BTreeSet::new());
struct Held((PathBuf, String));
impl Drop for Held {
    fn drop(&mut self) {
        RECOVERING.lock().unwrap().remove(&self.0);
    }
}

/// A retry resumes the same operation, backup and destination; it never selects a newer backup or
/// reactivates the old source. Only this daemon's own state is written.
pub async fn recover(daemon: &Arc<Daemon>, params: BackupRecoverParams) -> Result<BackupRecovered> {
    let row = daemon.workspaces.row(&params.workspace)?;
    let scope = (daemon.paths.home.clone(), row.id.clone());
    if !RECOVERING.lock().unwrap().insert(scope.clone()) {
        return Err(Error::Conflict(format!(
            "{} is being recovered; wait for it to end",
            row.slug
        )));
    }
    let _held = Held(scope);
    recover_inner(daemon, &row.id, params)
        .await
        .map_err(request_error)
}

async fn recover_inner(
    daemon: &Arc<Daemon>,
    workspace: &str,
    params: BackupRecoverParams,
) -> anyhow::Result<BackupRecovered> {
    let mut saved = match load(daemon, workspace)? {
        Some(mut saved) => {
            ensure!(
                saved.worker == params.worker && saved.original.epoch == params.epoch,
                Error::Conflict(
                    "the recovery plan changed; read it again before confirming".into()
                )
            );
            if let Some(result) = saved.result {
                return Ok(result);
            }
            if saved.backup != params.backup
                || saved.keep_local_copy != params.keep_local_copy
                || saved.affected_workspaces != params.affected_workspaces
            {
                let controller = daemon.transfer_controller.socket(daemon, None).await?;
                let mut peer = Peer::connect(&controller).await?;
                let current = journal(
                    &mut peer,
                    CloudTransferJournalParams::Read {
                        workspace_id: workspace.into(),
                        owner_peer_id: saved.original.binding.owner_peer_id.clone(),
                    },
                )
                .await?;
                ensure!(
                    current == saved.original,
                    Error::Conflict(format!(
                        "recovery started already: retry backup {} with the same local-copy choice",
                        saved.backup
                    ))
                );
                saved.backup = params.backup.clone();
                saved.operation = uuid::Uuid::now_v7().to_string();
                saved.keep_local_copy = params.keep_local_copy;
                saved.affected_workspaces = params.affected_workspaces.clone();
            }
            saved
        }
        None => {
            let (worker, original) = origin(daemon, workspace)?;
            ensure!(
                worker == params.worker && original.epoch == params.epoch,
                Error::Conflict(
                    "the workspace moved since confirmation; read its recovery plan again".into()
                )
            );
            Recovery {
                original,
                worker,
                backup: params.backup.clone(),
                operation: uuid::Uuid::now_v7().to_string(),
                keep_local_copy: params.keep_local_copy,
                started: false,
                affected_workspaces: params.affected_workspaces.clone(),
                result: None,
            }
        }
    };
    let vm = crate::workers::recovery_worker(daemon, &saved.worker)?;
    let mut accepted = params.affected_workspaces;
    accepted.sort();
    ensure!(
        affected(daemon, &saved.worker)?
            .iter()
            .all(|w| accepted.contains(w)),
        Error::Conflict("the workspaces affected by deleting this worker changed; confirm its recovery plan again".into())
    );
    ensure!(
        !crate::cloud_transfer_driver::running(daemon)
            .iter()
            .any(|t| t.source == saved.worker || t.destination == saved.worker),
        Error::Conflict(
            "a transfer uses this worker; finish or cancel it before recovering".into()
        )
    );
    let backup = uuid::Uuid::parse_str(&saved.backup).map_err(|_| {
        Error::BadRequest("Invalid backup id; select a backup from its recovery plan".into())
    })?;
    ensure!(
        backup.to_string() == saved.backup && backup.get_version_num() == 7,
        Error::Conflict("invalid backup id".into())
    );
    let controller = daemon.transfer_controller.socket(daemon, None).await?;
    let mut controller = Peer::connect(&controller).await?;
    let mut current = journal(
        &mut controller,
        CloudTransferJournalParams::Read {
            workspace_id: workspace.into(),
            owner_peer_id: saved.original.binding.owner_peer_id.clone(),
        },
    )
    .await?;
    let is_ours = |s: &State| {
        s.transfer
            .as_ref()
            .is_some_and(|t| t.operation_id == saved.operation)
    };
    ensure!(
        current == saved.original || is_ours(&current),
        Error::Conflict("the controller's journal changed; reconcile it before recovering".into())
    );
    save(daemon, &saved)?;
    let staging = daemon
        .paths
        .home
        .join("backup-recoveries")
        .join(&saved.operation);
    std::os::unix::fs::DirBuilderExt::mode(std::fs::DirBuilder::new().recursive(true), 0o700)
        .create(&staging)?;
    let original_package = staging.join("backup");
    if !original_package.exists() {
        let (bucket, prefix) = crate::workers::backup_place(daemon, &saved.worker)?;
        let object = format!("gs://{bucket}/{prefix}/{workspace}/{}.bin", saved.backup);
        let archive = staging.join("archive.bin");
        let head: serde_json::Value = serde_json::from_str(
            &storage(
                daemon,
                &["storage", "objects", "describe", &object, "--format=json"],
            )
            .await?,
        )?;
        let (version, bytes) = download_version(&object, &head)?;
        let free = crate::cloud_transfer::free_bytes(&daemon.paths.home).context(
            "could not read free space for recovery; inspect the Mac's disk before retrying",
        )?;
        ensure!(
            free >= bytes.saturating_mul(3).saturating_add(1024 * 1024 * 1024),
            Error::Conflict("not enough free space for this backup's archive, package and restored files; free space before retrying".into())
        );
        storage(
            daemon,
            &[
                "storage",
                "cp",
                "--quiet",
                &version,
                archive.to_str().context("invalid staging path")?,
            ],
        )
        .await?;
        let key = daemon
            .backups
            .key(workspace)?
            .context("the Mac's backup key is missing; restore its Keychain before recovering")?;
        let context = crate::cloud_workspace_backup::backup_context(&saved.original, &saved.backup);
        if let Err(error) = crate::cloud_workspace_archive::open_backup(
            daemon,
            &archive,
            &original_package,
            &context,
            &key,
        )
        .await
        {
            // Only this newly opened, never installed package; retries decrypt the ciphertext again.
            if original_package.exists() {
                std::fs::remove_dir_all(&original_package)?;
            }
            return Err(error.into());
        }
    }
    let verified = package::verify(&original_package)?;
    ensure!(
        verified.manifest.running && verified.manifest.source == saved.original,
        Error::Conflict(
            "the backup does not belong to the workspace's current stay on this worker".into()
        )
    );
    // Backups currently omit databases. Never turn their declarations into empty replacements.
    ensure!(
        verified
            .manifest
            .development_profiles
            .iter()
            .all(|p| p.profile.services.is_empty()),
        Error::Conflict("this backup has development services but no database exports; recover its kept disk manually instead".into())
    );
    let local = crate::cloud_transfer::peer(daemon)?.peer_id;
    if current == saved.original {
        let check = crate::cloud_transfer::destination_check(
            daemon,
            CloudTransferCheckParams {
                estimated_bytes: 0,
                workspace_id: workspace.into(),
                slug: verified.manifest.workspace.slug.clone(),
                source_peer_id: saved.original.execution_peer_id.clone(),
                agents: verified
                    .manifest
                    .chats
                    .iter()
                    .map(|c| c.info.agent)
                    .collect(),
                journal: Some(serde_json::to_value(&saved.original)?),
                keep_local_copy: saved.keep_local_copy,
                compose_projects: Vec::new(),
                sign_in_from_caller: false,
            },
        )
        .await?;
        ensure!(
            check.problems.is_empty(),
            Error::Conflict(format!(
                "cannot recover here: {}",
                check.problems.join("; ")
            ))
        );
        current = journal(
            &mut controller,
            CloudTransferJournalParams::Prepare {
                workspace_id: workspace.into(),
                owner_peer_id: saved.original.binding.owner_peer_id.clone(),
                revision: current.revision,
                operation_id: saved.operation.clone(),
                target_peer_id: local.clone(),
            },
        )
        .await?;
    }
    saved.started = true;
    save(daemon, &saved)?;
    if phase(&current)? == Phase::Preparing {
        current = advance(&mut controller, &current, CloudTransferStep::BeginStop).await?;
    }
    let retained_disk = crate::workers::fence_for_recovery(&vm).await?;
    let stopping = if phase(&current)? == Phase::Stopping {
        current.clone()
    } else {
        // `prepare` increments once, then `begin_stop` once; reconstruction is checked by the
        // package verifier against the journal's later snapshot state on every retry.
        let mut stopping = current.clone();
        stopping.revision = saved.original.revision + 2;
        stopping.epoch = saved.original.epoch;
        stopping.execution_peer_id = saved.original.execution_peer_id.clone();
        let t = stopping
            .transfer
            .as_mut()
            .context("missing recovery operation")?;
        t.phase = Phase::Stopping;
        t.source_stop_digest = None;
        t.snapshot_digest = None;
        t.destination_receipt_digest = None;
        stopping
    };
    let restored_package = staging.join("package");
    if !restored_package.exists() {
        let partial = staging.join("package-partial");
        if partial.exists() {
            std::fs::remove_dir_all(&partial)?;
        }
        copy_package(&original_package, &partial)?;
        let mut manifest = verified.manifest;
        manifest.source = stopping.clone();
        std::fs::write(
            partial.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        package::verify(&partial)?;
        std::fs::rename(partial, &restored_package)?;
    }
    let digest = package::journal_digest(&package::verify(&restored_package)?.digest)?;
    if phase(&current)? == Phase::Stopping {
        let receipt = chauffeur_protocol::cloud_backup::BackupRecovered {
            workspace_path: workspace.into(),
            retained_disk: retained_disk.clone(),
            kept_copy: None,
        };
        let evidence = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            Sha256::digest(serde_json::to_vec(&receipt)?),
        );
        current = advance(
            &mut controller,
            &current,
            CloudTransferStep::SourceStopped {
                peer_id: saved.original.execution_peer_id.clone(),
                receipt_digest: evidence,
            },
        )
        .await?;
    }
    if phase(&current)? == Phase::Stopped {
        current = advance(
            &mut controller,
            &current,
            CloudTransferStep::SnapshotPublished {
                peer_id: saved.original.execution_peer_id.clone(),
                snapshot_digest: digest.clone(),
            },
        )
        .await?;
    }
    if phase(&current)? == Phase::SnapshotReady {
        let restored = crate::cloud_workspace_restore::restore(
            daemon,
            &restored_package,
            &current,
            &local,
            saved.keep_local_copy,
        )
        .await?;
        current = advance(
            &mut controller,
            &current,
            CloudTransferStep::DestinationRestored {
                peer_id: local,
                snapshot_digest: digest.clone(),
                receipt_digest: restored.digest,
            },
        )
        .await?;
    }
    if phase(&current)? == Phase::Restored {
        crate::workers::fence_for_recovery(&vm).await?;
        let receipt = current
            .transfer
            .as_ref()
            .and_then(|t| t.destination_receipt_digest.clone())
            .context("missing restoration receipt")?;
        current = advance(
            &mut controller,
            &current,
            CloudTransferStep::Commit {
                snapshot_digest: digest,
                destination_receipt_digest: receipt,
            },
        )
        .await?;
    }
    ensure!(
        phase(&current)? == Phase::Committed,
        Error::Conflict(
            "recovery was cancelled; retain its disk and reconcile the controller before retrying"
                .into()
        )
    );
    crate::workers::fence_for_recovery(&vm).await?;
    crate::cloud_transfer::destination_activate(daemon, serde_json::to_value(&current)?, false)?;
    let row = daemon.workspaces.row(workspace)?;
    let receipt: Option<String> = daemon.store.with(|c| {
        c.query_row(
            "SELECT receipt FROM workspace_restores WHERE workspace_id=?1",
            [workspace],
            |r| r.get(0),
        )
    })?;
    let kept_copy = receipt
        .and_then(|raw| serde_json::from_str::<crate::cloud_workspace_restore::Restored>(&raw).ok())
        .and_then(|r| r.kept_copy);
    let result = BackupRecovered {
        workspace_path: row.path,
        retained_disk,
        kept_copy,
    };
    saved.result = Some(result.clone());
    save(daemon, &saved)?;
    crate::cloud_workspace_backup::forget(daemon, workspace);
    daemon.workspaces.publish(workspace);
    // Only staging of the completed recovery. Its receipt, worktrees and retained disk stay.
    if let Err(error) = std::fs::remove_dir_all(staging) {
        tracing::warn!(%error, workspace, "could not remove the completed recovery's staging");
    }
    Ok(result)
}

/// Download the size-checked generation, never a replacement object uploaded after the check.
fn download_version(object: &str, head: &serde_json::Value) -> anyhow::Result<(String, u64)> {
    let number = |v: &serde_json::Value| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    };
    let bytes = number(&head["size"]).context("storage described no backup size")?;
    ensure!(
        (40..=crate::cloud_workspace_archive::MAX_CIPHERTEXT).contains(&bytes),
        Error::Conflict(
            "the backup exceeds the supported archive size; inspect storage before recovering"
                .into()
        )
    );
    let generation =
        number(&head["generation"]).context("storage described no backup generation")?;
    Ok((format!("{object}#{generation}"), bytes))
}

fn copy_package(source: &Path, target: &Path) -> anyhow::Result<()> {
    std::fs::create_dir(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        ensure!(
            !ty.is_symlink(),
            Error::Conflict("a backup package contains a symbolic link".into())
        );
        if ty.is_dir() {
            copy_package(&entry.path(), &target.join(entry.file_name()))?;
        } else {
            ensure!(
                ty.is_file(),
                Error::Conflict("a backup package contains a special file".into())
            );
            std::fs::copy(entry.path(), target.join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn phase(state: &State) -> anyhow::Result<Phase> {
    state
        .transfer
        .as_ref()
        .map(|t| t.phase)
        .context("the recovery has no journal operation")
}

async fn journal(peer: &mut Peer, ask: CloudTransferJournalParams) -> anyhow::Result<State> {
    let answer: CloudTransferJournal =
        serde_json::from_value(peer.peer_value(PeerCall::CloudTransferJournal(ask)).await?)?;
    serde_json::from_value(
        answer
            .state
            .context("the controller's journal changed; read it before retrying")?,
    )
    .context("invalid journal state")
}

async fn advance(peer: &mut Peer, state: &State, step: CloudTransferStep) -> anyhow::Result<State> {
    let t = state
        .transfer
        .as_ref()
        .context("missing recovery operation")?;
    journal(
        peer,
        CloudTransferJournalParams::Advance {
            workspace_id: state.binding.workspace_id.clone(),
            owner_peer_id: state.binding.owner_peer_id.clone(),
            revision: state.revision,
            operation_id: t.operation_id.clone(),
            source_epoch: t.source_epoch,
            step,
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_interrupted_recovery_keeps_its_remote_action_until_activation_completes() {
        let temp = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::new(
            temp.path().join("home"),
            temp.path().join("workspaces"),
            temp.path().join("repos"),
        );
        paths.ensure().unwrap();
        std::fs::write(
            &paths.settings,
            "linear_enabled = false\n[office]\nurl = \"\"\n",
        )
        .unwrap();
        let daemon = Daemon::open(paths).unwrap();
        let workspace = uuid::Uuid::now_v7().to_string();
        let mut saved = Recovery {
            original: State {
                version: 1,
                revision: 1,
                epoch: 1,
                transfer: None,
                binding: crate::cloud_workspace_handoff::Binding {
                    workspace_id: workspace.clone(),
                    owner_peer_id: format!("12D3KooW{}", "1".repeat(44)),
                    controller_peer_id: format!("12D3KooW{}", "2".repeat(44)),
                },
                execution_peer_id: format!("12D3KooW{}", "3".repeat(44)),
            },
            worker: "w1".into(),
            backup: uuid::Uuid::now_v7().to_string(),
            operation: uuid::Uuid::now_v7().to_string(),
            keep_local_copy: false,
            started: false,
            affected_workspaces: vec!["moving".into()],
            result: None,
        };
        save(&daemon, &saved).unwrap();
        assert!(
            crate::cloud_transfer_driver::moved_to(&daemon, &workspace).is_none(),
            "a failed preflight does not override local ownership"
        );
        saved.started = true;
        save(&daemon, &saved).unwrap();
        assert_eq!(
            crate::cloud_transfer_driver::moved_to(&daemon, &workspace).as_deref(),
            Some("w1"),
            "quarantine without a retired fence still offers recovery"
        );
        saved.result = Some(BackupRecovered {
            workspace_path: "/recovered".into(),
            retained_disk: "disk".into(),
            kept_copy: None,
        });
        save(&daemon, &saved).unwrap();
        assert!(crate::cloud_transfer_driver::moved_to(&daemon, &workspace).is_none());
    }

    #[test]
    fn downloads_pin_the_checked_generation_and_refuse_an_oversized_backup() {
        let head = serde_json::json!({"generation":"42", "size":"1234"});
        assert_eq!(
            download_version("gs://b/w/id.bin", &head).unwrap(),
            ("gs://b/w/id.bin#42".into(), 1234)
        );
        assert!(
            download_version(
                "gs://b/w/id.bin",
                &serde_json::json!({"generation":"42", "size":u64::MAX})
            )
            .is_err()
        );
        assert!(download_version("gs://b/w/id.bin", &serde_json::json!({"size":1234})).is_err());
    }

    #[test]
    fn only_backup_ids_of_this_workspace_are_offered_newest_first() {
        let first = uuid::Uuid::now_v7().to_string();
        let prefix = "gs://bucket/w1/workspace/";
        let list = format!(
            "{prefix}{first}.bin\n{prefix}../other.bin\ngs://bucket/w2/workspace/{first}.bin\n{prefix}{first}.bin\n"
        );
        let ids = backup_ids(&list, prefix);
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].id, first);
    }
}
