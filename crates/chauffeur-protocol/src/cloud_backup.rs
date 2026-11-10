//! Encrypted backups of a workspace while it runs on a GCP worker (docs/cloud-workspace-backups.md):
//! what a client sees of them, and the private call that hands the worker its key.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum BackupHealth {
    /// Set up less than fifteen minutes ago; no backup has succeeded or failed yet.
    Pending,
    /// The last backup succeeded less than fifteen minutes ago.
    Current,
    /// The last success, or the setup when none came, is older than fifteen minutes: the worker
    /// may be slow or unreachable.
    Stale,
    /// The last attempt failed (`error`); the backups kept before it are still there.
    Failed,
}

/// One successful backup kept in storage, with what opening it needs besides its key.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct BackupRef {
    pub id: String,
    /// `gs://<bucket>/<prefix>/<workspace>/<id>.bin`.
    pub object: String,
    pub taken_at: String,
    /// Taken while an agent turn ran: each part is coherent, the whole is as after a power cut.
    pub live: bool,
    /// The ciphertext's base64url SHA-256 and length.
    pub digest: String,
    #[ts(type = "number")]
    pub bytes: u64,
    /// The package manifest's base64url SHA-256, which the ciphertext authenticates.
    pub manifest_digest: String,
    /// The worker that took it, and the epoch of the transfer it runs the workspace under.
    pub worker_peer_id: String,
    #[ts(type = "number")]
    pub epoch: u64,
}

/// Where a workspace's backups stand, as the machine it runs on last said.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct BackupStatus {
    pub health: BackupHealth,
    /// The newest backup kept, which is the state it holds; unchanged since, it is still current
    /// at `checked_at`.
    pub last_success_at: Option<String>,
    pub checked_at: Option<String>,
    pub last_attempt_at: Option<String>,
    /// When the backups were set up on the machine it runs on: a first backup is awaited from
    /// then.
    #[serde(default)]
    pub configured_at: Option<String>,
    /// Why the last attempt failed; none once one succeeds again.
    pub error: Option<String>,
    /// The successful backups kept, newest first (three at most).
    pub backups: Vec<BackupRef>,
    /// Files of the workspace folder in no repo and outside its notes, which no backup carries
    /// (the first ten).
    #[serde(default)]
    pub left_out: Vec<String>,
    /// Chats whose conversation the newest backup could not carry, with why (a transcript caught
    /// with no complete record yet): the next backup tries again.
    #[serde(default)]
    pub incomplete: Vec<String>,
}

/// `cloud.backup.configure`: from the Mac, once the workspace is active on a worker. The key is
/// the Mac's, one per workspace; storage sees ciphertext only. Private Unix socket only.
#[derive(Serialize, Deserialize, TS, Clone)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudBackupConfigureParams {
    pub workspace: String,
    /// 32 bytes, base64url.
    pub key: String,
    pub bucket: String,
    /// The worker's own prefix in the bucket, which its service account is limited to.
    pub prefix: String,
}

impl std::fmt::Debug for CloudBackupConfigureParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudBackupConfigureParams")
            .field("workspace", &self.workspace)
            .field("bucket", &self.bucket)
            .field("prefix", &self.prefix)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

/// A backup still present in storage, including one the Mac never heard of while asleep.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct RecoveryBackup {
    pub id: String,
    pub taken_at: String,
}

/// What recovering here will stop: the old worker and every workspace depending on it.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct BackupRecoveryPlan {
    pub workspace: String,
    pub worker: String,
    #[ts(type = "number")]
    pub epoch: u64,
    pub affected_workspaces: Vec<String>,
    pub backups: Vec<RecoveryBackup>,
}

/// Explicit confirmation of a plan; a changed worker, epoch or affected set requires a new one.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct BackupRecoverParams {
    pub workspace: String,
    pub worker: String,
    #[ts(type = "number")]
    pub epoch: u64,
    pub backup: String,
    pub affected_workspaces: Vec<String>,
    #[serde(default)]
    pub keep_local_copy: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct BackupRecovered {
    pub workspace_path: String,
    /// Still billed: retained for recovering work newer than the selected backup by hand.
    pub retained_disk: String,
    pub kept_copy: Option<String>,
}
