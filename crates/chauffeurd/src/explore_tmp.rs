//! An exploration's private temporary folder, given to its Claude sessions as
//! `CLAUDE_CODE_TMPDIR`.
//!
//! Claude Code's OS sandbox lets shell commands write to the session's temporary folder, by
//! default `/tmp/claude-<uid>`: shared by every Claude Code session of the user, and never
//! cleaned. Pointed at a folder of the exploration's own, what its commands write there is
//! removed with it, at archive and at promote, and they cannot reach other sessions' files.
//!
//! Claude Code only keeps the folder when `<folder>/claude-<uid>` fits in 44 bytes (a Unix socket
//! path's limit, with room for its sockets); a longer one falls back to the shared folder. Hence
//! `/tmp/chauffeur-<uid>/<10 characters of the workspace id>`, 0700 and owned by the user at
//! every level, never through a symlink: `/tmp` is shared, another user could plant a folder.

use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Claude Code's limit on `<CLAUDE_CODE_TMPDIR>/claude-<uid>`.
const CLAUDE_TMP_LIMIT: usize = 44;

/// The folder of a workspace: the end of its id (a UUID v7, random there), short.
pub fn dir(root: &Path, workspace_id: &str) -> PathBuf {
    let hex: String = workspace_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    root.join(&hex[hex.len().saturating_sub(10)..])
}

/// Makes the workspace's folder (and the root) when missing; fails on one that is not a real
/// folder, the user's alone.
pub fn ensure(root: &Path, workspace_id: &str) -> io::Result<PathBuf> {
    let dir = dir(root, workspace_id);
    private_folder(root)?;
    private_folder(&dir)?;
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    let claude = format!("{}/claude-{uid}", dir.display());
    if claude.len() > CLAUDE_TMP_LIMIT {
        tracing::warn!(
            dir = %dir.display(),
            "an exploration's temporary folder is too long for Claude Code, which falls back to its shared one"
        );
    }
    Ok(dir)
}

/// Removes the workspace's folder and what its sessions left there; none is fine.
pub fn remove(root: &Path, workspace_id: &str) -> io::Result<()> {
    match std::fs::remove_dir_all(dir(root, workspace_id)) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn private_folder(dir: &Path) -> io::Result<()> {
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let meta = std::fs::symlink_metadata(dir)?;
    // SAFETY: getuid cannot fail.
    if !meta.is_dir() || meta.uid() != unsafe { libc::getuid() } {
        return Err(io::Error::other(format!(
            "{} is not a folder of this user's",
            dir.display()
        )));
    }
    if meta.mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "01a14ffc-4b4d-74c6-82ef-29f0bbc4a593";

    #[test]
    fn the_folder_is_short_and_the_users_alone() {
        assert_eq!(
            dir(Path::new("/tmp/chauffeur-501"), ID),
            PathBuf::from("/tmp/chauffeur-501/f0bbc4a593")
        );
        // Claude Code keeps it for the uids of macOS (501…) and Linux (1000…, up to 5 digits).
        for uid in ["501", "1000", "65534"] {
            let root = PathBuf::from(format!("/tmp/chauffeur-{uid}"));
            let claude = format!("{}/claude-{uid}", dir(&root, ID).display());
            assert!(claude.len() <= CLAUDE_TMP_LIMIT, "{claude}");
        }

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let made = ensure(&root, ID).unwrap();
        assert_eq!(made, dir(&root, ID));
        for folder in [&root, &made] {
            let meta = std::fs::symlink_metadata(folder).unwrap();
            assert!(meta.is_dir());
            assert_eq!(meta.mode() & 0o777, 0o700, "{}", folder.display());
        }
        // Again: kept as it is, a wider mode narrowed.
        std::fs::write(made.join("left"), "x").unwrap();
        std::fs::set_permissions(&made, std::fs::Permissions::from_mode(0o755)).unwrap();
        ensure(&root, ID).unwrap();
        assert!(made.join("left").exists());
        assert_eq!(std::fs::metadata(&made).unwrap().mode() & 0o777, 0o700);

        remove(&root, ID).unwrap();
        assert!(!made.exists());
        assert!(root.exists(), "the root stays: other explorations use it");
        remove(&root, ID).unwrap();
    }

    #[test]
    fn a_symlink_is_never_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        let root = tmp.path().join("root");
        std::os::unix::fs::symlink(&elsewhere, &root).unwrap();
        assert!(ensure(&root, ID).is_err());
        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);

        let root = tmp.path().join("real-root");
        std::fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(&elsewhere, dir(&root, ID)).unwrap();
        assert!(ensure(&root, ID).is_err());
    }
}
