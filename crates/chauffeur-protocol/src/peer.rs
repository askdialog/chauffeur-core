//! Methods one daemon calls on another: the steps of a transfer, its ciphertext I/O, a backup's
//! key. The daemon serves them on its Unix socket's `/peer` path, to the user's own processes
//! only: never to an agent, the app's TCP endpoint or the phone, which speak
//! [`crate::client::ClientCall`] on `/ws`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::client::{EmptyParams, WorkspaceParams};
use crate::cloud_archive::{
    CloudArchiveBinding, CloudArchiveBlock, CloudArchiveHandle, CloudArchiveReadParams,
    CloudArchiveReceiveParams, CloudArchiveReceiveResult, CloudArchiveWriteParams,
};
use crate::cloud_backup::CloudBackupConfigureParams;
use crate::cloud_transfer::{
    CloudTransferActivateParams, CloudTransferActivated, CloudTransferCaptureParams,
    CloudTransferCaptured, CloudTransferCheckParams, CloudTransferDigest, CloudTransferDiscarded,
    CloudTransferJournal, CloudTransferJournalParams, CloudTransferPeer, CloudTransferReadiness,
    CloudTransferRestoreParams, CloudTransferRestored, CloudTransferSealed, CloudTransferSetup,
    CloudTransferSetupParams, CloudTransferState, TransferSizeEstimate,
};
use crate::sidecar::PackageCryptResult;
use crate::worker::{
    WorkerClaudeConfigChunk, WorkerClaudeConfigInstallParams, WorkerClaudeConfigInstalled,
    WorkerClaudeConfigStatus, WorkerCredentials, WorkerCredentialsInstalled,
};

/// The version of [`PeerCall`], sent in its `hello`. Both ends of a peer connection run the same
/// Chauffeur (a transfer refuses daemons of other versions), so the daemon takes this one only;
/// `protocol.lock` keeps its fingerprint as for the client protocol.
pub const PEER_PROTOCOL_VERSION: u32 = 3;

methods! {
    /// What `/peer` serves.
    pub enum PeerCall / PeerMethod {
        #[serde(rename = "hello")]
        Hello(PeerHelloParams) -> PeerHelloResult,
        /// A proof coordinator's steps of one transfer. Private Unix only; never forwarded to mobile.
        #[serde(rename = "cloud.transfer.peer")]
        CloudTransferPeer(EmptyParams) -> CloudTransferPeer,
        #[serde(rename = "cloud.transfer.source_estimate")]
        CloudTransferSourceEstimate(WorkspaceParams) -> TransferSizeEstimate,
        #[serde(rename = "cloud.transfer.journal")]
        CloudTransferJournal(CloudTransferJournalParams) -> CloudTransferJournal,
        #[serde(rename = "cloud.transfer.source_stop")]
        CloudTransferSourceStop(CloudTransferState) -> CloudTransferDigest,
        #[serde(rename = "cloud.transfer.source_capture")]
        CloudTransferSourceCapture(CloudTransferCaptureParams) -> CloudTransferCaptured,
        #[serde(rename = "cloud.transfer.source_seal")]
        CloudTransferSourceSeal(CloudTransferState) -> CloudTransferSealed,
        #[serde(rename = "cloud.transfer.source_retire")]
        CloudTransferSourceRetire(CloudTransferState) -> EmptyParams,
        #[serde(rename = "cloud.transfer.source_discard")]
        CloudTransferSourceDiscard(CloudTransferState) -> CloudTransferDiscarded,
        #[serde(rename = "cloud.transfer.source_release")]
        CloudTransferSourceRelease(CloudTransferState) -> EmptyParams,
        #[serde(rename = "cloud.transfer.destination_abandon")]
        CloudTransferDestinationAbandon(CloudTransferState) -> EmptyParams,
        #[serde(rename = "cloud.transfer.destination_check")]
        CloudTransferDestinationCheck(CloudTransferCheckParams) -> CloudTransferReadiness,
        #[serde(rename = "cloud.transfer.destination_restore")]
        CloudTransferDestinationRestore(CloudTransferRestoreParams) -> CloudTransferRestored,
        #[serde(rename = "cloud.transfer.destination_activate")]
        CloudTransferDestinationActivate(CloudTransferActivateParams) -> CloudTransferActivated,
        /// On a worker, after an activation that left them waiting: installs the toolchains each
        /// repo declares, then starts the setup scripts.
        #[serde(rename = "cloud.transfer.destination_setup")]
        CloudTransferDestinationSetup(CloudTransferSetupParams) -> CloudTransferSetup,
        /// Hands a worker the key and place of a workspace's backups. Private Unix only.
        #[serde(rename = "cloud.backup.configure")]
        CloudBackupConfigure(CloudBackupConfigureParams) -> EmptyParams,
        /// Hands a worker this Mac's GitHub and AWS SSO access on each send. Private Unix only.
        #[serde(rename = "worker.credentials_install")]
        WorkerCredentialsInstall(WorkerCredentials) -> WorkerCredentialsInstalled,
        /// A Mac's Claude Code configuration, sent in chunks on each send to a worker unless it
        /// installed the same last time. Private Unix only.
        #[serde(rename = "worker.claude_config_status")]
        WorkerClaudeConfigStatus(EmptyParams) -> WorkerClaudeConfigStatus,
        #[serde(rename = "worker.claude_config_write")]
        WorkerClaudeConfigWrite(WorkerClaudeConfigChunk) -> EmptyParams,
        #[serde(rename = "worker.claude_config_install")]
        WorkerClaudeConfigInstall(WorkerClaudeConfigInstallParams) -> WorkerClaudeConfigInstalled,
        /// Ciphertext handles are scoped to this private Unix connection.
        #[serde(rename = "cloud.archive.source_open")]
        CloudArchiveSourceOpen(CloudArchiveBinding) -> CloudArchiveHandle,
        #[serde(rename = "cloud.archive.receive_begin")]
        CloudArchiveReceiveBegin(CloudArchiveReceiveParams) -> CloudArchiveReceiveResult,
        #[serde(rename = "cloud.archive.read")]
        CloudArchiveRead(CloudArchiveReadParams) -> CloudArchiveBlock,
        #[serde(rename = "cloud.archive.write")]
        CloudArchiveWrite(CloudArchiveWriteParams) -> EmptyParams,
        #[serde(rename = "cloud.archive.commit")]
        CloudArchiveCommit(CloudArchiveHandle) -> PackageCryptResult,
        #[serde(rename = "cloud.archive.close")]
        CloudArchiveClose(CloudArchiveHandle) -> EmptyParams,
    }
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct PeerHelloParams {
    pub protocol_version: u32,
    /// Free-form name for logs, e.g. `chauffeurd 0.25.0 transfer`.
    pub client: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct PeerHelloResult {
    pub protocol_version: u32,
    pub daemon_version: String,
}
