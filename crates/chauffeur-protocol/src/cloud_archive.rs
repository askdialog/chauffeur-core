//! Ciphertext I/O bindings for private Unix RPC.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::sidecar::{PackageCryptResult, PackageTransferContext};

/// Trusted local host supplies authenticated routing metadata; this is not source-stop evidence.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudArchiveBinding {
    pub context: PackageTransferContext,
    pub local_peer_id: String,
    pub head: PackageCryptResult,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudArchiveReceiveParams {
    pub binding: CloudArchiveBinding,
    pub allow_new: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudArchiveHandle {
    pub handle: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[ts(export)]
pub enum CloudArchiveReceiveResult {
    New { handle: String },
    Stored { head: PackageCryptResult },
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudArchiveReadParams {
    pub handle: String,
    #[ts(type = "number")]
    pub offset: u64,
    pub length: u32,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudArchiveWriteParams {
    pub handle: String,
    #[ts(type = "number")]
    pub offset: u64,
    pub bytes: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudArchiveBlock {
    pub bytes: String,
}
