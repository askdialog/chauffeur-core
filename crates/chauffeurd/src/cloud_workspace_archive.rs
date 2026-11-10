//! Endpoint-only encrypted artifact preparation and reception. No relay, key distribution,
//! journal transition or ownership activation is enabled by possessing an archive or its key.
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chauffeur_protocol::sidecar::{
    PackageCryptParams, PackageCryptResult, PackageTransferContext, SidecarCall,
};
use zeroize::Zeroizing;

use crate::cloud_workspace_handoff::{Phase, State};
use crate::cloud_workspace_package::{self as package, Package};
use crate::cloud_workspace_snapshot::{copy_hash, open_file};
use crate::{Daemon, Error, Result};

pub(crate) const MAX_CIPHERTEXT: u64 = 16 * 1024 * 1024 * 1024 + 36;

/// Kept at endpoints, outside artifacts. The coordinator must durably retain and deliver this
/// through an authenticated endpoint-only channel before allowing recoverable copies to disappear.
pub struct ArchiveKey(Zeroizing<[u8; 32]>);
impl ArchiveKey {
    pub fn generate() -> Self {
        Self(Zeroizing::new(rand::random()))
    }
    /// Only authenticated endpoint recovery/key delivery may supply these bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }
    /// Only protected endpoint persistence or authenticated key delivery may receive this value.
    pub fn expose_bytes(&self) -> &[u8; 32] {
        &self.0
    }
    fn encoded(&self) -> Zeroizing<String> {
        Zeroizing::new(URL_SAFE_NO_PAD.encode(self.0.as_slice()))
    }
}

impl fmt::Debug for ArchiveKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ArchiveKey([REDACTED])")
    }
}

pub(crate) fn transfer_context(published: &State) -> anyhow::Result<PackageTransferContext> {
    published.validate()?;
    let transfer = published
        .transfer
        .as_ref()
        .context("No workspace transfer operation")?;
    ensure!(
        transfer.phase == Phase::SnapshotReady,
        "The workspace snapshot is not published"
    );
    Ok(PackageTransferContext {
        workspace_id: published.binding.workspace_id.clone(),
        owner_peer_id: published.binding.owner_peer_id.clone(),
        controller_peer_id: published.binding.controller_peer_id.clone(),
        operation_id: transfer.operation_id.clone(),
        source_peer_id: transfer.source_peer_id.clone(),
        target_peer_id: transfer.target_peer_id.clone(),
        source_epoch: transfer.source_epoch,
        snapshot_digest: transfer
            .snapshot_digest
            .clone()
            .context("Missing snapshot digest")?,
        backup: false,
    })
}

fn output_path(path: &Path) -> anyhow::Result<PathBuf> {
    // Resolve the parent only: an existing final entry, including a dangling link, must refuse.
    let name = path.file_name().context("Missing archive output name")?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let path = parent.canonicalize()?.join(name);
    ensure!(
        std::fs::symlink_metadata(&path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
        "Archive output already exists; retain it and choose a new output"
    );
    Ok(path)
}
fn text(path: &Path) -> anyhow::Result<String> {
    Ok(path.to_str().context("Non-UTF8 archive path")?.to_owned())
}
fn check_ciphertext(path: &Path, expected: &PackageCryptResult) -> anyhow::Result<()> {
    ensure!(
        expected.bytes <= MAX_CIPHERTEXT && expected.bytes >= 40,
        "Unsupported archive size"
    );
    let parent = path.parent().context("Missing archive parent")?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("Invalid archive name")?;
    let source = open_file(parent, name)?.context("Encrypted archive missing")?;
    let (hex, bytes) = copy_hash(source, None, MAX_CIPHERTEXT)?;
    ensure!(
        bytes == expected.bytes && package::journal_digest(&hex)? == expected.digest,
        "The encrypted archive differs from its authenticated reference"
    );
    Ok(())
}

/// Caller authenticates current snapshot-ready state. The output is new and contains ciphertext
/// only. Failure retains private partial output and the original package; nothing is deleted.
pub async fn seal(
    daemon: &Arc<Daemon>,
    folder: &Path,
    output: &Path,
    published: &State,
    key: &ArchiveKey,
) -> Result<PackageCryptResult> {
    let (daemon, folder, output, state, key) = (
        daemon.clone(),
        folder.to_owned(),
        output.to_owned(),
        published.clone(),
        key.encoded(),
    );
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        runtime.block_on(async move {
            let verified = package::verify_published(&folder, &state)?;
            let folder = folder.canonicalize()?;
            let output = output_path(&output)?;
            ensure!(
                !output.starts_with(&folder),
                "Keep encrypted archives outside the source package"
            );
            let context = transfer_context(&state)?;
            let result = daemon
                .chats
                .sidecar
                .call(SidecarCall::PackageSeal(PackageCryptParams {
                    input: text(&folder)?,
                    output: text(&output)?,
                    key: key.to_string(),
                    context,
                    ciphertext_digest: None,
                    ciphertext_bytes: None,
                }))
                .await?;
            let result: PackageCryptResult = serde_json::from_value(result)?;
            check_ciphertext(&output, &result)?;
            ensure!(
                package::verify_published(&folder, &state)?.digest == verified.digest,
                "The source package changed during encryption"
            );
            Ok::<_, anyhow::Error>(result)
        })
    })
    .await
    .map_err(|e| Error::Internal(e.into()))?
    .map_err(Error::Internal)
}

/// Seals a backup's package, captured while the workspace ran here, under `context` (a backup's,
/// whose snapshot digest is the package's). The output is new and holds ciphertext only.
pub async fn seal_backup(
    daemon: &Arc<Daemon>,
    folder: &Path,
    output: &Path,
    context: &PackageTransferContext,
    key: &ArchiveKey,
) -> Result<PackageCryptResult> {
    let (daemon, folder, output, context, key) = (
        daemon.clone(),
        folder.to_owned(),
        output.to_owned(),
        context.clone(),
        key.encoded(),
    );
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        runtime.block_on(async move {
            ensure!(context.backup, "Not a backup's context");
            let verified = package::verify(&folder)?;
            let folder = folder.canonicalize()?;
            let output = output_path(&output)?;
            ensure!(
                !output.starts_with(&folder),
                "Keep encrypted archives outside the source package"
            );
            let result = daemon
                .chats
                .sidecar
                .call(SidecarCall::PackageSeal(PackageCryptParams {
                    input: text(&folder)?,
                    output: text(&output)?,
                    key: key.to_string(),
                    context,
                    ciphertext_digest: None,
                    ciphertext_bytes: None,
                }))
                .await?;
            let result: PackageCryptResult = serde_json::from_value(result)?;
            check_ciphertext(&output, &result)?;
            ensure!(
                package::verify(&folder)?.digest == verified.digest,
                "The backup's package changed during encryption"
            );
            Ok::<_, anyhow::Error>(result)
        })
    })
    .await
    .map_err(|e| Error::Internal(e.into()))?
    .map_err(Error::Internal)
}

/// Opens a backup into `output`, a new folder, with the workspace's key and the context
/// `backup_context` makes from the committed transfer and the backup's id: no reference from the
/// worker is needed, the ciphertext authenticates itself. Returns the verified package.
pub async fn open_backup(
    daemon: &Arc<Daemon>,
    archive: &Path,
    output: &Path,
    context: &PackageTransferContext,
    key: &ArchiveKey,
) -> Result<Package> {
    let (daemon, archive, output, context, key) = (
        daemon.clone(),
        archive.to_owned(),
        output.to_owned(),
        context.clone(),
        key.encoded(),
    );
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        runtime.block_on(async move {
            ensure!(context.backup, "Not a backup's context");
            let parent = archive
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let name = archive
                .file_name()
                .and_then(|n| n.to_str())
                .context("Invalid backup name")?;
            let source = open_file(&parent.canonicalize()?, name)?.context("Backup missing")?;
            let (hex, bytes) = copy_hash(source, None, MAX_CIPHERTEXT)?;
            let head = PackageCryptResult {
                digest: package::journal_digest(&hex)?,
                bytes,
            };
            let archive = parent.canonicalize()?.join(name);
            check_ciphertext(&archive, &head)?;
            let output = output_path(&output)?;
            let result = daemon
                .chats
                .sidecar
                .call(SidecarCall::PackageOpen(PackageCryptParams {
                    input: text(&archive)?,
                    output: text(&output)?,
                    key: key.to_string(),
                    context,
                    ciphertext_digest: Some(head.digest.clone()),
                    ciphertext_bytes: Some(head.bytes),
                }))
                .await?;
            ensure!(
                serde_json::from_value::<PackageCryptResult>(result)? == head,
                "Unexpected backup reception receipt"
            );
            package::verify(&output)
        })
    })
    .await
    .map_err(|e| Error::Internal(e.into()))?
    .map_err(Error::Internal)
}

/// The authenticated coordinator supplies both the published state and exact ciphertext head.
/// No workspace metadata or execution authority is installed. A received package must still pass
/// restoration under quarantine; failed outputs remain private and are never retried in place.
pub async fn open(
    daemon: &Arc<Daemon>,
    archive: &Path,
    output: &Path,
    published: &State,
    key: &ArchiveKey,
    expected: &PackageCryptResult,
) -> Result<Package> {
    let (daemon, archive, output, state, key, expected) = (
        daemon.clone(),
        archive.to_owned(),
        output.to_owned(),
        published.clone(),
        key.encoded(),
        expected.clone(),
    );
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        runtime.block_on(async move {
            let context = transfer_context(&state)?;
            let parent = archive
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let archive = parent
                .canonicalize()?
                .join(archive.file_name().context("Missing archive name")?);
            check_ciphertext(&archive, &expected)?;
            let output = output_path(&output)?;
            let result = daemon
                .chats
                .sidecar
                .call(SidecarCall::PackageOpen(PackageCryptParams {
                    input: text(&archive)?,
                    output: text(&output)?,
                    key: key.to_string(),
                    context,
                    ciphertext_digest: Some(expected.digest.clone()),
                    ciphertext_bytes: Some(expected.bytes),
                }))
                .await?;
            ensure!(
                serde_json::from_value::<PackageCryptResult>(result)? == expected,
                "Unexpected archive reception receipt"
            );
            let verified = package::verify_published(&output, &state)?;
            Ok::<_, anyhow::Error>(verified)
        })
    })
    .await
    .map_err(|e| Error::Internal(e.into()))?
    .map_err(Error::Internal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_keys_and_private_requests_never_include_the_key_in_debug_output() {
        let key = ArchiveKey::generate();
        let encoded = key.encoded();
        let restored = ArchiveKey::from_bytes(*key.expose_bytes());
        assert_eq!(restored.expose_bytes(), key.expose_bytes());
        assert!(!format!("{key:?}").contains(encoded.as_str()));
        let request = SidecarCall::PackageSeal(PackageCryptParams {
            input: "/private/source".into(),
            output: "/private/archive".into(),
            key: encoded.to_string(),
            context: PackageTransferContext {
                workspace_id: "workspace".into(),
                owner_peer_id: "owner".into(),
                controller_peer_id: "controller".into(),
                operation_id: "operation".into(),
                source_peer_id: "source".into(),
                target_peer_id: "target".into(),
                source_epoch: 1,
                snapshot_digest: "digest".into(),
                backup: false,
            },
            ciphertext_digest: None,
            ciphertext_bytes: None,
        });
        assert!(!format!("{request:?}").contains(encoded.as_str()));
        assert!(format!("{request:?}").contains("REDACTED"));
    }
}
