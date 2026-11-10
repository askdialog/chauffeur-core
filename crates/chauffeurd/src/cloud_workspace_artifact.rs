//! Daemon-owned ciphertext staging and reception, not a listener or execution authority.
//! Trusted callers authenticate current snapshot-ready state and retain keys separately.
use std::ffi::CString;
use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use anyhow::{Context, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chauffeur_protocol::sidecar::{PackageCryptResult, PackageTransferContext};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cloud_workspace_handoff::State;

pub const BLOCK_BYTES: usize = 256 * 1024;
const MAX_BYTES: u64 = 16 * 1024 * 1024 * 1024 + 36;
const ROOT: &str = "workspace-archives";
const MANIFEST: &str = "binding.json";
const ARCHIVE: &str = "archive.bin";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    version: u8,
    context: PackageTransferContext,
    head: PackageCryptResult,
    local_peer_id: String,
}
impl Binding {
    fn new(
        state: &State,
        peer: &str,
        head: &PackageCryptResult,
        source: bool,
    ) -> anyhow::Result<Self> {
        let context = crate::cloud_workspace_archive::transfer_context(state)?;
        Self::bound(&context, peer, head, source)
    }
    fn bound(
        context: &PackageTransferContext,
        peer: &str,
        head: &PackageCryptResult,
        source: bool,
    ) -> anyhow::Result<Self> {
        use crate::cloud_workspace_handoff::{digest, peer as valid_peer, uuid};
        ensure!(
            uuid(&context.workspace_id)
                && uuid(&context.operation_id)
                && [
                    &context.owner_peer_id,
                    &context.controller_peer_id,
                    &context.source_peer_id,
                    &context.target_peer_id
                ]
                .iter()
                .all(|p| valid_peer(p))
                && context.owner_peer_id != context.controller_peer_id
                && context.source_peer_id != context.target_peer_id
                && context.source_peer_id != context.controller_peer_id
                && context.target_peer_id != context.controller_peer_id
                && context.source_epoch > 0
                && context.source_epoch < 9_007_199_254_740_991
                && digest(&context.snapshot_digest)
                && peer
                    == if source {
                        &context.source_peer_id
                    } else {
                        &context.target_peer_id
                    },
            "Archive endpoint or context differs from the authenticated operation"
        );
        let digest = URL_SAFE_NO_PAD.decode(&head.digest)?;
        ensure!(
            digest.len() == 32
                && URL_SAFE_NO_PAD.encode(digest) == head.digest
                && (40..=MAX_BYTES).contains(&head.bytes),
            "Invalid authenticated archive reference"
        );
        Ok(Self {
            version: 1,
            context: context.clone(),
            head: head.clone(),
            local_peer_id: peer.into(),
        })
    }
}

fn private(meta: &Metadata, directory: bool) -> anyhow::Result<()> {
    // SAFETY: geteuid takes no pointers and has no preconditions.
    let uid = unsafe { libc::geteuid() };
    ensure!(
        meta.uid() == uid
            && meta.permissions().mode() & 0o7777 == if directory { 0o700 } else { 0o600 }
            && if directory {
                meta.is_dir()
            } else {
                meta.is_file() && meta.nlink() == 1
            },
        "Archive storage refuses non-private, linked or unsupported entries"
    );
    Ok(())
}
fn entry(parent: &File, name: &str, flags: i32) -> anyhow::Result<Option<File>> {
    let name = CString::new(name)?;
    // SAFETY: the owned parent FD and NUL-terminated name outlive openat; mode is used only for O_CREAT.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            0o600,
        )
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(error.into());
    }
    // SAFETY: successful openat returns a fresh, owned FD.
    let file = unsafe { File::from_raw_fd(fd) };
    private(&file.metadata()?, flags & libc::O_DIRECTORY != 0)?;
    Ok(Some(file))
}
fn directory(parent: &File, name: &str, create: bool) -> anyhow::Result<(File, bool)> {
    let mut created = false;
    if create {
        let c_name = CString::new(name)?;
        // SAFETY: the owned parent FD and NUL-terminated name outlive mkdirat.
        if unsafe { libc::mkdirat(parent.as_raw_fd(), c_name.as_ptr(), 0o700) } == 0 {
            created = true;
        } else {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(error.into());
            }
        }
    }
    let file = entry(parent, name, libc::O_RDONLY | libc::O_DIRECTORY)?
        .context("Archive directory missing; preserve retained state and reconcile")?;
    if created {
        file.sync_all()?;
        parent.sync_all()?;
    }
    Ok((file, created))
}
fn same_inode(a: &File, b: &File) -> anyhow::Result<bool> {
    let (a, b) = (a.metadata()?, b.metadata()?);
    Ok(a.dev() == b.dev() && a.ino() == b.ino())
}
struct OperationDirectory(File);
impl std::ops::Deref for OperationDirectory {
    type Target = File;
    fn deref(&self) -> &File {
        &self.0
    }
}
impl Drop for OperationDirectory {
    fn drop(&mut self) {
        // Explicit unlock ends this job's ownership even if a concurrently forked child
        // briefly inherited the open description before its CLOEXEC descriptors close.
        // SAFETY: this wrapper owns the live descriptor on which it acquired LOCK_EX.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}
struct Artifact {
    home: File,
    root: File,
    directory: OperationDirectory,
    file: File,
    manifest: File,
    manifest_stamp: Stamp,
    binding: Binding,
}
impl Artifact {
    fn anchored(&self) -> anyhow::Result<()> {
        private(&self.home.metadata()?, true)?;
        for (parent, name, pinned, directory) in [
            (&self.home, ROOT, &self.root, true),
            (
                &self.root,
                self.binding.context.operation_id.as_str(),
                &self.directory,
                true,
            ),
            (&self.directory, ARCHIVE, &self.file, false),
            (&self.directory, MANIFEST, &self.manifest, false),
        ] {
            let current = entry(
                parent,
                name,
                libc::O_RDONLY | if directory { libc::O_DIRECTORY } else { 0 },
            )?
            .context("Archive storage entry disappeared; retain state and reconcile")?;
            ensure!(
                same_inode(&current, pinned)?,
                "Archive storage entry changed; retain both copies and reconcile"
            );
        }
        ensure!(
            stamp(&self.manifest)? == self.manifest_stamp,
            "Archive binding changed; retain state and reconcile"
        );
        Ok(())
    }
    fn verify(&self) -> anyhow::Result<Stamp> {
        self.anchored()?;
        let before = stamp(&self.file)?;
        ensure!(
            before.2 == self.binding.head.bytes,
            "Archive length differs from its authenticated reference"
        );
        let mut hash = Sha256::new();
        let mut offset = 0;
        let mut buffer = vec![0u8; BLOCK_BYTES];
        while offset < self.binding.head.bytes {
            let length = buffer
                .len()
                .min((self.binding.head.bytes - offset) as usize);
            self.file.read_exact_at(&mut buffer[..length], offset)?;
            hash.update(&buffer[..length]);
            offset += length as u64;
        }
        ensure!(
            URL_SAFE_NO_PAD.encode(hash.finalize()) == self.binding.head.digest
                && before == stamp(&self.file)?,
            "Archive content changed or differs from its authenticated reference"
        );
        self.anchored()?;
        Ok(before)
    }
    fn sync(&self) -> anyhow::Result<()> {
        self.file.sync_all()?;
        self.manifest.sync_all()?;
        self.directory.sync_all()?;
        self.root.sync_all()?;
        self.home.sync_all()?;
        self.anchored()
    }
    fn durable(&self) -> anyhow::Result<Stamp> {
        let verified = self.verify()?;
        self.sync()?;
        ensure!(
            verified == stamp(&self.file)?,
            "Archive changed before its durability receipt"
        );
        Ok(verified)
    }
}
type Stamp = (u64, u64, u64, i64, i64, i64, i64);
fn stamp(file: &File) -> anyhow::Result<Stamp> {
    let m = file.metadata()?;
    private(&m, false)?;
    Ok((
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    ))
}

/// A ciphertext durability receipt only; it grants no restoration, ownership or execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    pub context: PackageTransferContext,
    pub head: PackageCryptResult,
}
fn receipt(artifact: &Artifact) -> Stored {
    Stored {
        context: artifact.binding.context.clone(),
        head: artifact.binding.head.clone(),
    }
}

pub enum Receive {
    New(Writer),
    Stored(Stored),
}

/// Holds an exclusive operation lock. Drop releases it and closes descriptors, retaining bytes.
pub struct Writer {
    artifact: Artifact,
    offset: u64,
    poisoned: bool,
}
impl Writer {
    /// Exact sequential offsets and at most one delivery block; any failed write poisons this handle.
    pub fn write(&mut self, offset: u64, bytes: &[u8]) -> anyhow::Result<()> {
        let result = (|| {
            ensure!(
                !self.poisoned
                    && offset == self.offset
                    && !bytes.is_empty()
                    && bytes.len() <= BLOCK_BYTES
                    && bytes.len() as u64 <= self.artifact.binding.head.bytes - self.offset,
                "Invalid archive block; retain partial output and reconcile"
            );
            self.artifact.anchored()?;
            self.artifact.file.write_all_at(bytes, offset)?;
            self.artifact.anchored()?;
            self.offset += bytes.len() as u64;
            Ok(())
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    /// Rehash the actual file, then sync it and every retained parent before returning a receipt.
    pub fn commit(self) -> anyhow::Result<Stored> {
        ensure!(
            !self.poisoned && self.offset == self.artifact.binding.head.bytes,
            "Archive output is incomplete; retain it and reconcile"
        );
        self.artifact.durable()?;
        Ok(receipt(&self.artifact))
    }
}

/// A pinned, verified source descriptor. Calls are bounded; transport also checks the full digest.
pub struct Reader {
    artifact: Artifact,
    stamp: Stamp,
}
impl Reader {
    pub fn read(&self, offset: u64, length: usize) -> anyhow::Result<Vec<u8>> {
        ensure!(
            length > 0
                && length <= BLOCK_BYTES
                && offset <= self.artifact.binding.head.bytes
                && length as u64 <= self.artifact.binding.head.bytes - offset,
            "Invalid archive read range"
        );
        self.artifact.anchored()?;
        ensure!(
            stamp(&self.artifact.file)? == self.stamp,
            "Archive source changed; retain it and reconcile"
        );
        let mut bytes = vec![0u8; length];
        self.artifact.file.read_exact_at(&mut bytes, offset)?;
        ensure!(
            stamp(&self.artifact.file)? == self.stamp,
            "Archive source changed during reading"
        );
        self.artifact.anchored()?;
        Ok(bytes)
    }
}

fn reserve(home: &Path, binding: Binding, allow_new: bool) -> anyhow::Result<Receive> {
    let (artifact, created) = access(home, binding, allow_new)?;
    if !created && !(allow_new && artifact.file.metadata()?.len() < artifact.binding.head.bytes) {
        artifact.durable()?;
        return Ok(Receive::Stored(receipt(&artifact)));
    }
    // Replaying an interrupted upload overwrites its prefix under the same authenticated
    // binding and exclusive lock. No receipt exists until the entire file verifies again.
    Ok(Receive::New(Writer {
        artifact,
        offset: 0,
        poisoned: false,
    }))
}
fn access(home: &Path, binding: Binding, allow_new: bool) -> anyhow::Result<(Artifact, bool)> {
    // Resolve the configured daemon home once, then walk only pinned directory descriptors.
    let home = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(home.canonicalize()?)?;
    private(&home.metadata()?, true)?;
    let (root, _) = directory(&home, ROOT, allow_new)?;
    let (directory, created) = directory(&root, &binding.context.operation_id, allow_new)?;
    // SAFETY: flock operates on a live owned directory FD.
    ensure!(
        unsafe { libc::flock(directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "Archive operation is in use; wait for its existing host job"
    );
    let directory = OperationDirectory(directory);
    let manifest;
    let manifest_stamp;
    if created {
        let mut output = entry(
            &directory,
            MANIFEST,
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
        )?
        .context("Could not reserve archive binding")?;
        output.write_all(&serde_json::to_vec(&binding)?)?;
        output.sync_all()?;
        directory.sync_all()?;
        root.sync_all()?;
        manifest_stamp = stamp(&output)?;
        manifest = output;
    } else {
        manifest = entry(&directory, MANIFEST, libc::O_RDONLY)?
            .context("Archive binding is incomplete; preserve its directory and reconcile")?;
        ensure!(
            manifest.metadata()?.len() <= 8192,
            "Archive binding exceeds its limit"
        );
        manifest_stamp = stamp(&manifest)?;
        let mut bytes = Vec::new();
        (&manifest).take(8193).read_to_end(&mut bytes)?;
        ensure!(
            manifest_stamp == stamp(&manifest)?
                && bytes.len() <= 8192
                && serde_json::from_slice::<Binding>(&bytes)? == binding,
            "Retained archive belongs to another operation binding; preserve it and reconcile"
        );
    }
    let flags = libc::O_RDWR
        | if created {
            libc::O_CREAT | libc::O_EXCL
        } else {
            0
        };
    let file = entry(&directory, ARCHIVE, flags)?
        .context("Retained archive is missing; preserve its binding and reconcile")?;
    let artifact = Artifact {
        home,
        root,
        directory,
        file,
        manifest,
        manifest_stamp,
        binding,
    };
    artifact.anchored()?;
    Ok((artifact, created))
}

/// Where an operation's ciphertext lives, staged on a source or received on a destination.
pub(crate) fn archive_path(home: &Path, operation: &str) -> std::path::PathBuf {
    home.join(ROOT).join(operation).join(ARCHIVE)
}

/// The host authenticates current published state and the destination identity before calling.
/// A complete retry is reverified and synced. With `allow_new`, partial bytes can be replayed
/// from offset zero under their original binding; missing previous output is never recreated.
pub fn receive(
    home: &Path,
    published: &State,
    destination_peer: &str,
    head: &PackageCryptResult,
    allow_new: bool,
) -> anyhow::Result<Receive> {
    reserve(
        home,
        Binding::new(published, destination_peer, head, false)?,
        allow_new,
    )
}

/// Copy an endpoint-local sealed file into a new operation-owned artifact. The source is never
/// renamed or deleted. A failed or uncertain copy remains reserved for explicit reconciliation.
pub fn stage_source(
    home: &Path,
    source: &Path,
    published: &State,
    source_peer: &str,
    head: &PackageCryptResult,
) -> anyhow::Result<Stored> {
    let binding = Binding::new(published, source_peer, head, true)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(source)?;
    let before = stamp(&file)?;
    ensure!(
        before.2 == head.bytes,
        "Sealed source length differs from its authenticated reference"
    );
    match reserve(home, binding, true)? {
        Receive::Stored(stored) => Ok(stored),
        Receive::New(mut writer) => {
            let mut offset = 0;
            let mut buffer = vec![0u8; BLOCK_BYTES];
            while offset < head.bytes {
                let length = buffer.len().min((head.bytes - offset) as usize);
                file.read_exact_at(&mut buffer[..length], offset)?;
                writer.write(offset, &buffer[..length])?;
                offset += length as u64;
            }
            ensure!(
                stamp(&file)? == before,
                "Sealed source changed during staging; retain both copies"
            );
            writer.commit()
        }
    }
}

/// The head of the ciphertext this source staged for the published transfer, if it did: what a
/// transfer resumed after its seal relays again. Its binding is read through the same pinned,
/// no-follow descriptors as the archive's, and the archive must open whole: staging cut short
/// (the binding is written first) is refused, never relayed.
pub(crate) fn staged_head(
    home: &Path,
    published: &State,
    source_peer: &str,
) -> anyhow::Result<Option<PackageCryptResult>> {
    let context = crate::cloud_workspace_archive::transfer_context(published)?;
    let home_dir = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(home.canonicalize()?)?;
    let directory = libc::O_RDONLY | libc::O_DIRECTORY;
    let Some(root) = entry(&home_dir, ROOT, directory)? else {
        return Ok(None);
    };
    let Some(operation) = entry(&root, &context.operation_id, directory)? else {
        return Ok(None);
    };
    let Some(manifest) = entry(&operation, MANIFEST, libc::O_RDONLY)? else {
        return Ok(None);
    };
    ensure!(
        manifest.metadata()?.len() <= 8192,
        "Archive binding exceeds its limit"
    );
    let mut bytes = Vec::new();
    (&manifest).take(8193).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 8192, "Archive binding exceeds its limit");
    let binding: Binding = serde_json::from_slice(&bytes)?;
    ensure!(
        binding == Binding::bound(&context, source_peer, &binding.head, true)?,
        "The staged archive belongs to another transfer; reconcile it"
    );
    source_reader(home, published, source_peer, &binding.head)
        .context("The staged archive is incomplete; cancel the transfer and start it again")?;
    Ok(Some(binding.head))
}

/// Open a staged source only, with the exact original binding and no implicit initialization.
pub fn source_reader(
    home: &Path,
    published: &State,
    source_peer: &str,
    head: &PackageCryptResult,
) -> anyhow::Result<Reader> {
    let (artifact, _) = access(
        home,
        Binding::new(published, source_peer, head, true)?,
        false,
    )?;
    let stamp = artifact.durable()?;
    Ok(Reader { artifact, stamp })
}

pub(crate) fn validate_bound(
    context: &PackageTransferContext,
    peer: &str,
    head: &PackageCryptResult,
    source: bool,
) -> anyhow::Result<()> {
    Binding::bound(context, peer, head, source)?;
    Ok(())
}
/// Private host bridge only: context is routing metadata, never source-stop or execution permission.
pub(crate) fn receive_bound(
    home: &Path,
    context: &PackageTransferContext,
    peer: &str,
    head: &PackageCryptResult,
    allow_new: bool,
) -> anyhow::Result<Receive> {
    reserve(home, Binding::bound(context, peer, head, false)?, allow_new)
}
pub(crate) fn source_reader_bound(
    home: &Path,
    context: &PackageTransferContext,
    peer: &str,
    head: &PackageCryptResult,
) -> anyhow::Result<Reader> {
    let (artifact, _) = access(home, Binding::bound(context, peer, head, true)?, false)?;
    let stamp = artifact.durable()?;
    Ok(Reader { artifact, stamp })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{DirBuilderExt, symlink};
    use std::path::PathBuf;

    use crate::cloud_workspace_handoff::{Binding as WorkspaceBinding, Phase, Transfer};

    struct Fixture {
        _temp: tempfile::TempDir,
        source_home: PathBuf,
        target_home: PathBuf,
        sealed: PathBuf,
        state: State,
        head: PackageCryptResult,
        bytes: Vec<u8>,
    }
    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let source_home = temp.path().join("source");
            let target_home = temp.path().join("target");
            for dir in [&source_home, &target_home] {
                fs::DirBuilder::new().mode(0o700).create(dir).unwrap();
            }
            let peer = |c: char| format!("12D3KooW{}", c.to_string().repeat(44));
            let state = State {
                version: 1,
                binding: WorkspaceBinding {
                    workspace_id: uuid::Uuid::now_v7().to_string(),
                    owner_peer_id: peer('1'),
                    controller_peer_id: peer('2'),
                },
                revision: 5,
                epoch: 1,
                execution_peer_id: peer('1'),
                transfer: Some(Transfer {
                    operation_id: uuid::Uuid::now_v7().to_string(),
                    source_peer_id: peer('1'),
                    target_peer_id: peer('3'),
                    source_epoch: 1,
                    phase: Phase::SnapshotReady,
                    source_stop_digest: Some(URL_SAFE_NO_PAD.encode([1; 32])),
                    snapshot_digest: Some(URL_SAFE_NO_PAD.encode([2; 32])),
                    destination_receipt_digest: None,
                }),
            };
            let bytes: Vec<u8> = (0..BLOCK_BYTES * 2 + 73).map(|i| (i % 251) as u8).collect();
            let head = PackageCryptResult {
                digest: URL_SAFE_NO_PAD.encode(Sha256::digest(&bytes)),
                bytes: bytes.len() as u64,
            };
            let sealed = temp.path().join("sealed.bin");
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&sealed)
                .unwrap();
            file.write_all(&bytes).unwrap();
            Self {
                _temp: temp,
                source_home,
                target_home,
                sealed,
                state,
                head,
                bytes,
            }
        }
        fn source(&self) -> &str {
            &self.state.transfer.as_ref().unwrap().source_peer_id
        }
        fn target(&self) -> &str {
            &self.state.transfer.as_ref().unwrap().target_peer_id
        }
        fn operation(&self, home: &Path) -> PathBuf {
            home.join(ROOT)
                .join(&self.state.transfer.as_ref().unwrap().operation_id)
        }
        fn begin(&self, allow_new: bool) -> anyhow::Result<Receive> {
            receive(
                &self.target_home,
                &self.state,
                self.target(),
                &self.head,
                allow_new,
            )
        }
        fn writer(&self) -> Writer {
            match self.begin(true).unwrap() {
                Receive::New(writer) => writer,
                Receive::Stored(_) => panic!("fixture must be new"),
            }
        }
        fn complete(&self) {
            let mut writer = self.writer();
            for (i, bytes) in self.bytes.chunks(BLOCK_BYTES).enumerate() {
                writer.write((i * BLOCK_BYTES) as u64, bytes).unwrap();
            }
            assert_eq!(writer.commit().unwrap().head, self.head);
        }
    }

    #[test]
    fn staged_source_streams_into_private_durable_destination_and_reopens_without_overwrite() {
        let f = Fixture::new();
        stage_source(&f.source_home, &f.sealed, &f.state, f.source(), &f.head).unwrap();
        let reader = source_reader(&f.source_home, &f.state, f.source(), &f.head).unwrap();
        let mut writer = f.writer();
        for offset in (0..f.bytes.len()).step_by(BLOCK_BYTES) {
            let bytes = reader
                .read(offset as u64, BLOCK_BYTES.min(f.bytes.len() - offset))
                .unwrap();
            writer.write(offset as u64, &bytes).unwrap();
        }
        let stored = writer.commit().unwrap();
        drop(reader);
        assert_eq!(
            stored.context,
            crate::cloud_workspace_archive::transfer_context(&f.state).unwrap()
        );
        let output = f.operation(&f.target_home).join(ARCHIVE);
        let before = fs::metadata(&output).unwrap().ino();
        for allow in [false, true] {
            assert!(matches!(f.begin(allow).unwrap(), Receive::Stored(s) if s == stored));
        }
        assert_eq!(fs::metadata(output).unwrap().ino(), before);
        assert_eq!(fs::read(&f.sealed).unwrap(), f.bytes);
        for home in [&f.source_home, &f.target_home] {
            for dir in [home.join(ROOT), f.operation(home)] {
                assert_eq!(fs::metadata(dir).unwrap().mode() & 0o777, 0o700);
            }
            for name in [MANIFEST, ARCHIVE] {
                assert_eq!(
                    fs::metadata(f.operation(home).join(name)).unwrap().mode() & 0o777,
                    0o600
                );
            }
        }
    }

    #[test]
    fn an_uncertain_complete_file_is_reverified_and_partial_files_are_retained() {
        let f = Fixture::new();
        let mut writer = f.writer();
        for (i, bytes) in f.bytes.chunks(BLOCK_BYTES).enumerate() {
            writer.write((i * BLOCK_BYTES) as u64, bytes).unwrap();
        }
        drop(writer);
        assert!(matches!(f.begin(false).unwrap(), Receive::Stored(_)));
        let g = Fixture::new();
        let mut writer = g.writer();
        writer.write(0, &g.bytes[..52]).unwrap();
        drop(writer);
        assert!(g.begin(false).is_err());
        assert_eq!(
            fs::read(g.operation(&g.target_home).join(ARCHIVE)).unwrap(),
            &g.bytes[..52]
        );
        let mut writer = g.writer();
        for (i, bytes) in g.bytes.chunks(BLOCK_BYTES).enumerate() {
            writer.write((i * BLOCK_BYTES) as u64, bytes).unwrap();
        }
        writer.commit().unwrap();
        assert!(matches!(g.begin(false).unwrap(), Receive::Stored(_)));
    }

    #[test]
    fn missing_trusted_output_is_never_recreated_and_incomplete_bindings_are_preserved() {
        let f = Fixture::new();
        assert!(f.begin(false).is_err());
        assert!(!f.target_home.join(ROOT).exists());
        f.complete();
        let path = f.operation(&f.target_home);
        fs::remove_file(path.join(ARCHIVE)).unwrap();
        for allow in [false, true] {
            assert!(f.begin(allow).is_err());
            assert!(!path.join(ARCHIVE).exists());
        }
        let g = Fixture::new();
        drop(g.writer());
        let path = g.operation(&g.target_home);
        fs::remove_file(path.join(MANIFEST)).unwrap();
        assert!(g.begin(true).is_err());
        assert!(!path.join(MANIFEST).exists());
        assert!(path.join(ARCHIVE).exists());
    }

    #[test]
    fn operation_locks_prevent_overlapping_writers_and_readers_without_deleting_outputs() {
        let f = Fixture::new();
        let writer = f.writer();
        assert!(f.begin(true).is_err());
        drop(writer);
        assert!(matches!(f.begin(true).unwrap(), Receive::New(_)));
        stage_source(&f.source_home, &f.sealed, &f.state, f.source(), &f.head).unwrap();
        let reader = source_reader(&f.source_home, &f.state, f.source(), &f.head).unwrap();
        assert!(source_reader(&f.source_home, &f.state, f.source(), &f.head).is_err());
        assert!(stage_source(&f.source_home, &f.sealed, &f.state, f.source(), &f.head).is_err());
        let inherited = reader.artifact.directory.try_clone().unwrap();
        drop(reader);
        assert!(source_reader(&f.source_home, &f.state, f.source(), &f.head).is_ok());
        drop(inherited);
    }

    #[test]
    fn commit_hashes_actual_file_bytes_and_poisoned_handles_never_return_receipts() {
        let f = Fixture::new();
        let mut writer = f.writer();
        for (i, bytes) in f.bytes.chunks(BLOCK_BYTES).enumerate() {
            writer.write((i * BLOCK_BYTES) as u64, bytes).unwrap();
        }
        let file = OpenOptions::new()
            .write(true)
            .open(f.operation(&f.target_home).join(ARCHIVE))
            .unwrap();
        file.write_all_at(b"changed", 12).unwrap();
        assert!(writer.commit().is_err());
        assert!(f.begin(false).is_err());
        let g = Fixture::new();
        let mut writer = g.writer();
        assert!(writer.write(1, &[3]).is_err());
        assert!(writer.write(0, &g.bytes[..12]).is_err());
        assert!(writer.commit().is_err());
        let h = Fixture::new();
        let mut writer = h.writer();
        assert!(writer.write(0, &vec![0; BLOCK_BYTES + 1]).is_err());
        assert!(writer.commit().is_err());
    }

    #[test]
    fn bindings_endpoints_limits_and_noncanonical_operation_paths_refuse_before_creation() {
        let f = Fixture::new();
        for peer in [f.source(), f.state.binding.controller_peer_id.as_str()] {
            assert!(receive(&f.target_home, &f.state, peer, &f.head, true).is_err());
        }
        for bytes in [0, 39, MAX_BYTES + 1] {
            let mut head = f.head.clone();
            head.bytes = bytes;
            assert!(receive(&f.target_home, &f.state, f.target(), &head, true).is_err());
        }
        for operation in ["..", "../escape", "/absolute"] {
            let mut state = f.state.clone();
            state.transfer.as_mut().unwrap().operation_id = operation.into();
            assert!(receive(&f.target_home, &state, f.target(), &f.head, true).is_err());
        }
        assert!(!f.target_home.join(ROOT).exists());
        f.complete();
        let mut head = f.head.clone();
        head.digest = URL_SAFE_NO_PAD.encode([4; 32]);
        assert!(receive(&f.target_home, &f.state, f.target(), &head, true).is_err());
        let mut state = f.state.clone();
        state.transfer.as_mut().unwrap().snapshot_digest = Some(URL_SAFE_NO_PAD.encode([3; 32]));
        assert!(receive(&f.target_home, &state, f.target(), &f.head, true).is_err());
        assert!(matches!(f.begin(false).unwrap(), Receive::Stored(_)));
    }

    #[test]
    fn root_operation_manifest_and_archive_symlinks_never_follow_outside_storage() {
        for name in [ROOT, "operation", MANIFEST, ARCHIVE] {
            let f = Fixture::new();
            f.complete();
            let victim = if name == ROOT {
                f.target_home.join(ROOT)
            } else if name == "operation" {
                f.operation(&f.target_home)
            } else {
                f.operation(&f.target_home).join(name)
            };
            let retained = victim.with_extension("retained");
            fs::rename(&victim, &retained).unwrap();
            symlink(&retained, &victim).unwrap();
            assert!(f.begin(true).is_err());
            assert!(
                fs::symlink_metadata(victim)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert!(retained.exists());
        }
    }

    #[test]
    fn hardlinks_shared_permissions_and_special_files_are_refused_without_blocking() {
        let f = Fixture::new();
        f.complete();
        let archive = f.operation(&f.target_home).join(ARCHIVE);
        let alias = f.target_home.join("alias");
        fs::hard_link(&archive, &alias).unwrap();
        assert!(f.begin(false).is_err());
        assert_eq!(fs::read(alias).unwrap(), f.bytes);
        let g = Fixture::new();
        g.complete();
        fs::set_permissions(
            g.operation(&g.target_home).join(ARCHIVE),
            fs::Permissions::from_mode(0o640),
        )
        .unwrap();
        assert!(g.begin(true).is_err());
        let h = Fixture::new();
        h.complete();
        let archive = h.operation(&h.target_home).join(ARCHIVE);
        fs::remove_file(&archive).unwrap();
        let name = CString::new(archive.to_str().unwrap()).unwrap();
        // SAFETY: the temporary test path is NUL-terminated and remains valid through mkfifo.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(h.begin(false).is_err());
        assert!(fs::symlink_metadata(archive).is_ok());
    }

    #[test]
    fn renamed_or_replaced_entries_cannot_receive_a_commit_receipt() {
        for replace_manifest in [false, true] {
            let f = Fixture::new();
            let mut writer = f.writer();
            for (i, bytes) in f.bytes.chunks(BLOCK_BYTES).enumerate() {
                writer.write((i * BLOCK_BYTES) as u64, bytes).unwrap();
            }
            if replace_manifest {
                fs::write(f.operation(&f.target_home).join(MANIFEST), b"changed").unwrap();
            } else {
                let dir = f.operation(&f.target_home);
                fs::rename(&dir, dir.with_extension("moved")).unwrap();
                fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
            }
            assert!(writer.commit().is_err());
        }
    }

    #[test]
    fn source_reads_detect_mutation_and_refuse_out_of_range_or_unbounded_requests() {
        let f = Fixture::new();
        stage_source(&f.source_home, &f.sealed, &f.state, f.source(), &f.head).unwrap();
        let reader = source_reader(&f.source_home, &f.state, f.source(), &f.head).unwrap();
        for (offset, size) in [
            (0, 0),
            (0, BLOCK_BYTES + 1),
            (f.head.bytes, 1),
            (u64::MAX, 1),
        ] {
            assert!(reader.read(offset, size).is_err());
        }
        OpenOptions::new()
            .write(true)
            .open(f.operation(&f.source_home).join(ARCHIVE))
            .unwrap()
            .write_all_at(b"changed", 0)
            .unwrap();
        assert!(reader.read(0, 40).is_err());
        drop(reader);
        assert!(source_reader(&f.source_home, &f.state, f.source(), &f.head).is_err());
    }

    #[test]
    fn staging_refuses_wrong_content_and_keeps_original_source_and_partial_output() {
        let f = Fixture::new();
        let mut head = f.head.clone();
        head.digest = URL_SAFE_NO_PAD.encode([8; 32]);
        assert!(stage_source(&f.source_home, &f.sealed, &f.state, f.source(), &head).is_err());
        assert_eq!(fs::read(&f.sealed).unwrap(), f.bytes);
        assert_eq!(
            fs::read(f.operation(&f.source_home).join(ARCHIVE)).unwrap(),
            f.bytes
        );
    }
}
