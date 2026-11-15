//! The versions of a deliverable (docs/adr/deliverable-versions.md): copies of its folder,
//! `.chauffeur/artifacts/<deliverable>/`, in its `.versions/<n>/`, taken after each turn that
//! changed it and after each change by hand. Restoring copies one back as a new version.
//!
//! A version holds the deliverable's files, not its `exports/` (made again from them) nor its
//! dotfiles. A deliverable lying alone at the top of `artifacts/` has no folder, so no versions.

use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use chauffeur_protocol::deck::{DeliverableVersion, VersionOrigin};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::paths::NOTES_DIR;
use crate::{Error, Result};

pub const ARTIFACTS_DIR: &str = "artifacts";
pub const VERSIONS_DIR: &str = ".versions";
pub const EXPORTS_DIR: &str = "exports";
const INDEX: &str = "index.json";
/// A deliverable heavier than this is not copied at each version.
const MAX_BYTES: u64 = 100 * 1024 * 1024;

/// One writer of versions at a time: a turn's end and a change by hand may meet.
static WRITING: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize, Default)]
struct Index {
    versions: Vec<Entry>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Entry {
    n: u32,
    at: String,
    by: VersionOrigin,
    digest: String,
}

impl Entry {
    fn info(&self) -> DeliverableVersion {
        DeliverableVersion {
            n: self.n,
            at: self.at.clone(),
            by: self.by,
        }
    }
}

/// The deliverable folder `path` (as `files.read` names a file) belongs to: the first folder
/// under `.chauffeur/artifacts/`. None for a path elsewhere or a file at the top of `artifacts/`.
pub fn folder_of(workspace_path: &Path, path: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = Path::new(path)
        .components()
        .map(|c| match c {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Option<_>>()?;
    match parts.as_slice() {
        [notes, artifacts, name, _, ..]
            if *notes == NOTES_DIR && *artifacts == ARTIFACTS_DIR && !name.starts_with('.') =>
        {
            Some(
                workspace_path
                    .join(NOTES_DIR)
                    .join(ARTIFACTS_DIR)
                    .join(name),
            )
        }
        _ => None,
    }
}

/// Whether a file of a deliverable folder, relative to it, is part of its versions.
fn versioned(relative: &Path) -> bool {
    let mut components = relative.components();
    let first = components.next();
    !relative
        .components()
        .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
        && first.is_some_and(|c| c.as_os_str() != EXPORTS_DIR)
}

/// The deliverable's files, relative to its folder, sorted; none when it is heavier than a version
/// may be.
fn files(folder: &Path) -> Option<Vec<PathBuf>> {
    let mut total = 0;
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(folder)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || e.path().strip_prefix(folder).is_ok_and(versioned))
        .flatten()
    {
        if !entry.file_type().is_file() {
            continue;
        }
        total += entry.metadata().map_or(0, |m| m.len());
        if total > MAX_BYTES {
            return None;
        }
        let relative = entry.path().strip_prefix(folder).ok()?.to_path_buf();
        out.push(relative);
    }
    out.sort();
    Some(out)
}

fn digest(folder: &Path, files: &[PathBuf]) -> String {
    let mut hash = Sha256::new();
    for file in files {
        hash.update(file.to_string_lossy().as_bytes());
        hash.update([0]);
        hash.update(std::fs::read(folder.join(file)).unwrap_or_default());
        hash.update([0]);
    }
    hex::encode(hash.finalize())
}

/// The digest of what a folder holds under the given names (files or folders), for comparing a
/// brand with the copy a deliverable made of it.
pub fn digest_of(folder: &Path, names: &[&str]) -> String {
    let mut all = Vec::new();
    for name in names {
        let path = folder.join(name);
        if path.is_file() {
            all.push(PathBuf::from(name));
        } else if path.is_dir() {
            for entry in walkdir::WalkDir::new(&path)
                .follow_links(false)
                .into_iter()
                .flatten()
            {
                if entry.file_type().is_file()
                    && let Ok(relative) = entry.path().strip_prefix(folder)
                {
                    all.push(relative.to_path_buf());
                }
            }
        }
    }
    all.sort();
    digest(folder, &all)
}

fn read_index(folder: &Path) -> Index {
    std::fs::read_to_string(folder.join(VERSIONS_DIR).join(INDEX))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_index(folder: &Path, index: &Index) -> Result<()> {
    let dir = folder.join(VERSIONS_DIR);
    let tmp = dir.join(format!(".{INDEX}.tmp"));
    std::fs::write(
        &tmp,
        serde_json::to_string_pretty(index).expect("index serializes"),
    )?;
    std::fs::rename(&tmp, dir.join(INDEX))?;
    Ok(())
}

/// A deliverable folder that is a real folder, never reached through a symlink.
fn real_folder(folder: &Path) -> bool {
    std::fs::symlink_metadata(folder).is_ok_and(|m| m.is_dir())
}

/// Keeps a version of the folder, unless it is the same as the last one; the version kept.
pub fn snapshot(folder: &Path, by: VersionOrigin) -> Result<Option<DeliverableVersion>> {
    let _writing = WRITING.lock().unwrap();
    if !real_folder(folder) {
        return Ok(None);
    }
    let Some(files) = files(folder) else {
        tracing::warn!(folder = %folder.display(), "deliverable too heavy for versions");
        return Ok(None);
    };
    if files.is_empty() {
        return Ok(None);
    }
    let digest = digest(folder, &files);
    let mut index = read_index(folder);
    if index
        .versions
        .last()
        .is_some_and(|last| last.digest == digest)
    {
        return Ok(None);
    }
    let n = index.versions.last().map_or(1, |last| last.n + 1);
    let target = folder.join(VERSIONS_DIR).join(n.to_string());
    if target.exists() {
        std::fs::remove_dir_all(&target)?;
    }
    for file in &files {
        let to = target.join(file);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(folder.join(file), &to)?;
    }
    let entry = Entry {
        n,
        at: crate::store::now(),
        by,
        digest,
    };
    index.versions.push(entry.clone());
    write_index(folder, &index)?;
    Ok(Some(entry.info()))
}

/// The folder's versions, the newest first.
pub fn list(folder: &Path) -> Vec<DeliverableVersion> {
    read_index(folder)
        .versions
        .iter()
        .rev()
        .map(Entry::info)
        .collect()
}

/// Where version `n` of a folder lies; refused when there is no such version.
pub fn version_dir(folder: &Path, n: u32) -> Result<PathBuf> {
    let dir = folder.join(VERSIONS_DIR).join(n.to_string());
    if !read_index(folder).versions.iter().any(|v| v.n == n) || !real_folder(&dir) {
        return Err(Error::NotFound(format!("no version {n} of this document")));
    }
    Ok(dir)
}

/// Copies version `n` back over the deliverable's files, then keeps that as a new version.
pub fn restore(folder: &Path, n: u32) -> Result<DeliverableVersion> {
    let source = version_dir(folder, n)?;
    let current = files(folder).unwrap_or_default();
    let wanted = files(&source).unwrap_or_default();
    {
        let _writing = WRITING.lock().unwrap();
        for file in current.iter().filter(|f| !wanted.contains(f)) {
            std::fs::remove_file(folder.join(file))?;
        }
        for file in &wanted {
            let to = folder.join(file);
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(source.join(file), &to)?;
        }
    }
    match snapshot(folder, VersionOrigin::Restore)? {
        Some(version) => Ok(version),
        // Already what version n holds: the newest version stands for it.
        None => list(folder)
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound("no version of this document".into())),
    }
}

/// After an agent's turn: a version of every deliverable folder of the workspace it changed.
pub fn after_turn(workspace_path: &Path) {
    let artifacts = workspace_path.join(NOTES_DIR).join(ARTIFACTS_DIR);
    if !real_folder(&artifacts) {
        return;
    }
    for entry in std::fs::read_dir(&artifacts)
        .into_iter()
        .flatten()
        .flatten()
    {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') || !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        if let Err(error) = snapshot(&entry.path(), VersionOrigin::Agent) {
            tracing::warn!(%error, folder = %entry.path().display(), "could not keep a version of the deliverable");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deliverable_folder_is_the_first_folder_under_the_artifacts() {
        let ws = Path::new("/w");
        assert_eq!(
            folder_of(ws, ".chauffeur/artifacts/pitch/deck.json"),
            Some(PathBuf::from("/w/.chauffeur/artifacts/pitch"))
        );
        assert_eq!(
            folder_of(ws, ".chauffeur/artifacts/pitch/slides/01.html"),
            Some(PathBuf::from("/w/.chauffeur/artifacts/pitch"))
        );
        assert_eq!(folder_of(ws, ".chauffeur/artifacts/alone.html"), None);
        assert_eq!(folder_of(ws, ".chauffeur/artifacts/../x/y.html"), None);
        assert_eq!(folder_of(ws, "repo/.chauffeur/artifacts/x/y.html"), None);
    }

    #[test]
    fn a_version_is_kept_only_when_the_files_changed_and_never_holds_exports() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("pitch");
        std::fs::create_dir_all(folder.join(EXPORTS_DIR)).unwrap();
        std::fs::write(folder.join("deck.json"), "{}").unwrap();
        std::fs::write(folder.join(EXPORTS_DIR).join("pitch.pdf"), "pdf").unwrap();
        let first = snapshot(&folder, VersionOrigin::Agent).unwrap().unwrap();
        assert_eq!(first.n, 1);
        assert!(snapshot(&folder, VersionOrigin::Agent).unwrap().is_none());
        assert!(
            !folder
                .join(VERSIONS_DIR)
                .join("1")
                .join(EXPORTS_DIR)
                .exists()
        );
        std::fs::write(folder.join("deck.json"), "{\"a\":1}").unwrap();
        let second = snapshot(&folder, VersionOrigin::Hand).unwrap().unwrap();
        assert_eq!((second.n, second.by), (2, VersionOrigin::Hand));
        let listed: Vec<u32> = list(&folder).iter().map(|v| v.n).collect();
        assert_eq!(listed, [2, 1]);
    }

    #[test]
    fn restoring_brings_an_earlier_version_back_as_a_new_one() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("doc");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("index.html"), "one").unwrap();
        snapshot(&folder, VersionOrigin::Agent).unwrap();
        std::fs::write(folder.join("index.html"), "two").unwrap();
        std::fs::write(folder.join("extra.css"), "x").unwrap();
        snapshot(&folder, VersionOrigin::Agent).unwrap();
        let restored = restore(&folder, 1).unwrap();
        assert_eq!((restored.n, restored.by), (3, VersionOrigin::Restore));
        assert_eq!(
            std::fs::read_to_string(folder.join("index.html")).unwrap(),
            "one"
        );
        assert!(!folder.join("extra.css").exists());
        assert!(restore(&folder, 9).is_err());
    }
}
