use std::path::{Path, PathBuf};

/// Where Chauffeur keeps its state and its workspaces.
///
/// Defaults: state in `~/.chauffeur`, workspaces in `~/chauffeur/workspaces`, clones made by
/// Chauffeur in `~/chauffeur/repos`. `CHAUFFEUR_HOME`, `CHAUFFEUR_WORKSPACES_DIR` and
/// `CHAUFFEUR_REPOS_DIR` override them (tests point them at temporary directories).
#[derive(Clone, Debug)]
pub struct Paths {
    pub home: PathBuf,
    pub workspaces_dir: PathBuf,
    pub repos_dir: PathBuf,
    pub db: PathBuf,
    pub token: PathBuf,
    pub socket: PathBuf,
    pub daemon_info: PathBuf,
    pub lock: PathBuf,
    pub logs_dir: PathBuf,
    pub settings: PathBuf,
    /// Notes of archived workspaces, by slug.
    pub archive_dir: PathBuf,
    /// Where each exploration gets its private temporary folder (see `explore_tmp`):
    /// `/tmp/chauffeur-<uid>`, short enough for Claude Code to keep it (`CHAUFFEUR_TMP_DIR`
    /// overrides it).
    pub explore_tmp_root: PathBuf,
}

impl Paths {
    /// Fails when no home folder can be found: the state must never land in a shared `/tmp`,
    /// where another user could plant its files first.
    pub fn from_env() -> anyhow::Result<Paths> {
        let user_home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .or_else(passwd_home)
            .ok_or_else(|| anyhow::anyhow!("no home folder: set HOME or CHAUFFEUR_HOME"))?;
        let home = std::env::var_os("CHAUFFEUR_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| user_home.join(".chauffeur"));
        let workspaces_dir = std::env::var_os("CHAUFFEUR_WORKSPACES_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| user_home.join("chauffeur").join("workspaces"));
        let repos_dir = std::env::var_os("CHAUFFEUR_REPOS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| user_home.join("chauffeur").join("repos"));
        let mut paths = Paths::new(home, workspaces_dir, repos_dir);
        paths.explore_tmp_root = std::env::var_os("CHAUFFEUR_TMP_DIR")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            // SAFETY: getuid cannot fail.
            .unwrap_or_else(|| {
                PathBuf::from(format!("/tmp/chauffeur-{}", unsafe { libc::getuid() }))
            });
        Ok(paths)
    }

    pub fn new(home: PathBuf, workspaces_dir: PathBuf, repos_dir: PathBuf) -> Paths {
        Paths {
            db: home.join("chauffeur.db"),
            token: home.join("token"),
            socket: home.join("chauffeurd.sock"),
            daemon_info: home.join("daemon.json"),
            lock: home.join("daemon.lock"),
            logs_dir: home.join("logs"),
            settings: home.join("settings.toml"),
            archive_dir: home.join("archive"),
            // Tests: inside their temporary home. A daemon gets `/tmp/chauffeur-<uid>` (from_env).
            explore_tmp_root: home.join("explore-tmp"),
            home,
            workspaces_dir,
            repos_dir,
        }
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        for dir in [
            &self.home,
            &self.workspaces_dir,
            &self.repos_dir,
            &self.logs_dir,
            &self.archive_dir,
        ] {
            std::fs::create_dir_all(dir)?;
        }
        // The state folder holds the token, the socket, every chat's transcript and the logs:
        // the user's alone, whatever umask created it (a 755 home on Linux exposes it all).
        for dir in [&self.home, &self.logs_dir, &self.archive_dir] {
            private_dir(dir)?;
        }
        Ok(())
    }

    pub fn workspace_dir(&self, slug: &str) -> PathBuf {
        self.workspaces_dir.join(slug)
    }
}

/// Makes a directory `0700`.
pub fn private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// Makes a file `0600`; a missing file is fine.
pub fn private_file(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

/// Opens a log file for appending, created `0600`: agents' output lands there.
pub fn open_private_log(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    // `mode` only applies on creation: a log created by an older build stays as it was.
    private_file(path)?;
    Ok(file)
}

/// The account's home folder, for a daemon launchd or systemd starts without `HOME`.
fn passwd_home() -> Option<PathBuf> {
    // SAFETY: getpwuid returns a pointer into static storage, or null; copied out at once.
    unsafe {
        let entry = libc::getpwuid(libc::getuid());
        (!entry.is_null() && !(*entry).pw_dir.is_null()).then(|| {
            PathBuf::from(
                std::ffi::CStr::from_ptr((*entry).pw_dir)
                    .to_string_lossy()
                    .into_owned(),
            )
        })
    }
    .filter(|home| !home.as_os_str().is_empty())
}

/// Folder of a workspace that holds its notes and generated files; never a worktree.
pub const NOTES_DIR: &str = ".chauffeur";
/// Folder of the notes folder where Claude Code writes its plans.
pub const PLANS_DIR: &str = "plans";
/// Folder of the notes folder that holds each design chat's screens, one folder per chat.
pub const DESIGNS_DIR: &str = "designs";
/// Folder of a workspace that holds the worktree groups agents create.
pub const AGENT_WORKTREES_DIR: &str = ".worktrees";

pub fn is_inside(path: &Path, dir: &Path) -> bool {
    path.starts_with(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn the_state_folder_is_the_users_alone_whatever_umask_made_it() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("state");
        std::fs::create_dir(&home).unwrap();
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755)).unwrap();
        let paths = Paths::new(
            home.clone(),
            temp.path().join("workspaces"),
            temp.path().join("repos"),
        );
        paths.ensure().unwrap();
        for dir in [&paths.home, &paths.logs_dir, &paths.archive_dir] {
            let mode = std::fs::metadata(dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "{}", dir.display());
        }
        // Workspaces and clones are the user's project folders: left as umask makes them.
        let mode = std::fs::metadata(&paths.workspaces_dir)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_ne!(mode, 0o700);
    }

    #[test]
    fn logs_are_private_even_when_they_predate_the_rule() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("old.log");
        std::fs::write(&log, "x").unwrap();
        std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o644)).unwrap();
        open_private_log(&log).unwrap();
        assert_eq!(
            std::fs::metadata(&log).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let fresh = temp.path().join("new.log");
        open_private_log(&fresh).unwrap();
        assert_eq!(
            std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
