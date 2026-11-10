//! A local controller with its own protected identity and journal, owned by the Mac's daemon.
//! It starts on first transfer and comes back with the daemon; an explicit controller stays external.
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use chauffeur_protocol::client::{ClientCall, EmptyParams};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use crate::Daemon;
use crate::cloud_transfer_driver::Peer;

const MARKER: &str = "managed-by-chauffeur";
const INITIAL_SETTINGS: &str =
    "github_org = \"\"\ngithub_sources = false\nlinear_enabled = false\n[office]\nurl = \"\"\n";

#[derive(Default)]
pub struct TransferController {
    child: Mutex<Option<Child>>,
}

fn home(daemon: &Daemon) -> PathBuf {
    daemon.paths.home.join("transfer-controller")
}

fn configured(daemon: &Daemon, asked: Option<&str>) -> Option<PathBuf> {
    let saved = daemon.settings.get();
    let path = asked.unwrap_or(&saved.transfer.controller).trim();
    (!path.is_empty()).then(|| crate::settings::expand_home(path))
}

fn owned(path: &Path) -> bool {
    let directory = std::fs::symlink_metadata(path).ok();
    let marker = std::fs::symlink_metadata(path.join(MARKER)).ok();
    directory.is_some_and(|m| m.is_dir() && !m.file_type().is_symlink())
        && marker.is_some_and(|m| m.is_file() && !m.file_type().is_symlink())
        && std::fs::read(path.join(MARKER)).is_ok_and(|bytes| bytes == b"transfer controller v1\n")
}

async fn connect(path: &Path) -> Option<Peer> {
    tokio::time::timeout(Duration::from_secs(2), Peer::connect(path))
        .await
        .ok()?
        .ok()
}

/// Never adopts an arbitrary pre-existing folder or follows a link into the user's other data.
fn prepare(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            anyhow::ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "The automatic transfer controller's folder {} is not a directory; move it aside and retry",
                path.display()
            );
            anyhow::ensure!(
                owned(path),
                "{} is not a controller Chauffeur created; move it aside and retry",
                path.display()
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let pending = tempfile::Builder::new()
                .prefix(".transfer-controller-")
                .tempdir_in(
                    path.parent()
                        .context("the controller has a parent state folder")?,
                )?;
            crate::paths::private_dir(pending.path())?;
            for (name, contents) in [
                (MARKER, "transfer controller v1\n"),
                ("settings.toml", INITIAL_SETTINGS),
            ] {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(pending.path().join(name))?;
                std::io::Write::write_all(&mut file, contents.as_bytes())?;
            }
            std::fs::rename(pending.path(), path)?;
        }
        Err(error) => return Err(error.into()),
    }
    crate::paths::private_dir(path)?;
    let settings = path.join("settings.toml");
    // Only the initial configuration is ours to seed; journal and identity are never reset.
    if !settings.exists() {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(settings)?;
        std::io::Write::write_all(&mut file, INITIAL_SETTINGS.as_bytes())?;
    }
    Ok(())
}

impl TransferController {
    /// Serializes startup, reuses a surviving owned controller and restarts a stopped or older one.
    pub(crate) async fn socket(
        &self,
        daemon: &Daemon,
        asked: Option<&str>,
    ) -> anyhow::Result<PathBuf> {
        if let Some(path) = configured(daemon, asked) {
            return Ok(path);
        }
        let mut child = self.child.lock().await;
        anyhow::ensure!(
            !daemon.shutdown.is_cancelled(),
            "Chauffeur is stopping; reopen it before transferring"
        );
        let dir = home(daemon);
        prepare(&dir)?;
        let paths =
            crate::paths::Paths::new(dir.clone(), dir.join("workspaces"), dir.join("repos"));
        if let Some(mut peer) = connect(&paths.socket).await {
            if peer.version == env!("CARGO_PKG_VERSION") {
                return Ok(paths.socket);
            }
            tokio::time::timeout(
                Duration::from_secs(5),
                peer.call_value(ClientCall::DaemonShutdown(EmptyParams {})),
            )
            .await
            .context("the previous automatic controller did not stop")?
            .context("stopping the previous automatic controller version")?;
            for _ in 0..100 {
                if !paths.socket.exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        if let Some(process) = child.as_mut() {
            // It may be exiting after shutdown: wait before starting another process on its lock.
            if process.try_wait()?.is_none() {
                process
                    .kill()
                    .await
                    .context("stopping an unresponsive automatic controller")?;
            }
        }
        *child = None;
        paths.ensure()?;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(paths.logs_dir.join("controller.log"))?;
        crate::paths::private_file(&paths.logs_dir.join("controller.log"))?;
        let exe = std::env::current_exe()?;
        anyhow::ensure!(
            exe.file_name().is_some_and(|name| name == "chauffeurd"),
            "The automatic controller needs the running chauffeurd executable; start the daemon before transferring"
        );
        *child = Some(
            Command::new(exe)
                .arg("--transfer-controller")
                .current_dir(&paths.home)
                .env_clear()
                .envs(daemon.shell_env.vars())
                .env("CHAUFFEUR_HOME", &paths.home)
                .env("CHAUFFEUR_WORKSPACES_DIR", &paths.workspaces_dir)
                .env("CHAUFFEUR_REPOS_DIR", &paths.repos_dir)
                .env("CHAUFFEUR_PORT_OVERRIDE", "0")
                .stdin(Stdio::null())
                .stdout(log.try_clone()?)
                .stderr(log)
                .kill_on_drop(true)
                .spawn()
                .context("starting the automatic transfer controller")?,
        );
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while tokio::time::Instant::now() < deadline {
            if let Some(process) = child.as_mut()
                && let Some(status) = process.try_wait()?
            {
                bail!(
                    "The automatic transfer controller stopped ({status}); see {} and retry",
                    paths.logs_dir.join("controller.log").display()
                );
            }
            if let Some(peer) = connect(&paths.socket).await {
                anyhow::ensure!(
                    peer.version == env!("CARGO_PKG_VERSION"),
                    "The automatic transfer controller runs another version; restart Chauffeur and retry"
                );
                return Ok(paths.socket);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        bail!(
            "The automatic transfer controller did not start; see {} and retry",
            paths.logs_dir.join("controller.log").display()
        )
    }

    pub async fn shutdown(&self, daemon: &Daemon) {
        // This namespace is exclusively ours; never stop the controller explicitly configured by a user.
        let dir = home(daemon);
        if !owned(&dir) {
            return;
        }
        let mut child = self.child.lock().await;
        if let Some(mut peer) = connect(&dir.join("chauffeurd.sock")).await {
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                peer.call_value(ClientCall::DaemonShutdown(EmptyParams {})),
            )
            .await;
        }
        if let Some(mut process) = child.take()
            && tokio::time::timeout(Duration::from_secs(5), process.wait())
                .await
                .is_err()
        {
            let _ = process.kill().await;
        }
    }
}

pub fn spawn(daemon: Arc<Daemon>) {
    tokio::spawn(async move {
        loop {
            if daemon.shutdown.is_cancelled() {
                break;
            }
            if home(&daemon).join(MARKER).exists()
                && configured(&daemon, None).is_none()
                && let Err(error) = daemon.transfer_controller.socket(&daemon, None).await
            {
                tracing::warn!(%error, "could not restart the automatic transfer controller");
            }
            tokio::select! {
                _ = daemon.shutdown.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn preparation_keeps_the_existing_journal_and_initializes_private_offline_settings() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("controller");
        prepare(&dir).unwrap();
        let settings = std::fs::read_to_string(dir.join("settings.toml")).unwrap();
        let parsed = settings.parse::<toml_edit::DocumentMut>().unwrap();
        assert_eq!(parsed["linear_enabled"].as_bool(), Some(false));
        assert_eq!(parsed["office"]["url"].as_str(), Some(""));
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(dir.join(MARKER))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        std::fs::write(dir.join("journal"), b"committed ownership").unwrap();
        prepare(&dir).unwrap();
        assert_eq!(
            std::fs::read(dir.join("journal")).unwrap(),
            b"committed ownership"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("settings.toml")).unwrap(),
            settings
        );
    }

    #[test]
    fn an_existing_unowned_folder_is_never_overwritten_or_adopted() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("controller");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("settings.toml"), b"user settings").unwrap();
        assert!(prepare(&dir).is_err());
        assert!(!owned(&dir));
        assert_eq!(
            std::fs::read(dir.join("settings.toml")).unwrap(),
            b"user settings"
        );
        assert!(!dir.join(MARKER).exists());
    }

    #[test]
    fn links_cannot_redirect_creation_or_prove_ownership_of_another_daemon() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real");
        prepare(&real).unwrap();
        let link = temp.path().join("controller");
        symlink(&real, &link).unwrap();
        assert!(prepare(&link).is_err());
        assert!(!owned(&link));
        std::fs::remove_file(real.join(MARKER)).unwrap();
        std::fs::write(temp.path().join("marker"), b"transfer controller v1\n").unwrap();
        symlink(temp.path().join("marker"), real.join(MARKER)).unwrap();
        assert!(prepare(&real).is_err());
        assert!(!owned(&real));
    }

    #[tokio::test]
    async fn explicit_and_saved_controllers_never_create_a_local_one() {
        let temp = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::new(
            temp.path().join("home"),
            temp.path().join("ws"),
            temp.path().join("repos"),
        );
        paths.ensure().unwrap();
        std::fs::write(&paths.settings,"linear_enabled = false\n[transfer]\ncontroller = \"/tmp/saved-controller.sock\"\n[office]\nurl = \"\"\n").unwrap();
        let daemon = Daemon::open(paths).unwrap();
        assert_eq!(
            daemon
                .transfer_controller
                .socket(&daemon, None)
                .await
                .unwrap(),
            PathBuf::from("/tmp/saved-controller.sock")
        );
        assert_eq!(
            daemon
                .transfer_controller
                .socket(&daemon, Some("/tmp/explicit-controller.sock"))
                .await
                .unwrap(),
            PathBuf::from("/tmp/explicit-controller.sock")
        );
        daemon.transfer_controller.shutdown(&daemon).await;
        assert!(!home(&daemon).exists());
    }
}
