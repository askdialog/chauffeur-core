//! Source-local capture/seal ownership after authenticated complete-stop evidence.
//! No registered-stop result, local digest or prepared archive advances the controller journal.
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use chauffeur_protocol::cloud_archive::CloudArchiveBinding;

use crate::{
    Daemon, Error, Result, cloud_workspace_archive as archive,
    cloud_workspace_artifact as artifact,
    cloud_workspace_handoff::{Phase, State},
    cloud_workspace_package as package, cloud_workspace_snapshot as snapshot,
    workspace_execution::PreparationAttempt,
};

/// Owns the preparation barrier while the authenticated controller selects this snapshot.
/// Dropping it leaves durable uncertainty; it neither resumes the source nor deletes files.
pub struct CapturedSource {
    daemon: Arc<Daemon>,
    stopping: State,
    stopped: State,
    root: PathBuf,
    digest: String,
    blocked_chats: BTreeMap<String, String>,
    attempt: PreparationAttempt,
}
#[derive(Debug)]
pub struct PreparedSource {
    pub binding: CloudArchiveBinding,
}
fn changed() -> Error {
    Error::Conflict("Source preparation changed or remains uncertain; retain its source, package and key before reconciling the operation".into())
}
fn stopped_after(stopping: &State, stopped: &State) -> Result<()> {
    stopping.validate()?;
    stopped.validate()?;
    if stopping.transfer.as_ref().map(|t| t.phase) != Some(Phase::Stopping)
        || stopped.transfer.as_ref().map(|t| t.phase) != Some(Phase::Stopped)
    {
        return Err(changed());
    }
    let mut expected = stopping.clone();
    expected.revision = expected.revision.checked_add(1).ok_or_else(changed)?;
    let transfer = expected.transfer.as_mut().ok_or_else(changed)?;
    transfer.phase = Phase::Stopped;
    transfer.source_stop_digest = stopped
        .transfer
        .as_ref()
        .ok_or_else(changed)?
        .source_stop_digest
        .clone();
    if expected != *stopped {
        return Err(changed());
    }
    Ok(())
}

/// Trusted coordinator supplies the authenticated current Stopped journal, whose digest names a
/// complete-stop receipt this daemon stored (`workspace_source_stop::stop_source`). Registered
/// stops alone cannot call this boundary. No public RPC accepts a caller's claim of complete quiescence.
/// Once accepted, the host task continues after its observer disconnects.
pub async fn capture(
    daemon: Arc<Daemon>,
    stopping: State,
    stopped: State,
) -> Result<CapturedSource> {
    stopped_after(&stopping, &stopped)?;
    let digest = stopped
        .transfer
        .as_ref()
        .and_then(|t| t.source_stop_digest.as_deref())
        .ok_or_else(changed)?;
    crate::workspace_source_stop::require_receipt(&daemon, &stopping, digest)?;
    let attempt = daemon.execution.begin_preparation(&stopping)?;
    let task = tokio::spawn(async move {
        daemon.execution.require_settled_stop(&stopping)?;
        let operation = &stopping.transfer.as_ref().ok_or_else(changed)?.operation_id;
        let root = daemon
            .paths
            .home
            .canonicalize()?
            .join(format!("workspace-preparation-{operation}"));
        // Left by an earlier attempt of this operation, settled since (it failed here, or its
        // daemon stopped): nothing reads it any more, and this capture makes it again.
        if root.exists() {
            std::fs::remove_dir_all(&root)?;
        }
        snapshot::private_dir(&root)?;
        std::fs::File::open(root.parent().ok_or_else(changed)?)?.sync_all()?;
        let captured = package::capture(&daemon, &stopping, &root.join("package")).await?;
        daemon.execution.require_settled_stop(&stopping)?;
        // What leaves: a copy this machine keeps is compared with it when the workspace returns.
        let contents = serde_json::to_string(&captured.contents)?;
        daemon.store.with(|c| {
            c.execute(
                "INSERT INTO workspace_departures(workspace_id,contents) VALUES(?1,?2) ON CONFLICT(workspace_id) DO UPDATE SET contents=excluded.contents",
                rusqlite::params![stopping.binding.workspace_id, contents],
            )
        })?;
        let digest = package::journal_digest(&captured.digest)?;
        let blocked_chats = captured
            .manifest
            .chats
            .iter()
            .filter_map(|chat| {
                chat.blocked
                    .as_ref()
                    .map(|reason| (chat.info.id.clone(), reason.clone()))
            })
            .collect();
        Ok((daemon, stopping, stopped, root, digest, blocked_chats))
    })
    .await
    // A panic leaves the attempt pending, for explicit recovery.
    .map_err(|e| Error::Internal(e.into()))?;
    match task {
        Ok((daemon, stopping, stopped, root, digest, blocked_chats)) => Ok(CapturedSource {
            daemon,
            stopping,
            stopped,
            root,
            digest,
            blocked_chats,
            attempt,
        }),
        // Its task ended, so none of its work can still act: settled, the transfer can be
        // resumed or cancelled at once.
        Err(error) => {
            attempt.finish()?;
            Err(error)
        }
    }
}

impl CapturedSource {
    pub fn snapshot_digest(&self) -> &str {
        &self.digest
    }
    pub fn blocked_chats(&self) -> &BTreeMap<String, String> {
        &self.blocked_chats
    }

    /// Explicit authenticated pre-commit cancellation after capture has completed. No async work
    /// remains in this capability; settle it before asking the existing gate to resume locally.
    /// Retained package files are left for separately authorized cleanup.
    pub fn cancel(self, cancelled: State) -> Result<()> {
        cancelled.validate()?;
        let snapshot = cancelled
            .transfer
            .as_ref()
            .ok_or_else(changed)?
            .snapshot_digest
            .clone();
        if cancelled.revision <= self.stopped.revision
            || snapshot
                .as_ref()
                .is_some_and(|digest| digest != &self.digest)
        {
            return Err(changed());
        }
        let mut expected = self.stopped.clone();
        expected.revision = cancelled.revision;
        let transfer = expected.transfer.as_mut().ok_or_else(changed)?;
        transfer.phase = Phase::Cancelled;
        transfer.snapshot_digest = snapshot;
        if expected != cancelled {
            return Err(changed());
        }
        self.attempt.finish()?;
        self.daemon.execution.release_cancelled(&cancelled)
    }

    /// The coordinator authenticates the exact SnapshotReady acknowledgement before calling.
    /// A protected key reservation precedes sidecar encryption. Only verified, durable staged
    /// ciphertext settles this local attempt; every error retains its pending recovery barrier.
    pub async fn seal(self, published: State) -> Result<PreparedSource> {
        published.validate()?;
        let mut expected = self.stopped.clone();
        expected.revision = expected.revision.checked_add(1).ok_or_else(changed)?;
        let transfer = expected.transfer.as_mut().ok_or_else(changed)?;
        transfer.phase = Phase::SnapshotReady;
        transfer.snapshot_digest = Some(self.digest.clone());
        if published != expected {
            return Err(changed());
        }
        tokio::spawn(async move {
            let Self {
                daemon,
                stopping,
                root,
                attempt,
                ..
            } = self;
            daemon.execution.require_settled_stop(&stopping)?;
            let context = archive::transfer_context(&published)?;
            let selected = context.clone();
            let owner = daemon.clone();
            let key = tokio::task::spawn_blocking(move || {
                owner.preparation_keys.reserve(
                    &owner.cloud_vault,
                    &owner.cloud_vault_keys,
                    &selected,
                    &selected.source_peer_id,
                )
            })
            .await
            .map_err(|e| Error::Internal(e.into()))??;
            let head = archive::seal(
                &daemon,
                &root.join("package"),
                &root.join("archive.bin"),
                &published,
                &key,
            )
            .await?;
            daemon.execution.require_settled_stop(&stopping)?;
            let owner = daemon.clone();
            let state = published.clone();
            let selected_head = head.clone();
            tokio::task::spawn_blocking(move || {
                artifact::stage_source(
                    &owner.paths.home,
                    &root.join("archive.bin"),
                    &state,
                    &state.transfer.as_ref().ok_or_else(changed)?.source_peer_id,
                    &selected_head,
                )?;
                // A staged receipt is independently checked before the preparation can settle.
                artifact::source_reader(
                    &owner.paths.home,
                    &state,
                    &state.transfer.as_ref().ok_or_else(changed)?.source_peer_id,
                    &selected_head,
                )?;
                Ok::<_, Error>(())
            })
            .await
            .map_err(|e| Error::Internal(e.into()))??;
            daemon.execution.require_settled_stop(&stopping)?;
            attempt.finish()?;
            Ok(PreparedSource {
                binding: CloudArchiveBinding {
                    local_peer_id: context.source_peer_id.clone(),
                    context,
                    head,
                },
            })
        })
        .await
        .map_err(|e| Error::Internal(e.into()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_workspace_handoff::{Binding, Transfer};
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    fn states() -> (State, State) {
        let peer = |n: char| format!("12D3KooW{}", n.to_string().repeat(44));
        let stopping = State {
            version: 1,
            binding: Binding {
                workspace_id: uuid::Uuid::now_v7().to_string(),
                owner_peer_id: peer('1'),
                controller_peer_id: peer('2'),
            },
            revision: 3,
            epoch: 1,
            execution_peer_id: peer('1'),
            transfer: Some(Transfer {
                operation_id: uuid::Uuid::now_v7().to_string(),
                source_peer_id: peer('1'),
                target_peer_id: peer('3'),
                source_epoch: 1,
                phase: Phase::Stopping,
                source_stop_digest: None,
                snapshot_digest: None,
                destination_receipt_digest: None,
            }),
        };
        let mut stopped = stopping.clone();
        stopped.revision += 1;
        let t = stopped.transfer.as_mut().unwrap();
        t.phase = Phase::Stopped;
        t.source_stop_digest = Some(URL_SAFE_NO_PAD.encode([1; 32]));
        (stopping, stopped)
    }
    #[test]
    fn only_the_exact_complete_stop_acknowledgement_can_prepare_a_source() {
        let (stopping, stopped) = states();
        assert!(stopped_after(&stopping, &stopped).is_ok());
        assert!(stopped_after(&stopping, &stopping).is_err());
        for kind in 0..5 {
            let mut changed = stopped.clone();
            match kind {
                0 => changed.revision += 1,
                1 => changed.binding.owner_peer_id = format!("12D3KooW{}", "4".repeat(44)),
                2 => {
                    changed.transfer.as_mut().unwrap().operation_id =
                        uuid::Uuid::now_v7().to_string()
                }
                3 => {
                    changed.transfer.as_mut().unwrap().target_peer_id =
                        format!("12D3KooW{}", "4".repeat(44))
                }
                _ => changed.transfer.as_mut().unwrap().source_stop_digest = None,
            }
            assert!(stopped_after(&stopping, &changed).is_err());
        }
    }
}
