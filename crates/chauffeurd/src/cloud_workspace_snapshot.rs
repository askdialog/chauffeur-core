//! Private daemon-side Git snapshot primitive. Not an RPC or execution permission.
//! Callers must quiesce/fence source writers; the consistency check is only a tripwire.
//! Payloads are plaintext in a private local directory: encrypt before any transport.
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Component, Path};

use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::git::Git;

const MAX_FILES: usize = 100_000;
const MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_MANIFEST: u64 = 32 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitSnapshot {
    version: u8,
    head: String,
    pub(crate) branch: Option<String>,
    bundle_digest: String,
    index_digest: String,
    status_digest: String,
    files: Vec<SnapshotFile>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotFile {
    path: String,
    /// None means tracked but absent in the worktree.
    digest: Option<String>,
    size: u64,
    executable: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    symlink: bool,
}

pub(crate) fn valid_path(path: &str) -> anyhow::Result<()> {
    ensure!(
        !path.is_empty() && path.len() <= 8192 && !path.contains(['\\', ':', '\0']),
        "Non-portable snapshot path"
    );
    ensure!(
        Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(s) if !s.to_string_lossy().eq_ignore_ascii_case(".git"))),
        "Unsafe snapshot path"
    );
    ensure!(
        !path
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == ".."),
        "Non-canonical snapshot path"
    );
    Ok(())
}

/// Walk with openat/O_NOFOLLOW: neither a file nor an ancestor symlink may escape root.
/// O_NONBLOCK ensures a FIFO substituted for a file cannot hang the capture.
pub(crate) fn open_file(root: &Path, path: &str) -> anyhow::Result<Option<File>> {
    match open_entry(root, path, false)? {
        Some((Entry::File(file), _)) => Ok(Some(file)),
        None => Ok(None),
        _ => unreachable!("links are refused"),
    }
}

enum Entry {
    File(File),
    Link(Vec<u8>),
}

impl Entry {
    fn copy_hash(self, output: Option<&mut File>, budget: u64) -> anyhow::Result<(String, u64)> {
        match self {
            Self::File(file) => copy_hash(file, output, budget),
            Self::Link(target) => copy_hash(Cursor::new(target), output, budget),
        }
    }
}

/// Link targets are read as bytes, never followed; ancestor links still refuse the entry.
fn open_entry(root: &Path, path: &str, links: bool) -> anyhow::Result<Option<(Entry, bool)>> {
    valid_path(path)?;
    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(root)?;
    let parts: Vec<_> = path.split('/').collect();
    for (index, part) in parts.iter().enumerate() {
        let name = CString::new(*part)?;
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if index + 1 < parts.len() {
                libc::O_DIRECTORY
            } else {
                0
            };
        // SAFETY: owned directory FD and NUL-terminated component live through openat.
        let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            if links && index + 1 == parts.len() && error.raw_os_error() == Some(libc::ELOOP) {
                let mut target = vec![0u8; 8193];
                // SAFETY: owned parent FD, NUL-terminated name and writable buffer are live.
                let length = unsafe {
                    libc::readlinkat(
                        directory.as_raw_fd(),
                        name.as_ptr(),
                        target.as_mut_ptr().cast(),
                        target.len(),
                    )
                };
                if length < 0 {
                    return Err(std::io::Error::last_os_error())
                        .with_context(|| format!("Could not read snapshot link {path}"));
                }
                ensure!(
                    length > 0 && length < target.len() as isize,
                    "Snapshot link {path} target is too long"
                );
                target.truncate(length as usize);
                return Ok(Some((Entry::Link(target), false)));
            }
            return Err(error).with_context(|| format!("Snapshot cannot read {path}; remove unsupported entries or replace symlink ancestors before retrying"));
        }
        // SAFETY: successful openat returned a fresh FD owned by this File.
        let file = unsafe { File::from_raw_fd(fd) };
        if index + 1 == parts.len() {
            ensure!(
                file.metadata()?.is_file(),
                "Snapshot entry {path} is not a regular file; replace or remove it before transferring"
            );
            let executable = file.metadata()?.permissions().mode() & 0o111 != 0;
            return Ok(Some((Entry::File(file), executable)));
        }
        directory = file;
    }
    unreachable!()
}

pub(crate) fn copy_hash(
    mut source: impl Read,
    mut output: Option<&mut File>,
    budget: u64,
) -> anyhow::Result<(String, u64)> {
    let mut digest = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let length = source.read(&mut buffer)?;
        if length == 0 {
            break;
        }
        size += length as u64;
        ensure!(size <= budget, "Snapshot byte budget exceeded");
        digest.update(&buffer[..length]);
        if let Some(file) = output.as_mut() {
            file.write_all(&buffer[..length])?;
        }
    }
    Ok((hex::encode(digest.finalize()), size))
}

pub(crate) fn private_dir(path: &Path) -> anyhow::Result<()> {
    fs::DirBuilder::new().mode(0o700).create(path)?;
    Ok(())
}
pub(crate) fn private_file(path: &Path) -> anyhow::Result<File> {
    Ok(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?)
}
fn path_text(path: &Path) -> anyhow::Result<&str> {
    path.to_str().context("Non-UTF8 snapshot directory")
}
async fn git_state(
    git: &Git,
    repo: &Path,
) -> anyhow::Result<(String, Option<String>, Vec<u8>, Vec<u8>)> {
    let head = git.run(repo, &["rev-parse", "--verify", "HEAD"]).await?;
    ensure!(
        head.len() == 40 && head.bytes().all(|b| b.is_ascii_hexdigit()),
        "Unborn or unsupported Git object format"
    );
    let branch = git
        .run_allowing(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"], &[1])
        .await?;
    let index = git
        .run_bytes(
            repo,
            &[
                "-c",
                "core.quotePath=true",
                "-c",
                "core.hooksPath=/dev/null",
                "diff",
                "--cached",
                "--binary",
                "--full-index",
                "--no-color",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "HEAD",
                "--",
            ],
        )
        .await?;
    ensure!(index.len() as u64 <= MAX_MANIFEST, "Index patch too large");
    let status = git
        .run_bytes(
            repo,
            &[
                // A running workspace's agents may hold the index lock: read, never refresh.
                "--no-optional-locks",
                "-c",
                "core.hooksPath=/dev/null",
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
            ],
        )
        .await?;
    Ok((
        head,
        if branch.is_empty() {
            None
        } else {
            Some(branch.trim_end().to_owned())
        },
        index,
        status,
    ))
}

/// The files a snapshot carries: tracked ones and untracked ones Git does not ignore.
async fn carried_paths(git: &Git, repo: &Path) -> anyhow::Result<BTreeSet<String>> {
    let listing = git
        .run_bytes(
            repo,
            &[
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
                "-z",
            ],
        )
        .await?;
    let paths: BTreeSet<String> = listing
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| std::str::from_utf8(p).map(String::from))
        .collect::<Result<_, _>>()?;
    ensure!(paths.len() <= MAX_FILES, "Snapshot file budget exceeded");
    for path in &paths {
        valid_path(path)?;
    }
    Ok(paths)
}

/// What a worktree holds as a transfer carries it: its commit, branch, staged changes and each
/// carried file's content and mode. A copy left behind is compared by it when the workspace
/// comes back, so edits made there are never overwritten unseen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contents {
    pub head: String,
    pub branch: Option<String>,
    pub index_digest: String,
    /// Each carried file's SHA-256 (none: tracked but absent) and whether it is executable.
    pub files: BTreeMap<String, (Option<String>, bool)>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub symlinks: BTreeSet<String>,
}

impl GitSnapshot {
    pub fn contents(&self) -> Contents {
        Contents {
            head: self.head.clone(),
            branch: self.branch.clone(),
            index_digest: self.index_digest.clone(),
            symlinks: self
                .files
                .iter()
                .filter(|f| f.symlink)
                .map(|f| f.path.clone())
                .collect(),
            files: self
                .files
                .iter()
                .map(|f| (f.path.clone(), (f.digest.clone(), f.executable)))
                .collect(),
        }
    }
}

/// Reads a worktree's contents as `capture` would record them, writing nothing.
pub async fn observe(git: &Git, repo: &Path) -> anyhow::Result<Contents> {
    let repo = repo.canonicalize()?;
    let state = git_state(git, &repo).await?;
    let mut files = BTreeMap::new();
    let mut symlinks = BTreeSet::new();
    for path in carried_paths(git, &repo).await? {
        let entry = open_entry(&repo, &path, true)?;
        let executable = entry.as_ref().is_some_and(|(_, executable)| *executable);
        if entry
            .as_ref()
            .is_some_and(|(entry, _)| matches!(entry, Entry::Link(_)))
        {
            symlinks.insert(path.clone());
        }
        let digest = entry
            .map(|(entry, _)| entry.copy_hash(None, MAX_BYTES).map(|(digest, _)| digest))
            .transpose()?;
        files.insert(path, (digest, executable));
    }
    Ok(Contents {
        head: state.0,
        branch: state.1,
        index_digest: hex::encode(Sha256::digest(&state.2)),
        files,
        symlinks,
    })
}

/// Checks entry types before fencing the source; capture checks them again after writers stop.
pub async fn check_source(git: &Git, repo: &Path) -> anyhow::Result<u64> {
    let (git, repo) = (git.clone(), repo.to_owned());
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || runtime.block_on(check_source_inner(&git, &repo))).await?
}

async fn check_source_inner(git: &Git, repo: &Path) -> anyhow::Result<u64> {
    let repo = repo.canonicalize()?;
    check_index(git, &repo).await?;
    let mut bytes = 0u64;
    for path in carried_paths(git, &repo).await? {
        bytes = bytes.saturating_add(match open_entry(&repo, &path, true)? {
            Some((Entry::File(file), _)) => file.metadata()?.len(),
            Some((Entry::Link(target), _)) => target.len() as u64,
            None => 0,
        });
    }
    let objects: u64 = git
        .run(&repo, &["rev-list", "--disk-usage", "--objects", "HEAD"])
        .await?
        .trim()
        .parse()
        .context("Could not estimate Git history size")?;
    Ok(bytes.saturating_add(objects))
}

async fn check_index(git: &Git, repo: &Path) -> anyhow::Result<()> {
    let stages = git.run_bytes(repo, &["ls-files", "--stage", "-z"]).await?;
    for entry in stages.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let header = std::str::from_utf8(entry)?
            .split_once('\t')
            .context("Malformed Git index")?
            .0;
        ensure!(
            !header.starts_with("160000 ") && header.ends_with(" 0"),
            "Submodules or unmerged index require explicit handling"
        );
    }
    let flags = git.run_bytes(repo, &["ls-files", "-v", "-z"]).await?;
    ensure!(
        flags
            .split(|b| *b == 0)
            .filter(|e| !e.is_empty())
            .all(|e| e[0] == b'H'),
        "Sparse or assume-unchanged index is unsupported"
    );
    Ok(())
}

/// Captures only the selected worktree, its HEAD ancestry and index delta. No remotes,
/// Git config/hooks, unrelated branches, ignored data, CLI homes or native sessions.
/// The output must not exist; callers retain ownership of a successfully returned snapshot.
pub async fn capture(git: &Git, repo: &Path, output: &Path) -> anyhow::Result<GitSnapshot> {
    let (git, repo, output) = (git.clone(), repo.to_owned(), output.to_owned());
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || runtime.block_on(capture_inner(&git, &repo, &output)))
        .await?
}

async fn capture_inner(git: &Git, repo: &Path, output: &Path) -> anyhow::Result<GitSnapshot> {
    ensure!(output.is_absolute(), "Snapshot output must be absolute");
    let repo = repo.canonicalize()?;
    ensure!(
        !output
            .parent()
            .context("Missing snapshot parent")?
            .canonicalize()?
            .starts_with(&repo),
        "Snapshot must be outside its worktree"
    );
    ensure!(
        Path::new(&git.run(&repo, &["rev-parse", "--show-toplevel"]).await?).canonicalize()?
            == repo,
        "Expected worktree root"
    );
    let state = git_state(git, &repo).await?;
    check_index(git, &repo).await?;
    let paths = carried_paths(git, &repo).await?;
    private_dir(output)?;
    let result = async {
        let payload = output.join("files");
        private_dir(&payload)?;
        let bundle = output.join("git.bundle");
        git.run(&repo, &["bundle", "create", path_text(&bundle)?, "HEAD"])
            .await?;
        fs::set_permissions(&bundle, fs::Permissions::from_mode(0o600))?;
        File::open(&bundle)?.sync_all()?;
        let (bundle_digest, mut bytes) = copy_hash(File::open(&bundle)?, None, MAX_BYTES)?;
        let mut index = private_file(&output.join("index.patch"))?;
        index.write_all(&state.2)?;
        index.sync_all()?;
        bytes += state.2.len() as u64;
        ensure!(bytes <= MAX_BYTES, "Snapshot byte budget exceeded");
        let mut files = Vec::new();
        for path in &paths {
            let source = open_entry(&repo, path, true)?;
            let is_link = source
                .as_ref()
                .is_some_and(|(entry, _)| matches!(entry, Entry::Link(_)));
            let (digest, size, executable) = if let Some((source, executable)) = source {
                let target = payload.join(path);
                fs::create_dir_all(target.parent().context("Missing parent")?)?;
                let mut file = private_file(&target)?;
                let (digest, size) = source.copy_hash(Some(&mut file), MAX_BYTES - bytes)?;
                file.sync_all()?;
                bytes += size;
                (Some(digest), size, executable)
            } else {
                (None, 0, false)
            };
            files.push(SnapshotFile {
                path: path.clone(),
                digest,
                size,
                executable,
                symlink: is_link,
            });
        }
        // Detect ordinary concurrent writes, including content changes invisible to status.
        for entry in &files {
            let current = open_entry(&repo, &entry.path, true)?;
            let executable = current.as_ref().is_some_and(|(_, executable)| *executable);
            let is_link = current
                .as_ref()
                .is_some_and(|(entry, _)| matches!(entry, Entry::Link(_)));
            ensure!(
                executable == entry.executable && is_link == entry.symlink,
                "Source mode changed during capture"
            );
            let digest = current
                .map(|(entry, _)| entry.copy_hash(None, MAX_BYTES))
                .transpose()?;
            ensure!(
                digest.as_ref().map(|d| &d.0) == entry.digest.as_ref()
                    && digest.as_ref().map_or(0, |d| d.1) == entry.size,
                "Source changed during capture"
            );
        }
        ensure!(
            git_state(git, &repo).await? == state,
            "Git state changed during capture"
        );
        let snapshot = GitSnapshot {
            version: 1,
            head: state.0,
            branch: state.1,
            bundle_digest,
            index_digest: hex::encode(Sha256::digest(&state.2)),
            status_digest: hex::encode(Sha256::digest(&state.3)),
            files,
        };
        let manifest = serde_json::to_vec(&snapshot)?;
        ensure!(manifest.len() as u64 <= MAX_MANIFEST, "Manifest too large");
        let mut file = private_file(&output.join("manifest.json"))?;
        file.write_all(&manifest)?;
        file.sync_all()?;
        sync_directories(output)?;
        Ok(snapshot)
    }
    .await;
    if result.is_err() {
        fs::remove_dir_all(output)?;
    }
    result
}

/// Checks a snapshot's files against its manifest without restoring it: every component and
/// payload hash, the limits, and that the folder holds nothing the manifest does not list.
pub fn verify(snapshot: &Path) -> anyhow::Result<GitSnapshot> {
    let manifest = open_file(snapshot, "manifest.json")?.context("Manifest missing")?;
    ensure!(
        manifest.metadata()?.len() <= MAX_MANIFEST,
        "Manifest too large"
    );
    let state: GitSnapshot = serde_json::from_reader(manifest.take(MAX_MANIFEST + 1))?;
    ensure!(
        state.version == 1 && state.files.len() <= MAX_FILES,
        "Unsupported snapshot"
    );
    ensure!(
        state.head.len() == 40 && state.head.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid HEAD"
    );
    let mut paths = BTreeSet::new();
    let mut bytes = 0;
    for (path, expected) in [
        ("git.bundle", &state.bundle_digest),
        ("index.patch", &state.index_digest),
    ] {
        let component = open_file(snapshot, path)?.context("Snapshot component missing")?;
        ensure!(
            path != "index.patch" || component.metadata()?.len() <= MAX_MANIFEST,
            "Index patch too large"
        );
        let (digest, size) = copy_hash(component, None, MAX_BYTES - bytes)?;
        ensure!(digest == *expected, "Snapshot component corrupted");
        bytes += size;
    }
    for entry in &state.files {
        valid_path(&entry.path)?;
        ensure!(paths.insert(entry.path.clone()), "Duplicate snapshot path");
        ensure!(
            !entry.symlink
                || (entry.digest.is_some()
                    && !entry.executable
                    && entry.size > 0
                    && entry.size <= 8192),
            "Invalid snapshot link"
        );
        if let Some(expected) = &entry.digest {
            let (digest, size) = copy_hash(
                open_file(&snapshot.join("files"), &entry.path)?.context("Payload missing")?,
                None,
                MAX_BYTES - bytes,
            )?;
            ensure!(
                digest == *expected && size == entry.size,
                "Snapshot payload corrupted"
            );
            bytes += size;
        } else {
            ensure!(entry.size == 0 && !entry.executable, "Invalid absent file");
        }
    }
    for entry in &state.files {
        for ancestor in Path::new(&entry.path).ancestors().skip(1) {
            if let Some(path) = ancestor.to_str() {
                ensure!(!paths.contains(path), "Snapshot paths overlap at {path}");
            }
        }
    }
    let listed: BTreeSet<String> = state
        .files
        .iter()
        .filter(|entry| entry.digest.is_some())
        .map(|entry| format!("files/{}", entry.path))
        .chain(["manifest.json", "git.bundle", "index.patch"].map(String::from))
        .collect();
    for entry in walkdir::WalkDir::new(snapshot)
        .follow_links(false)
        .min_depth(1)
    {
        let entry = entry?;
        if entry.file_type().is_dir() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(snapshot)?
            .to_str()
            .context("Non-UTF8 snapshot entry")?;
        ensure!(listed.contains(relative), "Unexpected snapshot entry");
    }
    Ok(state)
}

/// Materializes a new independent repo; never overwrites a source/return destination.
/// Hashes detect corruption, not authenticity: transport must authenticate the whole manifest.
/// Native sessions, environment profiles and execution ownership are separate prerequisites.
pub async fn restore(git: &Git, snapshot: &Path, destination: &Path) -> anyhow::Result<()> {
    let (git, snapshot, destination) = (git.clone(), snapshot.to_owned(), destination.to_owned());
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        runtime.block_on(restore_inner(&git, &snapshot, &destination, false))
    })
    .await?
}

/// Restores only the private Git object store; the linked worktree receives the captured files
/// once. Keeping a second checkout in this internal clone would double copying and disk syncs.
pub async fn restore_clone(git: &Git, snapshot: &Path, destination: &Path) -> anyhow::Result<()> {
    let (git, snapshot, destination) = (git.clone(), snapshot.to_owned(), destination.to_owned());
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        runtime.block_on(restore_inner(&git, &snapshot, &destination, true))
    })
    .await?
}

async fn restore_inner(
    git: &Git,
    snapshot: &Path,
    destination: &Path,
    bare: bool,
) -> anyhow::Result<()> {
    ensure!(
        snapshot.is_absolute() && destination.is_absolute(),
        "Snapshot paths must be absolute"
    );
    ensure!(
        !destination
            .parent()
            .context("Missing destination parent")?
            .canonicalize()?
            .starts_with(snapshot.canonicalize()?),
        "Destination must be outside snapshot"
    );
    let state = verify(snapshot)?;
    if let Some(branch) = &state.branch {
        git.run(snapshot, &["check-ref-format", "--branch", branch])
            .await?;
    }
    private_dir(destination)?;
    let result = async {
        restore_git(
            git,
            destination,
            if bare {
                &["init", "--bare", "--template="]
            } else {
                &["init", "--template="]
            },
        )
        .await?;
        restore_git(
            git,
            destination,
            &[
                "fetch",
                "--no-tags",
                path_text(&snapshot.join("git.bundle"))?,
                "HEAD",
            ],
        )
        .await?;
        ensure!(
            restore_git(git, destination, &["rev-parse", "FETCH_HEAD"]).await? == state.head,
            "Bundle HEAD differs from manifest"
        );
        restore_git(
            git,
            destination,
            &["update-ref", "--no-deref", "HEAD", &state.head],
        )
        .await?;
        if let Some(branch) = &state.branch {
            restore_git(
                git,
                destination,
                &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
            )
            .await?;
            restore_git(git, destination, &["update-ref", "HEAD", &state.head]).await?;
        }
        if bare {
            sync_directories(destination)?;
        } else {
            materialize(git, snapshot, destination, &state).await?;
        }
        Ok(())
    }
    .await;
    if result.is_err() {
        fs::remove_dir_all(destination)?;
    }
    result
}

/// Creates a linked worktree from an already restored private clone, preserving its captured
/// index and local files. The clone and destination must belong to the restoration operation.
pub async fn restore_worktree(
    git: &Git,
    snapshot: &Path,
    clone: &Path,
    destination: &Path,
) -> anyhow::Result<()> {
    let state = verify(snapshot)?;
    let destination_text = path_text(destination)?;
    let mut args = vec!["worktree", "add", "--force", "--no-checkout"];
    if state.branch.is_none() {
        args.push("--detach");
    }
    args.push(destination_text);
    args.push(state.branch.as_deref().unwrap_or(&state.head));
    restore_git(git, clone, &args).await?;
    materialize(git, snapshot, destination, &state).await
}

async fn materialize(
    git: &Git,
    snapshot: &Path,
    destination: &Path,
    state: &GitSnapshot,
) -> anyhow::Result<()> {
    restore_git(git, destination, &["read-tree", "HEAD"]).await?;
    let patch = snapshot.join("index.patch");
    if patch.metadata()?.len() > 0 {
        restore_git(
            git,
            destination,
            &["apply", "--cached", "--binary", path_text(&patch)?],
        )
        .await?;
    }
    for entry in state.files.iter().filter(|entry| !entry.symlink) {
        if let Some(expected) = &entry.digest {
            let target = destination.join(&entry.path);
            fs::create_dir_all(target.parent().context("Missing parent")?)?;
            // As `git checkout` writes them: the user's umask decides, so another user (a
            // container's) reads the workspace as it would have read the source's.
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(if entry.executable { 0o777 } else { 0o666 })
                .open(&target)?;
            let (digest, size) = copy_hash(
                open_file(&snapshot.join("files"), &entry.path)?.context("Payload missing")?,
                Some(&mut file),
                MAX_BYTES,
            )?;
            ensure!(
                digest == *expected && size == entry.size,
                "Payload changed during restore"
            );
            file.sync_all()?;
        }
    }
    // Create every parent before links, including aliases on case-insensitive filesystems.
    for entry in state.files.iter().filter(|entry| entry.symlink) {
        let target = destination.join(&entry.path);
        fs::create_dir_all(target.parent().context("Missing link parent")?)?;
    }
    for entry in state.files.iter().filter(|entry| entry.symlink) {
        let mut bytes = Vec::new();
        open_file(&snapshot.join("files"), &entry.path)?
            .context("Link payload missing")?
            .take(8193)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 == entry.size
                && !bytes.contains(&0)
                && Some(hex::encode(Sha256::digest(&bytes))).as_ref() == entry.digest.as_ref(),
            "Link payload changed during restore"
        );
        symlink(
            std::ffi::OsString::from_vec(bytes),
            destination.join(&entry.path),
        )?;
    }
    let restored = git_state(git, destination).await?;
    ensure!(
        restored.0 == state.head
            && restored.1 == state.branch
            && hex::encode(Sha256::digest(&restored.2)) == state.index_digest
            && hex::encode(Sha256::digest(&restored.3)) == state.status_digest,
        "Restored Git state differs; source Git semantics require explicit handling"
    );
    sync_worktree_directories(destination)?;
    Ok(())
}

pub(crate) fn sync_directories(root: &Path) -> anyhow::Result<()> {
    sync_directories_inner(root, false)
}

/// Syncs restored worktrees without following their preserved symbolic links.
pub(crate) fn sync_worktree_directories(root: &Path) -> anyhow::Result<()> {
    sync_directories_inner(root, true)
}

fn sync_directories_inner(root: &Path, links: bool) -> anyhow::Result<()> {
    for entry in walkdir::WalkDir::new(root)
        .contents_first(true)
        .follow_links(false)
    {
        let entry = entry?;
        ensure!(
            links || !entry.file_type().is_symlink(),
            "Unexpected snapshot symlink"
        );
        if entry.file_type().is_dir() {
            File::open(entry.path())?.sync_all()?;
        }
    }
    if let Some(parent) = root.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

pub(crate) async fn restore_git(git: &Git, dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    let mut fixed = vec![
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "gc.auto=0",
        "-c",
        "maintenance.auto=false",
    ];
    fixed.extend_from_slice(args);
    git.run(dir, &fixed).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shellenv::ShellEnv;
    use std::os::unix::fs::symlink;

    async fn fixture() -> (tempfile::TempDir, Git) {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("source");
        fs::create_dir(&repo).unwrap();
        let git = Git::new(ShellEnv::new());
        restore_git(&git, &repo, &["init", "--template=", "-b", "snapshot-test"])
            .await
            .unwrap();
        git.run(&repo, &["config", "user.name", "Snapshot Test"])
            .await
            .unwrap();
        git.run(&repo, &["config", "user.email", "snapshot@example.invalid"])
            .await
            .unwrap();
        fs::write(repo.join("file.txt"), "base\n").unwrap();
        fs::write(repo.join("remove.txt"), "remove\n").unwrap();
        fs::write(repo.join(".gitignore"), "ignored-secret\n").unwrap();
        restore_git(&git, &repo, &["add", "."]).await.unwrap();
        restore_git(&git, &repo, &["commit", "-m", "fixture"])
            .await
            .unwrap();
        (root, git)
    }
    fn rewrite(root: &Path, change: impl FnOnce(&mut serde_json::Value)) {
        let path = root.join("manifest.json");
        let mut value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        change(&mut value);
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    #[tokio::test]
    async fn a_private_clone_does_not_write_a_duplicate_checkout() {
        let (root, git) = fixture().await;
        let source = root.path().join("source");
        fs::write(source.join("untracked.txt"), "keep this").unwrap();
        let snapshot = root.path().join("snapshot");
        capture(&git, &source, &snapshot).await.unwrap();
        let clone = root.path().join("clone");
        restore_clone(&git, &snapshot, &clone).await.unwrap();
        assert_eq!(
            git.run(&clone, &["rev-parse", "--is-bare-repository"])
                .await
                .unwrap(),
            "true"
        );
        assert!(!clone.join("file.txt").exists());
        assert!(!clone.join("untracked.txt").exists());
        let worktree = root.path().join("worktree");
        restore_worktree(&git, &snapshot, &clone, &worktree)
            .await
            .unwrap();
        assert_eq!(
            fs::read_to_string(worktree.join("untracked.txt")).unwrap(),
            "keep this"
        );
        assert_eq!(
            git.run(&source, &["status", "--porcelain=v1"])
                .await
                .unwrap(),
            git.run(&worktree, &["status", "--porcelain=v1"])
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn restores_selected_head_index_binary_worktree_and_untracked_without_git_configuration()
    {
        let (root, git) = fixture().await;
        let repo = root.path().join("source");
        // A commit reachable only through an unrelated branch must not travel.
        restore_git(&git, &repo, &["checkout", "-b", "private-other-branch"])
            .await
            .unwrap();
        fs::write(repo.join("unrelated.txt"), "other branch only").unwrap();
        restore_git(&git, &repo, &["add", "."]).await.unwrap();
        restore_git(&git, &repo, &["commit", "-m", "unrelated"])
            .await
            .unwrap();
        let unrelated = git.run(&repo, &["rev-parse", "HEAD"]).await.unwrap();
        restore_git(&git, &repo, &["checkout", "snapshot-test"])
            .await
            .unwrap();
        git.run(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                "https://secret@example.invalid/private",
            ],
        )
        .await
        .unwrap();
        // Behave like a linked worktree: its .git is a file referencing metadata elsewhere.
        let metadata = root.path().join("source-metadata");
        fs::rename(repo.join(".git"), &metadata).unwrap();
        fs::write(
            repo.join(".git"),
            format!("gitdir: {}\n", metadata.display()),
        )
        .unwrap();
        fs::write(repo.join("file.txt"), b"staged\0binary\n").unwrap();
        git.run(&repo, &["add", "file.txt"]).await.unwrap();
        fs::write(repo.join("file.txt"), b"unstaged\0different\n").unwrap();
        fs::remove_file(repo.join("remove.txt")).unwrap();
        fs::write(repo.join("new\nfile.txt"), b"new\0file\n").unwrap();
        fs::write(repo.join("run.sh"), "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(repo.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(repo.join("ignored-secret"), "never transfer").unwrap();
        let before = git_state(&git, &repo).await.unwrap();
        let snapshot = root.path().join("snapshot");
        capture(&git, &repo, &snapshot).await.unwrap();
        assert!(!snapshot.join("files/.git").exists());
        assert!(!snapshot.join("files/ignored-secret").exists());
        let destination = root.path().join("returned");
        restore(&git, &snapshot, &destination).await.unwrap();
        assert_eq!(git_state(&git, &destination).await.unwrap(), before);
        assert_eq!(
            fs::read(destination.join("file.txt")).unwrap(),
            b"unstaged\0different\n"
        );
        assert_eq!(
            fs::read(destination.join("new\nfile.txt")).unwrap(),
            b"new\0file\n"
        );
        assert!(!destination.join("remove.txt").exists());
        // Modes follow the umask, as a checkout's: compare with files made the same way.
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        for (name, wanted) in [("probe-x", 0o777), ("probe", 0o666)] {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(wanted)
                .open(root.path().join(name))
                .unwrap();
        }
        assert_eq!(
            mode(&destination.join("run.sh")),
            mode(&root.path().join("probe-x"))
        );
        assert_eq!(
            mode(&destination.join("file.txt")),
            mode(&root.path().join("probe"))
        );
        assert!(git.run(&destination, &["remote"]).await.unwrap().is_empty());
        assert!(
            git.run(&destination, &["cat-file", "-e", &unrelated])
                .await
                .is_err()
        );
        assert_eq!(
            fs::metadata(&snapshot).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(snapshot.join("manifest.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[tokio::test]
    async fn preserves_detached_head_and_staged_addition_and_deletion() {
        let (root, git) = fixture().await;
        let repo = root.path().join("source");
        restore_git(&git, &repo, &["checkout", "--detach"])
            .await
            .unwrap();
        fs::write(repo.join("added.txt"), "indexed addition\n").unwrap();
        git.run(&repo, &["add", "added.txt"]).await.unwrap();
        fs::write(repo.join("added.txt"), "later worktree version\n").unwrap();
        git.run(&repo, &["rm", "remove.txt"]).await.unwrap();
        let before = git_state(&git, &repo).await.unwrap();
        let snapshot = root.path().join("snapshot");
        capture(&git, &repo, &snapshot).await.unwrap();
        let dest = root.path().join("restored");
        restore(&git, &snapshot, &dest).await.unwrap();
        assert_eq!(git_state(&git, &dest).await.unwrap(), before);
    }

    #[tokio::test]
    async fn preserves_tracked_untracked_dangling_and_external_links_without_reading_targets() {
        let (root, git) = fixture().await;
        let repo = root.path().join("source");
        fs::create_dir_all(repo.join(".agents/skills")).unwrap();
        fs::create_dir_all(repo.join(".claude/skills/review")).unwrap();
        fs::write(repo.join(".claude/skills/review/SKILL.md"), "review").unwrap();
        symlink(
            "../../.claude/skills/review",
            repo.join(".agents/skills/review"),
        )
        .unwrap();
        git.run(&repo, &["add", "."]).await.unwrap();
        git.run(&repo, &["commit", "-m", "skills"]).await.unwrap();
        fs::remove_file(repo.join(".agents/skills/review")).unwrap();
        symlink("staged-target", repo.join(".agents/skills/review")).unwrap();
        git.run(&repo, &["add", ".agents/skills/review"])
            .await
            .unwrap();
        fs::remove_file(repo.join(".agents/skills/review")).unwrap();
        symlink(
            "../../.claude/skills/review",
            repo.join(".agents/skills/review"),
        )
        .unwrap();
        let outside = root.path().join("private");
        fs::write(&outside, "never read this target").unwrap();
        symlink(&outside, repo.join("external")).unwrap();
        symlink("missing", repo.join("dangling")).unwrap();
        symlink("loop", repo.join("loop")).unwrap();
        let before = observe(&git, &repo).await.unwrap();
        check_source(&git, &repo).await.unwrap();
        let snapshot = root.path().join("snapshot");
        let captured = capture(&git, &repo, &snapshot).await.unwrap();
        assert_eq!(captured.contents(), before);
        assert_eq!(
            fs::read(snapshot.join("files/external")).unwrap(),
            outside.as_os_str().as_encoded_bytes()
        );
        assert!(
            fs::symlink_metadata(snapshot.join("files/external"))
                .unwrap()
                .is_file()
        );
        let dest = root.path().join("restored");
        restore(&git, &snapshot, &dest).await.unwrap();
        assert_eq!(observe(&git, &dest).await.unwrap(), before);
        for name in [".agents/skills/review", "external", "dangling", "loop"] {
            assert_eq!(
                fs::read_link(dest.join(name)).unwrap(),
                fs::read_link(repo.join(name)).unwrap()
            );
        }
        // Replacing a link with a regular file containing the same bytes is still a conflict.
        fs::remove_file(dest.join("dangling")).unwrap();
        fs::write(dest.join("dangling"), "missing").unwrap();
        assert_ne!(observe(&git, &dest).await.unwrap(), before);
        assert_eq!(fs::read(&outside).unwrap(), b"never read this target");
    }

    #[tokio::test]
    async fn refuses_payloads_below_a_link_before_materializing_anything() {
        let (root, git) = fixture().await;
        let repo = root.path().join("source");
        symlink(root.path(), repo.join("escape")).unwrap();
        let snapshot = root.path().join("snapshot");
        capture(&git, &repo, &snapshot).await.unwrap();
        rewrite(&snapshot, |m| {
            m["files"].as_array_mut().unwrap().push(serde_json::json!({
                "path": "escape/overwrite", "digest": null, "size": 0, "executable": false
            }));
        });
        assert!(format!("{:#}", verify(&snapshot).unwrap_err()).contains("overlap"));
        let dest = root.path().join("restored");
        assert!(restore(&git, &snapshot, &dest).await.is_err());
        assert!(!dest.exists());
        assert!(!root.path().join("overwrite").exists());
    }

    #[tokio::test]
    async fn refuses_existing_destinations_and_corruption_before_materializing() {
        let (root, git) = fixture().await;
        let repo = root.path().join("source");
        let snapshot = root.path().join("snapshot");
        capture(&git, &repo, &snapshot).await.unwrap();
        assert!(capture(&git, &repo, &snapshot).await.is_err());
        assert!(snapshot.join("manifest.json").exists());
        assert!(restore(&git, &snapshot, &repo).await.is_err());
        assert_eq!(fs::read(repo.join("file.txt")).unwrap(), b"base\n");
        fs::write(snapshot.join("files/file.txt"), "corrupted").unwrap();
        let dest = root.path().join("restored");
        assert!(restore(&git, &snapshot, &dest).await.is_err());
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn refuses_symlink_ancestors_and_leaves_no_partial_snapshot() {
        let (root, git) = fixture().await;
        let repo = root.path().join("source");
        let outside = root.path().join("outside");
        fs::write(&outside, "private").unwrap();
        symlink(&outside, repo.join("link")).unwrap();
        let snapshot = root.path().join("snapshot");
        capture(&git, &repo, &snapshot).await.unwrap();
        fs::remove_dir_all(&snapshot).unwrap();
        fs::remove_file(repo.join("link")).unwrap();
        fs::create_dir(repo.join("nested")).unwrap();
        fs::write(repo.join("nested/file"), "tracked").unwrap();
        git.run(&repo, &["add", "nested/file"]).await.unwrap();
        fs::remove_dir_all(repo.join("nested")).unwrap();
        symlink(root.path(), repo.join("nested")).unwrap();
        assert!(check_source(&git, &repo).await.is_err());
        assert!(capture(&git, &repo, &snapshot).await.is_err());
        assert!(!snapshot.exists());
        assert_eq!(fs::read(&outside).unwrap(), b"private");
    }

    #[tokio::test]
    async fn rejects_unsafe_manifest_paths_and_wrong_bundle_head_without_overwrites() {
        let (root, git) = fixture().await;
        let snapshot = root.path().join("snapshot");
        capture(&git, &root.path().join("source"), &snapshot)
            .await
            .unwrap();
        let original = fs::read(snapshot.join("manifest.json")).unwrap();
        let dest = root.path().join("restored");
        for path in [
            "../escape",
            "/absolute",
            ".git/hooks/post-checkout",
            ".GIT/config",
            "x/../../escape",
            "x\\escape",
        ] {
            fs::write(snapshot.join("manifest.json"), &original).unwrap();
            rewrite(&snapshot, |m| m["files"][0]["path"] = path.into());
            assert!(restore(&git, &snapshot, &dest).await.is_err(), "{path}");
            assert!(!dest.exists());
        }
        fs::write(snapshot.join("manifest.json"), &original).unwrap();
        rewrite(&snapshot, |m| m["head"] = "0".repeat(40).into());
        assert!(restore(&git, &snapshot, &dest).await.is_err());
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn refuses_corrupted_bundle_or_index_before_creating_a_repository() {
        let (root, git) = fixture().await;
        let snapshot = root.path().join("snapshot");
        capture(&git, &root.path().join("source"), &snapshot)
            .await
            .unwrap();
        let dest = root.path().join("restored");
        for component in ["git.bundle", "index.patch"] {
            let path = snapshot.join(component);
            let original = fs::read(&path).unwrap();
            fs::write(&path, b"corrupted component").unwrap();
            assert!(restore(&git, &snapshot, &dest).await.is_err());
            assert!(!dest.exists());
            fs::write(path, original).unwrap();
        }
    }

    #[tokio::test]
    async fn refuses_a_restore_whose_git_semantics_would_silently_change_status() {
        let (root, git) = fixture().await;
        let repo = root.path().join("source");
        git.run(&repo, &["config", "core.filemode", "false"])
            .await
            .unwrap();
        fs::set_permissions(repo.join("file.txt"), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(git_state(&git, &repo).await.unwrap().3.is_empty());
        let snapshot = root.path().join("snapshot");
        capture(&git, &repo, &snapshot).await.unwrap();
        let dest = root.path().join("restored");
        assert!(restore(&git, &snapshot, &dest).await.is_err());
        assert!(!dest.exists());
        assert!(repo.join("file.txt").exists());
    }

    #[tokio::test]
    async fn blocks_hidden_index_entries_and_unborn_repositories() {
        let (root, git) = fixture().await;
        let repo = root.path().join("source");
        git.run(&repo, &["update-index", "--assume-unchanged", "file.txt"])
            .await
            .unwrap();
        let snapshot = root.path().join("snapshot");
        assert!(capture(&git, &repo, &snapshot).await.is_err());
        assert!(!snapshot.exists());
        let unborn = root.path().join("unborn");
        fs::create_dir(&unborn).unwrap();
        restore_git(&git, &unborn, &["init", "--template="])
            .await
            .unwrap();
        assert!(capture(&git, &unborn, &snapshot).await.is_err());
        assert!(!snapshot.exists());
    }
}
