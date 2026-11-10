//! API keys the daemon keeps for the user: in the macOS Keychain; on Linux in the Secret Service
//! (GNOME Keyring, KWallet…), else in `~/.config/chauffeur/<file>`, readable by the user only. A
//! daemon whose state is not `~/.chauffeur` (tests and scripts set `CHAUFFEUR_HOME`) keeps them in
//! its own `secrets/` folder instead, so that it never touches the user's. So does a GCP worker
//! (docs/cloud-workers.md), whose installation marks its state folder with a `worker` file: a
//! headless VM has no Secret Service, and it is a dedicated, single-user endpoint.
//!
//! A secret reaches `security` and `secret-tool` on their standard input, never on a command line
//! (where any process list would show it).

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

use crate::paths::Paths;

/// A silent miss that may hide a protected secret behind a locked collection. Setup stops on
/// both, rather than writing a new secret over one it cannot see: unlocking the keyring is what
/// the user can do.
#[derive(Debug, thiserror::Error)]
pub enum ProtectedStoreLocked {
    /// The Secret Service lists the secret as locked (the unlock prompt was dismissed or could
    /// not show).
    #[error(
        "The system keyring holding Chauffeur's cloud secrets is locked; unlock it and try again"
    )]
    Locked,
    /// The Secret Service could not be asked whether the secret is locked.
    #[error(
        "Could not check whether the system keyring holding Chauffeur's cloud secrets is locked; make sure it is running and unlocked, then try again"
    )]
    Unchecked,
}

/// Turns a protected secret's failure into a request error: a locked keyring stays apart from
/// `otherwise`, which every other failure becomes.
pub fn protected_failure(
    otherwise: fn() -> crate::error::Error,
) -> impl FnOnce(anyhow::Error) -> crate::error::Error {
    move |error| {
        if error.is::<ProtectedStoreLocked>() {
            crate::error::Error::Locked(error.to_string())
        } else {
            otherwise()
        }
    }
}

/// Where a secret lives in each store.
pub struct SecretName<'a> {
    /// The Keychain service, or the Secret Service attribute `service`.
    pub service: &'a str,
    /// The file name, in `~/.config/chauffeur/` or in a test daemon's `secrets/`.
    pub file: &'a str,
    /// What the Keychain and the keyring show.
    pub label: &'static str,
}

pub struct Secrets {
    /// Set when the daemon's state is not the user's: the secrets stay in this folder.
    folder: Option<PathBuf>,
}

impl Secrets {
    pub fn for_paths(paths: &Paths) -> Secrets {
        let user_state =
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".chauffeur"));
        Secrets {
            folder: folder_for(&paths.home, user_state.as_deref()),
        }
    }

    pub fn read(&self, name: &SecretName) -> Option<String> {
        match &self.folder {
            Some(folder) => read_file(&folder.join(name.file)),
            None => system::read(name),
        }
    }

    pub fn store(&self, name: &SecretName, value: &str) -> anyhow::Result<()> {
        // Printable ASCII without quotes or backslashes: API keys, and nothing `security -i` would
        // need escaped.
        if value.is_empty()
            || !value
                .chars()
                .all(|c| c.is_ascii_graphic() && c != '"' && c != '\\')
        {
            bail!("an API key is letters, digits and punctuation, without spaces or quotes");
        }
        match &self.folder {
            Some(folder) => write_file(&folder.join(name.file), value),
            None => system::store(name, value),
        }?;
        if self.read(name).as_deref() != Some(value) {
            bail!("the key could not be read back after saving it");
        }
        Ok(())
    }

    /// Root keys must distinguish absence from a locked/unavailable backend, and never fall
    /// back to a plaintext Linux config file. Isolated daemons still use only their own folder.
    pub fn read_protected(&self, name: &SecretName) -> anyhow::Result<Option<String>> {
        match &self.folder {
            Some(folder) => read_protected_file(&folder.join(name.file)),
            None => protected_system::read(name),
        }
    }

    pub fn create_protected(&self, name: &SecretName, value: &str) -> anyhow::Result<()> {
        if value.is_empty()
            || !value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            bail!("Invalid protected secret encoding");
        }
        if self.read_protected(name)?.is_some() {
            bail!("Protected secret already exists");
        }
        match &self.folder {
            Some(folder) => {
                use std::io::Write;
                use std::os::unix::fs::OpenOptionsExt;
                std::fs::create_dir_all(folder)?;
                crate::paths::private_dir(folder)?;
                let mut file = std::fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .mode(0o600)
                    .open(folder.join(name.file))?;
                file.write_all(value.as_bytes())?;
                file.sync_all()?;
                std::fs::File::open(folder)?.sync_all()?;
            }
            None => protected_system::create(name, value)?,
        }
        if self.read_protected(name)?.as_deref() != Some(value) {
            bail!("Protected secret verification failed");
        }
        Ok(())
    }

    pub fn delete(&self, name: &SecretName) -> anyhow::Result<()> {
        match &self.folder {
            Some(folder) => remove_file(&folder.join(name.file)),
            None => system::delete(name),
        }
    }
}

const MAX_PROTECTED_FILE: u64 = 128 * 1024;

/// The state folder's own `secrets/`, unless it is the user's `~/.chauffeur` on a machine that is
/// not a worker: the system's store then.
fn folder_for(home: &Path, user_state: Option<&Path>) -> Option<PathBuf> {
    (user_state != Some(home) || home.join("worker").is_file()).then(|| home.join("secrets"))
}

fn read_protected_file(path: &Path) -> anyhow::Result<Option<String>> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => bail!("Protected secret cannot be read"),
    };
    let metadata = file.metadata()?;
    // SAFETY: geteuid has no preconditions. Inspect the opened inode, never a path checked earlier.
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
        || metadata.len() > MAX_PROTECTED_FILE
    {
        bail!("Protected secret file is not private or exceeds its limit");
    }
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    // The descriptor may grow after metadata was inspected; never read an unbounded secret.
    file.take(MAX_PROTECTED_FILE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow::anyhow!("Protected secret cannot be read"))?;
    if bytes.len() as u64 > MAX_PROTECTED_FILE {
        bail!("Protected secret file exceeds its limit");
    }
    let value = std::str::from_utf8(&bytes)
        .map_err(|_| anyhow::anyhow!("Protected secret cannot be read"))?;
    if value.trim().is_empty() {
        bail!("Protected secret is empty");
    }
    Ok(Some(value.trim().to_owned()))
}

fn read_file(path: &Path) -> Option<String> {
    let value = std::fs::read_to_string(path).ok()?;
    Some(value.trim().to_string()).filter(|value| !value.is_empty())
}

fn write_file(path: &Path, value: &str) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        crate::paths::private_dir(dir)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("writing {}", path.display()))?;
    file.write_all(value.as_bytes())?;
    Ok(())
}

fn remove_file(path: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("removing {}", path.display()))
        }
        _ => Ok(()),
    }
}

/// Runs a command with `input` on its standard input; its trimmed output when it succeeds.
fn run(command: &mut std::process::Command, input: Option<&str>) -> anyhow::Result<String> {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(input.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(target_os = "macos")]
mod system {
    use std::process::Command;

    use super::{SecretName, run};

    pub fn read(name: &SecretName) -> Option<String> {
        run(
            Command::new("security").args(["find-generic-password", "-s", name.service, "-w"]),
            None,
        )
        .ok()
        .filter(|value| !value.is_empty())
    }

    /// `security -i` reads its commands, the key included, from its standard input.
    pub fn store(name: &SecretName, value: &str) -> anyhow::Result<()> {
        let command = format!(
            "add-generic-password -U -a chauffeur -s {} -l \"{}\" -w \"{value}\"\n",
            name.service, name.label
        );
        run(Command::new("security").arg("-i"), Some(&command)).map(|_| ())
    }

    pub fn delete(name: &SecretName) -> anyhow::Result<()> {
        if read(name).is_none() {
            return Ok(());
        }
        run(
            Command::new("security").args(["delete-generic-password", "-s", name.service]),
            None,
        )
        .map(|_| ())
    }
}

#[cfg(not(target_os = "macos"))]
mod system {
    use std::path::PathBuf;
    use std::process::Command;

    use super::{SecretName, read_file, remove_file, run, write_file};

    pub fn read(name: &SecretName) -> Option<String> {
        run(
            Command::new("secret-tool").args(["lookup", "service", name.service]),
            None,
        )
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| read_file(&file(name)?))
    }

    /// `secret-tool` reads the secret from its standard input; without a Secret Service, a file
    /// readable by the user only.
    pub fn store(name: &SecretName, value: &str) -> anyhow::Result<()> {
        let label = format!("--label={}", name.label);
        let stored = run(
            Command::new("secret-tool").args([
                "store",
                &label,
                "service",
                name.service,
                "account",
                "chauffeur",
            ]),
            Some(value),
        );
        if stored.is_ok() {
            return Ok(());
        }
        let path = file(name).ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
        write_file(&path, value)
    }

    pub fn delete(name: &SecretName) -> anyhow::Result<()> {
        let _ = run(
            Command::new("secret-tool").args(["clear", "service", name.service]),
            None,
        );
        match file(name) {
            Some(path) => remove_file(&path),
            None => Ok(()),
        }
    }

    fn file(name: &SecretName) -> Option<PathBuf> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
        Some(config.join("chauffeur").join(name.file))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    const NAME: SecretName<'static> = SecretName {
        service: "chauffeur-test",
        file: "test-api-key",
        label: "Chauffeur: test",
    };

    #[test]
    fn the_users_state_uses_the_system_store_unless_it_is_a_workers() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(".chauffeur");
        std::fs::create_dir_all(&home).unwrap();
        assert_eq!(folder_for(&home, Some(&home)), None);
        let other = dir.path().join("throwaway");
        assert_eq!(folder_for(&other, Some(&home)), Some(other.join("secrets")));
        std::fs::write(home.join("worker"), "GCP worker\n").unwrap();
        assert_eq!(folder_for(&home, Some(&home)), Some(home.join("secrets")));
    }

    fn protected_fixture() -> (tempfile::TempDir, Secrets, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(NAME.file);
        let secrets = Secrets {
            folder: Some(dir.path().to_owned()),
        };
        (dir, secrets, path)
    }

    #[test]
    fn protected_files_distinguish_absence_from_invalid_or_empty_contents() {
        let (_dir, secrets, path) = protected_fixture();
        assert_eq!(secrets.read_protected(&NAME).unwrap(), None);
        secrets.create_protected(&NAME, "synthetic_secret").unwrap();
        assert_eq!(
            secrets.read_protected(&NAME).unwrap().as_deref(),
            Some("synthetic_secret")
        );
        std::fs::write(&path, "  synthetic_secret\n").unwrap();
        assert_eq!(
            secrets.read_protected(&NAME).unwrap().as_deref(),
            Some("synthetic_secret")
        );
        for bytes in [b"   \n".as_slice(), &[0xff]] {
            std::fs::write(&path, bytes).unwrap();
            assert!(secrets.read_protected(&NAME).is_err());
        }
        assert!(secrets.create_protected(&NAME, "replacement").is_err());
    }

    #[test]
    fn protected_files_refuse_links_directories_and_nonprivate_permissions() {
        let (dir, secrets, path) = protected_fixture();
        let target = dir.path().join("target");
        std::fs::write(&target, "synthetic_secret").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&target, &path).unwrap();
        assert!(secrets.read_protected(&NAME).is_err());
        std::fs::remove_file(&target).unwrap();
        assert!(secrets.read_protected(&NAME).is_err());
        assert!(secrets.create_protected(&NAME, "replacement").is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(secrets.read_protected(&NAME).is_err());
        std::fs::remove_dir(&path).unwrap();
        secrets.create_protected(&NAME, "synthetic_secret").unwrap();
        std::fs::hard_link(&path, &target).unwrap();
        assert!(secrets.read_protected(&NAME).is_err());
        std::fs::remove_file(&target).unwrap();
        for mode in [0o644, 0o400, 0o660] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(secrets.read_protected(&NAME).is_err());
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(secrets.read_protected(&NAME).unwrap().is_some());
    }

    #[test]
    fn protected_files_refuse_a_fifo_without_waiting_for_a_writer() {
        use std::os::unix::ffi::OsStrExt;
        let (_dir, secrets, path) = protected_fixture();
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: path is a live NUL-terminated string and the mode is a valid creation mode.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::spawn(move || send.send(secrets.read_protected(&NAME)).unwrap());
        assert!(
            receive
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap()
                .is_err()
        );
    }

    #[test]
    fn protected_files_refuse_contents_larger_than_the_read_limit() {
        let (_dir, secrets, path) = protected_fixture();
        secrets.create_protected(&NAME, "synthetic_secret").unwrap();
        std::fs::write(&path, vec![b'x'; MAX_PROTECTED_FILE as usize]).unwrap();
        assert_eq!(
            secrets.read_protected(&NAME).unwrap().unwrap().len(),
            MAX_PROTECTED_FILE as usize
        );
        std::fs::write(&path, vec![b'x'; MAX_PROTECTED_FILE as usize + 1]).unwrap();
        assert!(secrets.read_protected(&NAME).is_err());
    }

    #[test]
    fn a_test_daemon_keeps_its_secrets_in_its_own_folder() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(
            dir.path().join("home"),
            dir.path().join("workspaces"),
            dir.path().join("repos"),
        );
        let secrets = Secrets::for_paths(&paths);
        assert_eq!(secrets.read(&NAME), None);
        secrets.store(&NAME, "ts_abc-123").unwrap();
        assert_eq!(secrets.read(&NAME).as_deref(), Some("ts_abc-123"));
        let file = dir.path().join("home/secrets/test-api-key");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(secrets.store(&NAME, "with space").is_err());
        assert!(secrets.store(&NAME, "quote\"d").is_err());
        assert_eq!(secrets.read(&NAME).as_deref(), Some("ts_abc-123"));
        secrets.delete(&NAME).unwrap();
        secrets.delete(&NAME).unwrap();
        assert_eq!(secrets.read(&NAME), None);
    }
}

// Separate strict root-key path: existing API-key storage semantics stay unchanged.
mod protected_system {
    use super::SecretName;
    use anyhow::{Result, bail};
    use std::process::{Command, Stdio};

    pub fn read(name: &SecretName) -> Result<Option<String>> {
        #[cfg(target_os = "macos")]
        let mut command = {
            let mut command = Command::new("/usr/bin/security");
            command.args([
                "find-generic-password",
                "-s",
                name.service,
                "-a",
                "chauffeur",
                "-w",
            ]);
            command
        };
        #[cfg(not(target_os = "macos"))]
        let mut command = {
            let mut command = Command::new("secret-tool");
            command.args(["lookup", "service", name.service, "account", "chauffeur"]);
            command
        };
        let output = command
            .stdin(Stdio::null())
            .output()
            .map_err(|_| anyhow::anyhow!("Protected secret store unavailable"))?;
        #[cfg(target_os = "macos")]
        let locked = || anyhow::Ok(false);
        #[cfg(not(target_os = "macos"))]
        let locked = || locked_item_exists(name);
        classify_read(output, locked)
    }

    /// `secret-tool lookup` exits 1 in silence both when nothing matches and when the matching
    /// item's collection stays locked. The Secret Service's `SearchItems` lists locked matches
    /// apart. Not being able to ask is an error, never "missing".
    #[cfg(not(target_os = "macos"))]
    fn locked_item_exists(name: &SecretName) -> Result<bool> {
        use std::collections::HashMap;
        use zbus::zvariant::OwnedObjectPath;
        let attributes = HashMap::from([("service", name.service), ("account", "chauffeur")]);
        let reply = zbus::blocking::connection::Builder::session()
            .and_then(|builder| {
                builder
                    .method_timeout(std::time::Duration::from_secs(5))
                    .build()
            })
            .and_then(|bus| {
                bus.call_method(
                    Some("org.freedesktop.secrets"),
                    "/org/freedesktop/secrets",
                    Some("org.freedesktop.Secret.Service"),
                    "SearchItems",
                    &(attributes,),
                )
            })
            .and_then(|reply| {
                reply
                    .body()
                    .deserialize::<(Vec<OwnedObjectPath>, Vec<OwnedObjectPath>)>()
            });
        Ok(!reply?.1.is_empty())
    }

    fn classify_read(
        output: std::process::Output,
        locked: impl FnOnce() -> Result<bool>,
    ) -> Result<Option<String>> {
        if output.status.success() {
            let value = String::from_utf8(output.stdout)
                .map_err(|_| anyhow::anyhow!("Invalid protected secret"))?;
            if value.trim().is_empty() {
                bail!("Protected secret is empty");
            }
            return Ok(Some(value.trim().to_owned()));
        }
        #[cfg(target_os = "macos")]
        let missing = output.status.code() == Some(44);
        #[cfg(not(target_os = "macos"))]
        let missing = output.status.code() == Some(1) && output.stderr.is_empty();
        if missing {
            match locked() {
                Ok(false) => Ok(None),
                Ok(true) => Err(super::ProtectedStoreLocked::Locked.into()),
                Err(error) => {
                    tracing::warn!(%error, "could not ask the secret service whether a secret is locked");
                    Err(super::ProtectedStoreLocked::Unchecked.into())
                }
            }
        } else {
            bail!("Protected secret store is locked or unavailable")
        }
    }

    pub fn create(name: &SecretName, value: &str) -> Result<()> {
        use std::io::Write;
        #[cfg(target_os = "macos")]
        let (mut command, input) = {
            let mut command = Command::new("/usr/bin/security");
            command.arg("-i");
            // Deliberately no -U: never replace an existing root.
            (
                command,
                format!(
                    "add-generic-password -a chauffeur -s {} -l \"{}\" -w \"{value}\"\n",
                    name.service, name.label
                ),
            )
        };
        #[cfg(not(target_os = "macos"))]
        let (mut command, input) = {
            let mut command = Command::new("secret-tool");
            command.args([
                "store",
                &format!("--label={}", name.label),
                "service",
                name.service,
                "account",
                "chauffeur",
            ]);
            (command, value.to_owned())
        };
        let input = zeroize::Zeroizing::new(input);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| anyhow::anyhow!("Protected secret store unavailable"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(input.as_bytes())
                .map_err(|_| anyhow::anyhow!("Protected secret store write failed"))?;
        }
        if !child.wait()?.success() {
            bail!("Protected secret store write failed");
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::process::ExitStatusExt;
        fn output(code: i32, stdout: &[u8], stderr: &[u8]) -> std::process::Output {
            std::process::Output {
                status: std::process::ExitStatus::from_raw(code << 8),
                stdout: stdout.to_vec(),
                stderr: stderr.to_vec(),
            }
        }
        #[test]
        fn native_absence_is_distinct_from_store_failure() {
            #[cfg(target_os = "macos")]
            let missing = 44;
            #[cfg(not(target_os = "macos"))]
            let missing = 1;
            let absent = || anyhow::Ok(false);
            assert_eq!(
                classify_read(output(missing, b"", b""), absent).unwrap(),
                None
            );
            assert!(classify_read(output(2, b"", b"backend unavailable"), absent).is_err());
            assert!(classify_read(output(0, b"", b""), absent).is_err());
            assert!(classify_read(output(0, &[0xff], b""), absent).is_err());
            assert_eq!(
                classify_read(output(0, b"test_secret\n", b""), absent).unwrap(),
                Some("test_secret".into())
            );
        }
        // `secret-tool` misses silently with exit 1; the Keychain's 44 never hides a locked item.
        #[cfg(not(target_os = "macos"))]
        #[test]
        fn a_silent_miss_on_a_locked_item_asks_to_unlock_rather_than_reporting_absence() {
            let error = classify_read(output(1, b"", b""), || Ok(true)).unwrap_err();
            assert!(error.is::<super::super::ProtectedStoreLocked>());
            assert!(matches!(
                super::super::protected_failure(|| crate::error::Error::Unavailable("x".into()))(
                    error
                ),
                crate::error::Error::Locked(_)
            ));
            assert!(
                classify_read(output(2, b"", b"backend unavailable"), || Ok(true))
                    .unwrap_err()
                    .downcast_ref::<super::super::ProtectedStoreLocked>()
                    .is_none()
            );
        }
        #[cfg(not(target_os = "macos"))]
        #[test]
        fn a_silent_miss_the_secret_service_cannot_be_asked_about_blocks_setup_rather_than_reporting_absence()
         {
            let no_bus = || Err(anyhow::anyhow!("no session bus"));
            let error = classify_read(output(1, b"", b""), no_bus).unwrap_err();
            assert!(matches!(
                error.downcast_ref::<super::super::ProtectedStoreLocked>(),
                Some(super::super::ProtectedStoreLocked::Unchecked)
            ));
            let crate::error::Error::Locked(message) =
                super::super::protected_failure(|| crate::error::Error::Unavailable("x".into()))(
                    error,
                )
            else {
                panic!("an unchecked keyring must stop setup with its own message");
            };
            assert!(message.starts_with("Could not check whether the system keyring"));
        }
    }
}
