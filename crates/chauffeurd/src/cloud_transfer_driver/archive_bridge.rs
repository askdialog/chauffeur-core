//! Connection-scoped private Unix ciphertext I/O. No paths, keys or execution authority.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chauffeur_protocol::cloud_archive::{
    CloudArchiveBlock, CloudArchiveHandle, CloudArchiveReceiveResult,
};
use chauffeur_protocol::peer::PeerCall;
use serde_json::Value;

use crate::{Error, Result, cloud_workspace_artifact as artifact};

const MAX_HANDLES: usize = 8;
enum Handle {
    Reader(artifact::Reader),
    Writer(artifact::Writer),
}
pub(crate) struct ArchiveBridge {
    home: PathBuf,
    closed: AtomicBool,
    handles: Mutex<HashMap<String, Handle>>,
}
fn uncertain() -> Error {
    Error::Conflict("Archive I/O is uncertain; retain source and output, then reconcile the operation before retrying".into())
}
fn invalid() -> Error {
    Error::BadRequest(
        "Invalid archive handle, block or range; inspect the retained operation".into(),
    )
}
fn id() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}
fn value(result: impl serde::Serialize) -> Result<Value> {
    Ok(serde_json::to_value(result)?)
}
pub(crate) fn is_archive_call(call: &PeerCall) -> bool {
    matches!(
        call,
        PeerCall::CloudArchiveSourceOpen(_)
            | PeerCall::CloudArchiveReceiveBegin(_)
            | PeerCall::CloudArchiveRead(_)
            | PeerCall::CloudArchiveWrite(_)
            | PeerCall::CloudArchiveCommit(_)
            | PeerCall::CloudArchiveClose(_)
    )
}
impl ArchiveBridge {
    pub(crate) fn new(home: &Path) -> Arc<Self> {
        Arc::new(Self {
            home: home.into(),
            closed: AtomicBool::new(false),
            handles: Mutex::new(HashMap::new()),
        })
    }
    /// The blocking task owns its descriptor until I/O settles, even after caller disconnect.
    pub(crate) async fn handle(self: &Arc<Self>, call: PeerCall) -> Result<Value> {
        let bridge = self.clone();
        tokio::task::spawn_blocking(move || bridge.run(call))
            .await
            .map_err(|e| Error::Internal(e.into()))?
    }
    fn run(&self, call: PeerCall) -> Result<Value> {
        if self.closed.load(Ordering::Acquire) {
            return Err(uncertain());
        }
        let mut handles = self.handles.lock().unwrap();
        if self.closed.load(Ordering::Acquire) {
            return Err(uncertain());
        }
        let result = match call {
            PeerCall::CloudArchiveSourceOpen(p) => {
                artifact::validate_bound(&p.context, &p.local_peer_id, &p.head, true)
                    .map_err(|_| invalid())?;
                if handles.len() >= MAX_HANDLES {
                    return Err(Error::Unavailable(
                        "Archive handle capacity reached; close the existing host handles".into(),
                    ));
                }
                let reader = artifact::source_reader_bound(
                    &self.home,
                    &p.context,
                    &p.local_peer_id,
                    &p.head,
                )
                .map_err(|_| uncertain())?;
                let handle = id();
                handles.insert(handle.clone(), Handle::Reader(reader));
                value(CloudArchiveHandle { handle })
            }
            PeerCall::CloudArchiveReceiveBegin(p) => {
                artifact::validate_bound(
                    &p.binding.context,
                    &p.binding.local_peer_id,
                    &p.binding.head,
                    false,
                )
                .map_err(|_| invalid())?;
                if handles.len() >= MAX_HANDLES {
                    return Err(Error::Unavailable(
                        "Archive handle capacity reached; close the existing host handles".into(),
                    ));
                }
                match artifact::receive_bound(
                    &self.home,
                    &p.binding.context,
                    &p.binding.local_peer_id,
                    &p.binding.head,
                    p.allow_new,
                )
                .map_err(|_| uncertain())?
                {
                    artifact::Receive::Stored(stored) => {
                        value(CloudArchiveReceiveResult::Stored { head: stored.head })
                    }
                    artifact::Receive::New(writer) => {
                        let handle = id();
                        handles.insert(handle.clone(), Handle::Writer(writer));
                        value(CloudArchiveReceiveResult::New { handle })
                    }
                }
            }
            PeerCall::CloudArchiveRead(p) => {
                let Some(Handle::Reader(reader)) = handles.remove(&p.handle) else {
                    return Err(invalid());
                };
                if p.length == 0 || p.length as usize > artifact::BLOCK_BYTES {
                    return Err(invalid());
                }
                let bytes = reader
                    .read(p.offset, p.length as usize)
                    .map_err(|_| uncertain())?;
                handles.insert(p.handle, Handle::Reader(reader));
                value(CloudArchiveBlock {
                    bytes: URL_SAFE_NO_PAD.encode(bytes),
                })
            }
            PeerCall::CloudArchiveWrite(p) => {
                let Some(Handle::Writer(mut writer)) = handles.remove(&p.handle) else {
                    return Err(invalid());
                };
                if p.bytes.is_empty() || p.bytes.len() > (artifact::BLOCK_BYTES * 4).div_ceil(3) {
                    return Err(invalid());
                }
                let bytes = URL_SAFE_NO_PAD.decode(&p.bytes).map_err(|_| invalid())?;
                if bytes.len() > artifact::BLOCK_BYTES || URL_SAFE_NO_PAD.encode(&bytes) != p.bytes
                {
                    return Err(invalid());
                }
                writer.write(p.offset, &bytes).map_err(|_| uncertain())?;
                handles.insert(p.handle, Handle::Writer(writer));
                value(serde_json::json!({}))
            }
            PeerCall::CloudArchiveCommit(p) => {
                let Some(Handle::Writer(writer)) = handles.remove(&p.handle) else {
                    return Err(invalid());
                };
                value(writer.commit().map_err(|_| uncertain())?.head)
            }
            PeerCall::CloudArchiveClose(p) => {
                handles.remove(&p.handle);
                value(serde_json::json!({}))
            }
            _ => Err(invalid()),
        };
        // Never publish a newly opened capability after the owning connection disappears.
        if self.closed.load(Ordering::Acquire) {
            handles.clear();
            return Err(uncertain());
        }
        result
    }
    pub(crate) fn close(self: &Arc<Self>) {
        self.closed.store(true, Ordering::Release);
        let bridge = self.clone();
        tokio::task::spawn_blocking(move || bridge.handles.lock().unwrap().clear());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chauffeur_protocol::cloud_archive::{
        CloudArchiveBinding, CloudArchiveReadParams, CloudArchiveReceiveParams,
        CloudArchiveWriteParams,
    };
    use chauffeur_protocol::sidecar::{PackageCryptResult, PackageTransferContext};
    use sha2::{Digest, Sha256};
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    fn binding(bytes: &[u8]) -> CloudArchiveBinding {
        let peer = |n: char| format!("12D3KooW{}", n.to_string().repeat(44));
        CloudArchiveBinding {
            context: PackageTransferContext {
                workspace_id: uuid::Uuid::now_v7().to_string(),
                owner_peer_id: peer('1'),
                controller_peer_id: peer('2'),
                operation_id: uuid::Uuid::now_v7().to_string(),
                source_peer_id: peer('1'),
                target_peer_id: peer('3'),
                source_epoch: 1,
                snapshot_digest: URL_SAFE_NO_PAD.encode([2; 32]),
                backup: false,
            },
            local_peer_id: peer('3'),
            head: PackageCryptResult {
                digest: URL_SAFE_NO_PAD.encode(Sha256::digest(bytes)),
                bytes: bytes.len() as u64,
            },
        }
    }
    fn open(b: CloudArchiveBinding) -> PeerCall {
        PeerCall::CloudArchiveReceiveBegin(CloudArchiveReceiveParams {
            binding: b,
            allow_new: true,
        })
    }
    fn write(handle: &str, offset: u64, bytes: &[u8]) -> PeerCall {
        PeerCall::CloudArchiveWrite(CloudArchiveWriteParams {
            handle: handle.into(),
            offset,
            bytes: URL_SAFE_NO_PAD.encode(bytes),
        })
    }
    async fn new_handle(bridge: &Arc<ArchiveBridge>, b: CloudArchiveBinding) -> String {
        let result = bridge.handle(open(b)).await.unwrap();
        result["handle"].as_str().unwrap().into()
    }
    fn fixture() -> (tempfile::TempDir, Arc<ArchiveBridge>) {
        let home = tempfile::tempdir().unwrap();
        fs::set_permissions(home.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let bridge = ArchiveBridge::new(home.path());
        (home, bridge)
    }

    #[tokio::test]
    async fn connection_scoped_handles_commit_and_recover_only_verified_complete_ciphertext() {
        let (home, a) = fixture();
        let b = ArchiveBridge::new(home.path());
        let bytes = vec![71; artifact::BLOCK_BYTES + 50];
        let binding = binding(&bytes);
        let handle = new_handle(&a, binding.clone()).await;
        assert!(b.handle(write(&handle, 0, b"foreign")).await.is_err());
        for (i, chunk) in bytes.chunks(artifact::BLOCK_BYTES).enumerate() {
            a.handle(write(&handle, (i * artifact::BLOCK_BYTES) as u64, chunk))
                .await
                .unwrap();
        }
        let head = a
            .handle(PeerCall::CloudArchiveCommit(CloudArchiveHandle {
                handle: handle.clone(),
            }))
            .await
            .unwrap();
        assert_eq!(head, serde_json::to_value(&binding.head).unwrap());
        assert!(a.handle(write(&handle, 0, &[0])).await.is_err());
        let stored = b.handle(open(binding.clone())).await.unwrap();
        assert_eq!(stored["kind"], "stored");
        assert_eq!(stored["head"], head);
        assert_eq!(
            fs::read(
                home.path()
                    .join("workspace-archives")
                    .join(&binding.context.operation_id)
                    .join("archive.bin")
            )
            .unwrap(),
            bytes
        );
    }

    #[tokio::test]
    async fn disconnect_preserves_a_partial_archive_and_a_fresh_connection_can_restart_it() {
        let (home, bridge) = fixture();
        let binding = binding(&[3; 80]);
        let handle = new_handle(&bridge, binding.clone()).await;
        bridge.handle(write(&handle, 0, &[3; 31])).await.unwrap();
        let guard = bridge.handles.lock().unwrap();
        bridge.close();
        assert!(bridge.closed.load(Ordering::Acquire));
        drop(guard);
        let until = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while !bridge.handles.lock().unwrap().is_empty() {
            assert!(tokio::time::Instant::now() < until);
            tokio::task::yield_now().await;
        }
        assert!(bridge.handle(open(binding.clone())).await.is_err());
        let fresh = ArchiveBridge::new(home.path());
        let restarted = new_handle(&fresh, binding.clone()).await;
        assert_eq!(
            fs::read(
                home.path()
                    .join("workspace-archives")
                    .join(&binding.context.operation_id)
                    .join("archive.bin")
            )
            .unwrap(),
            [3; 31]
        );
        fresh.handle(write(&restarted, 0, &[3; 80])).await.unwrap();
        let receipt = fresh
            .handle(PeerCall::CloudArchiveCommit(CloudArchiveHandle {
                handle: restarted,
            }))
            .await
            .unwrap();
        assert_eq!(receipt, serde_json::to_value(binding.head).unwrap());
    }

    #[tokio::test]
    async fn invalid_or_oversized_blocks_consume_the_writer_without_overwriting_its_reserved_output()
     {
        for malformed in [
            "=".into(),
            "A".repeat((artifact::BLOCK_BYTES * 4).div_ceil(3) + 1),
        ] {
            let (home, bridge) = fixture();
            let binding = binding(&[3; 80]);
            let handle = new_handle(&bridge, binding.clone()).await;
            assert!(
                bridge
                    .handle(PeerCall::CloudArchiveWrite(CloudArchiveWriteParams {
                        handle: handle.clone(),
                        offset: 0,
                        bytes: malformed
                    }))
                    .await
                    .is_err()
            );
            assert!(
                bridge
                    .handle(PeerCall::CloudArchiveCommit(CloudArchiveHandle { handle }))
                    .await
                    .is_err()
            );
            assert_eq!(
                fs::metadata(
                    home.path()
                        .join("workspace-archives")
                        .join(binding.context.operation_id)
                        .join("archive.bin")
                )
                .unwrap()
                .len(),
                0
            );
        }
    }

    #[tokio::test]
    async fn handle_limits_refuse_new_reservations_and_explicit_close_releases_capacity() {
        let (home, bridge) = fixture();
        let bytes = [7; 80];
        let mut handles = Vec::new();
        for _ in 0..MAX_HANDLES {
            handles.push(new_handle(&bridge, binding(&bytes)).await);
        }
        let next = binding(&bytes);
        assert!(matches!(
            bridge.handle(open(next.clone())).await,
            Err(Error::Unavailable(_))
        ));
        assert!(
            !home
                .path()
                .join("workspace-archives")
                .join(&next.context.operation_id)
                .exists()
        );
        bridge
            .handle(PeerCall::CloudArchiveClose(CloudArchiveHandle {
                handle: handles.pop().unwrap(),
            }))
            .await
            .unwrap();
        new_handle(&bridge, next).await;
    }

    #[tokio::test]
    async fn staged_source_descriptors_are_read_through_rpc_without_exposing_any_path() {
        let (home, bridge) = fixture();
        let bytes = vec![9; 300];
        let mut binding = binding(&bytes);
        binding.local_peer_id = binding.context.source_peer_id.clone();
        let state = crate::cloud_workspace_handoff::State {
            version: 1,
            binding: crate::cloud_workspace_handoff::Binding {
                workspace_id: binding.context.workspace_id.clone(),
                owner_peer_id: binding.context.owner_peer_id.clone(),
                controller_peer_id: binding.context.controller_peer_id.clone(),
            },
            revision: 5,
            epoch: 1,
            execution_peer_id: binding.local_peer_id.clone(),
            transfer: Some(crate::cloud_workspace_handoff::Transfer {
                operation_id: binding.context.operation_id.clone(),
                source_peer_id: binding.context.source_peer_id.clone(),
                target_peer_id: binding.context.target_peer_id.clone(),
                source_epoch: 1,
                phase: crate::cloud_workspace_handoff::Phase::SnapshotReady,
                source_stop_digest: Some(URL_SAFE_NO_PAD.encode([1; 32])),
                snapshot_digest: Some(binding.context.snapshot_digest.clone()),
                destination_receipt_digest: None,
            }),
        };
        let input = home.path().join("sealed.bin");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&input)
            .unwrap();
        file.write_all(&bytes).unwrap();
        artifact::stage_source(
            home.path(),
            &input,
            &state,
            &binding.local_peer_id,
            &binding.head,
        )
        .unwrap();
        let opened = bridge
            .handle(PeerCall::CloudArchiveSourceOpen(binding))
            .await
            .unwrap();
        let handle = opened["handle"].as_str().unwrap().to_owned();
        assert_eq!(opened.as_object().unwrap().len(), 1);
        let block = bridge
            .handle(PeerCall::CloudArchiveRead(CloudArchiveReadParams {
                handle: handle.clone(),
                offset: 10,
                length: 40,
            }))
            .await
            .unwrap();
        assert_eq!(
            URL_SAFE_NO_PAD
                .decode(block["bytes"].as_str().unwrap())
                .unwrap(),
            bytes[10..50]
        );
        bridge
            .handle(PeerCall::CloudArchiveClose(CloudArchiveHandle {
                handle: handle.clone(),
            }))
            .await
            .unwrap();
        assert!(
            bridge
                .handle(PeerCall::CloudArchiveRead(CloudArchiveReadParams {
                    handle,
                    offset: 0,
                    length: 1
                }))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn role_and_traversal_substitutions_never_create_an_archive_root() {
        let (home, bridge) = fixture();
        let mut b = binding(&[1; 80]);
        b.local_peer_id = b.context.controller_peer_id.clone();
        assert!(bridge.handle(open(b)).await.is_err());
        let mut b = binding(&[1; 80]);
        b.context.operation_id = "../outside".into();
        assert!(bridge.handle(open(b)).await.is_err());
        assert!(!home.path().join("workspace-archives").exists());
        assert!(serde_json::from_value::<PeerCall>(serde_json::json!({"method":"cloud.archive.source_open", "params":{"context":binding(&[1;80]).context,"local_peer_id":"peer","head":{"digest":"digest","bytes":80},"path":"/private/source"}})).is_err());
    }
}
