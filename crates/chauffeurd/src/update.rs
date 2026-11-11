//! Updates of a packaged Chauffeur.app (scripts/package.sh) from the GitHub releases of the
//! build's `CHAUFFEUR_RELEASE_REPO`, a repository with separate Core and Dialog releases, read through the user's own `gh` login. The daemon looks at
//! start, every 10 minutes, and when asked (`update.check`: the app's window regains the focus,
//! `chauffeur update`); `update.install` downloads the disk image, checks it against the SHA-256
//! published beside it, and puts its app in place of the one this daemon runs from, whatever the
//! agents do meanwhile. The app then quits and reopens, and the new app replaces this daemon: the
//! app asks before that if agents work.
//!
//! A daemon run from anything but an app bundle (a build from source, scripts/install.sh) never
//! looks, nor does a build without a release repository. No `gh` or no network only leaves a line
//! in the log.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, bail, ensure};
use chauffeur_protocol::model::{UpdatePhase, UpdateStatus};
use sha2::{Digest, Sha256};

use crate::{Daemon, Error, Result};

/// Where this build's disk images and Linux bundles are published (`owner/repo` on GitHub):
/// `CHAUFFEUR_RELEASE_REPO` in the daemon's environment, else as it was when the daemon was built.
/// Neither, the build looks for no update and a GCP worker needs `[workers] bundle`.
pub fn release_repo() -> Option<String> {
    std::env::var("CHAUFFEUR_RELEASE_REPO")
        .ok()
        .or_else(|| option_env!("CHAUFFEUR_RELEASE_REPO").map(str::to_string))
        .map(|repo| repo.trim().to_string())
        .filter(|repo| !repo.is_empty())
}
/// The environment `gh` runs in: the login shell's.
type Env = std::collections::BTreeMap<String, String>;
/// A release shows within this time; one `gh release view` each time is far within GitHub's
/// limits.
const CHECK_EVERY: Duration = Duration::from_secs(10 * 60);

/// Overrides the app bundle the daemon runs from, so that tests and the e2e scripts exercise
/// updates with a development daemon and a throwaway folder in place of /Applications.
const BUNDLE_OVERRIDE: &str = "CHAUFFEUR_APP_BUNDLE";

pub struct Updates {
    /// The app bundle this daemon belongs to, replaced by an update; `None` never updates.
    bundle: Option<PathBuf>,
    status: Mutex<UpdateStatus>,
}

impl Updates {
    pub fn new() -> Updates {
        let bundle = match std::env::var_os(BUNDLE_OVERRIDE).filter(|path| !path.is_empty()) {
            Some(path) => Some(PathBuf::from(path)),
            None => std::env::current_exe()
                .ok()
                .and_then(|exe| exe.canonicalize().ok())
                .and_then(|exe| bundle_of(&exe)),
        };
        Updates {
            bundle,
            status: Mutex::new(UpdateStatus {
                current: env!("CARGO_PKG_VERSION").to_string(),
                releases_url: release_repo()
                    .map(|repo| format!("https://github.com/{repo}/releases")),
                ..Default::default()
            }),
        }
    }

    pub fn status(&self) -> UpdateStatus {
        self.status.lock().unwrap().clone()
    }

    fn set(&self, change: impl FnOnce(&mut UpdateStatus)) {
        change(&mut self.status.lock().unwrap());
    }

    /// `update.check`: looks now.
    pub async fn check_now(&self, daemon: &Daemon) -> UpdateStatus {
        self.check(daemon).await;
        self.status()
    }

    /// Asks GitHub for the latest release; keeps the last answer when it cannot.
    pub async fn check(&self, daemon: &Daemon) {
        let (Some(_), Some(repo)) = (&self.bundle, release_repo()) else {
            return;
        };
        let env = daemon.shell_env.vars();
        let latest = tokio::task::spawn_blocking(move || latest_release(&env, &repo)).await;
        let latest = match latest {
            Ok(Ok(latest)) => latest,
            Ok(Err(error)) => {
                tracing::warn!(
                    error = format!("{error:#}"),
                    "could not look for a newer chauffeur"
                );
                return;
            }
            Err(error) => {
                tracing::warn!(%error, "could not look for a newer chauffeur");
                return;
            }
        };
        self.set(|status| {
            // The installed version stays the one shown until the app restarts on it.
            if status.phase == UpdatePhase::Installed {
                return;
            }
            let newer = latest.filter(|version| newer_than(version, &status.current));
            if newer != status.available {
                tracing::info!(available = ?newer, "chauffeur update");
            }
            status.available = newer;
        });
    }

    /// Starts installing the newer version in the background.
    pub fn install(&self, daemon: &Arc<Daemon>) -> Result<UpdateStatus> {
        let Some(bundle) = self.bundle.clone() else {
            return Err(Error::Conflict(
                "This Chauffeur was built from source: update it with git pull and scripts/install.sh."
                    .into(),
            ));
        };
        let version = {
            let mut status = self.status.lock().unwrap();
            if matches!(
                status.phase,
                UpdatePhase::Downloading | UpdatePhase::Installing
            ) {
                return Err(Error::Conflict(
                    "Chauffeur is being updated already.".into(),
                ));
            }
            let Some(version) = status.available.clone() else {
                return Err(Error::Conflict("No newer Chauffeur is published.".into()));
            };
            status.phase = UpdatePhase::Downloading;
            status.error = None;
            version
        };
        let Some(repo) = release_repo() else {
            return Err(Error::Conflict(
                "This Chauffeur was built without a release repository: it has no updates.".into(),
            ));
        };
        let env = daemon.shell_env.vars();
        let dir = daemon.paths.home.join("updates");
        let daemon = daemon.clone();
        tokio::task::spawn_blocking(move || {
            let updates = &daemon.updates;
            let installed = download(&env, &repo, &version, &dir).and_then(|dmg| {
                updates.set(|status| status.phase = UpdatePhase::Installing);
                install_image(&dmg, &dir.join("mnt"), &bundle)
            });
            let _ = std::fs::remove_dir_all(&dir);
            match installed {
                Ok(()) => {
                    tracing::info!(%version, bundle = %bundle.display(), "chauffeur updated");
                    updates.set(|status| status.phase = UpdatePhase::Installed);
                }
                Err(error) => {
                    let error = format!("{error:#}");
                    tracing::warn!(%error, %version, "could not update chauffeur");
                    updates.set(|status| {
                        status.phase = UpdatePhase::Failed;
                        status.error = Some(error);
                    });
                }
            }
        });
        Ok(self.status())
    }
}

impl Default for Updates {
    fn default() -> Self {
        Updates::new()
    }
}

/// Looks for a newer version at start, then every 10 minutes.
pub fn spawn(daemon: Arc<Daemon>) {
    if daemon.updates.bundle.is_none() || release_repo().is_none() {
        return;
    }
    tokio::spawn(async move {
        loop {
            daemon.updates.check(&daemon).await;
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// `X.app` for `X.app/Contents/MacOS/chauffeurd`, when the bundle carries the app beside it.
fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let bundle = macos.parent()?.parent()?;
    (macos.ends_with("Contents/MacOS")
        && bundle.extension().is_some_and(|ext| ext == "app")
        && macos.join("chauffeur-app").is_file())
    .then(|| bundle.to_path_buf())
}

/// A build cannot switch edition through its runtime environment.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Channel {
    Core,
    Dialog,
}

impl Channel {
    fn current() -> Self {
        match option_env!("CHAUFFEUR_RELEASE_CHANNEL") {
            Some("core") => Self::Core,
            _ => Self::Dialog,
        }
    }
    fn prefix(self) -> &'static str {
        match self {
            Self::Core => "core-v",
            Self::Dialog => "v",
        }
    }
    fn dmg(self, version: &str) -> String {
        match self {
            Self::Core => format!("Chauffeur-Core-{version}.dmg"),
            Self::Dialog => format!("Chauffeur-{version}.dmg"),
        }
    }
    fn worker_bundle(self, version: &str) -> String {
        match self {
            Self::Core => format!("chauffeur-core-{version}-linux-x64.tar.gz"),
            Self::Dialog => format!("chauffeur-{version}-linux-x64.tar.gz"),
        }
    }
}

pub(crate) fn release_tag(version: &str) -> String {
    format!("{}{version}", Channel::current().prefix())
}

pub(crate) fn worker_bundle_name(version: &str) -> String {
    Channel::current().worker_bundle(version)
}

fn dmg_name(version: &str) -> String {
    Channel::current().dmg(version)
}

fn gh(env: &Env) -> Command {
    let mut command = Command::new("gh");
    command.env_clear().envs(env).stdin(Stdio::null());
    command
}

/// Runs a command; its error is the last line it printed.
fn run(command: &mut Command, what: &str) -> anyhow::Result<Vec<u8>> {
    let output = command
        .output()
        .with_context(|| format!("could not run {what}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "{what} failed: {}",
            stderr.trim().lines().last().unwrap_or("no output")
        );
    }
    Ok(output.stdout)
}

/// The latest release's version when it carries the disk image and its checksum (a release whose
/// files are still uploading has none yet); `None` otherwise.
fn latest_release(env: &Env, repo: &str) -> anyhow::Result<Option<String>> {
    if Channel::current() == Channel::Core {
        let stdout = run(
            gh(env).args([
                "api",
                "--paginate",
                "--slurp",
                &format!("repos/{repo}/releases?per_page=100"),
            ]),
            "gh api releases",
        )?;
        return core_release_version(&stdout);
    }
    let stdout = run(
        gh(env).args([
            "release",
            "view",
            "--repo",
            repo,
            "--json",
            "tagName,assets",
        ]),
        "gh release view",
    )?;
    Ok(published_version(&stdout))
}

fn core_release_version(json: &[u8]) -> anyhow::Result<Option<String>> {
    let pages: Vec<Vec<serde_json::Value>> = serde_json::from_slice(json)?;
    Ok(pages
        .into_iter()
        .flatten()
        .filter(|release| release["draft"] == false && release["prerelease"] == false)
        .filter_map(|release| {
            published_version_for(&serde_json::to_vec(&release).ok()?, Channel::Core)
        })
        .max_by_key(|version| parse_version(version)))
}

fn published_version(json: &[u8]) -> Option<String> {
    published_version_for(json, Channel::current())
}

fn published_version_for(json: &[u8], channel: Channel) -> Option<String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Release {
        #[serde(alias = "tag_name")]
        tag_name: String,
        #[serde(default)]
        assets: Vec<Asset>,
    }
    #[derive(serde::Deserialize)]
    struct Asset {
        name: String,
    }
    let release: Release = serde_json::from_slice(json).ok()?;
    let version = release.tag_name.strip_prefix(channel.prefix())?;
    parse_version(version)?;
    let dmg = channel.dmg(version);
    let has = |name: &str| release.assets.iter().any(|asset| asset.name == name);
    (has(&dmg) && has(&format!("{dmg}.sha256"))).then(|| version.to_string())
}

/// `X.Y.Z`, nothing else: a pre-release or a build suffix is never offered.
fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.').map(|part| part.parse::<u64>().ok());
    let parsed = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(parsed)
}

fn newer_than(version: &str, current: &str) -> bool {
    match (parse_version(version), parse_version(current)) {
        (Some(version), Some(current)) => version > current,
        _ => false,
    }
}

/// Downloads the disk image and its checksum into `dir` and checks one against the other.
fn download(env: &Env, repo: &str, version: &str, dir: &Path) -> anyhow::Result<PathBuf> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let dmg = dmg_name(version);
    let checksum = format!("{dmg}.sha256");
    run(
        gh(env)
            .args(["release", "download", &release_tag(version), "--repo"])
            .args([repo, "--pattern", &dmg, "--pattern", &checksum])
            .arg("--dir")
            .arg(dir)
            .arg("--clobber"),
        "gh release download",
    )?;
    let path = dir.join(&dmg);
    let published = std::fs::read_to_string(dir.join(&checksum))
        .with_context(|| format!("reading {checksum}"))?;
    let published = published.split_whitespace().next().unwrap_or_default();
    let actual = sha256_of(&path)?;
    ensure!(
        published.eq_ignore_ascii_case(&actual),
        "{dmg} does not match its published SHA-256; download it from the release page instead"
    );
    Ok(path)
}

fn sha256_of(path: &Path) -> anyhow::Result<String> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Detaches the disk image however the installation ends.
struct Mounted<'a>(&'a Path);

impl Drop for Mounted<'_> {
    fn drop(&mut self) {
        let detached = Command::new("/usr/bin/hdiutil")
            .args(["detach", "-quiet", "-force"])
            .arg(self.0)
            .status();
        if !detached.is_ok_and(|status| status.success()) {
            tracing::warn!(mount = %self.0.display(), "could not detach the update's disk image");
        }
    }
}

/// Mounts the disk image at `mount` and puts its Chauffeur.app in place of `bundle`. The new app
/// is copied beside the old one first, so that a failure leaves the old one as it was.
fn install_image(dmg: &Path, mount: &Path, bundle: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(mount)?;
    run(
        Command::new("/usr/bin/hdiutil")
            .args([
                "attach",
                "-nobrowse",
                "-readonly",
                "-noautoopen",
                "-mountpoint",
            ])
            .arg(mount)
            .arg(dmg),
        "hdiutil attach",
    )?;
    let _mounted = Mounted(mount);
    let source = mount.join("Chauffeur.app");
    ensure!(
        source.join("Contents/MacOS/chauffeurd").is_file(),
        "the disk image holds no Chauffeur.app"
    );
    let parent = bundle
        .parent()
        .context("the app bundle has no parent folder")?;
    let name = bundle
        .file_name()
        .context("the app bundle has no name")?
        .to_string_lossy();
    let staged = parent.join(format!(".{name}.update"));
    let old = parent.join(format!(".{name}.old"));
    let _ = std::fs::remove_dir_all(&staged);
    run(
        Command::new("/usr/bin/ditto").arg(&source).arg(&staged),
        &format!("copying the new app into {}", parent.display()),
    )?;
    // Files gh downloads carry no quarantine, but one copied out of an image may: macOS would
    // block the daemon, the CLI and the sidecar when started from a terminal.
    let _ = Command::new("/usr/bin/xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(&staged)
        .status();
    let _ = std::fs::remove_dir_all(&old);
    let had_old = bundle.exists();
    if had_old {
        std::fs::rename(bundle, &old)
            .with_context(|| format!("moving {} aside", bundle.display()))?;
    }
    if let Err(error) = std::fs::rename(&staged, bundle) {
        if had_old {
            let _ = std::fs::rename(&old, bundle);
        }
        let _ = std::fs::remove_dir_all(&staged);
        return Err(error).with_context(|| format!("putting the new app at {}", bundle.display()));
    }
    // The running app and daemon keep their open files: removing the old bundle is safe.
    if let Err(error) = std::fs::remove_dir_all(&old)
        && had_old
    {
        tracing::warn!(%error, path = %old.display(), "could not remove the previous app");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, assets: &[&str]) -> Vec<u8> {
        let assets: Vec<_> = assets
            .iter()
            .map(|name| serde_json::json!({ "name": name, "size": 1 }))
            .collect();
        serde_json::to_vec(&serde_json::json!({ "tagName": tag, "assets": assets })).unwrap()
    }

    #[test]
    fn core_updates_ignore_dialog_extensions_drafts_and_incomplete_downloads() {
        let release = |tag: &str, name: &str, draft: bool| {
            serde_json::json!({
                "tag_name": tag, "draft": draft, "prerelease": false,
                "assets": [{"name": name}, {"name": format!("{name}.sha256")}]
            })
        };
        let pages = serde_json::json!([
            [
                release("v0.99.0", "Chauffeur-0.99.0.dmg", false),
                release("office-v0.99.0", "Chauffeur-Core-0.99.0.dmg", false),
                release("core-v0.99.0", "Chauffeur-Core-0.99.0.dmg", true),
                release("core-v0.98.0", "Chauffeur-0.98.0.dmg", false)
            ],
            [release("core-v0.37.0", "Chauffeur-Core-0.37.0.dmg", false)]
        ]);
        assert_eq!(
            core_release_version(&serde_json::to_vec(&pages).unwrap())
                .unwrap()
                .as_deref(),
            Some("0.37.0")
        );
        let core = serde_json::to_vec(&release("core-v0.37.0", "Chauffeur-Core-0.37.0.dmg", false))
            .unwrap();
        assert_eq!(published_version_for(&core, Channel::Dialog), None);
        assert_eq!(
            Channel::Core.worker_bundle("0.37.0"),
            "chauffeur-core-0.37.0-linux-x64.tar.gz"
        );
        assert_eq!(
            Channel::Dialog.worker_bundle("0.37.0"),
            "chauffeur-0.37.0-linux-x64.tar.gz"
        );
    }

    #[test]
    fn a_release_is_offered_only_with_its_disk_image_and_checksum() {
        let both = ["Chauffeur-0.10.0.dmg", "Chauffeur-0.10.0.dmg.sha256"];
        assert_eq!(
            published_version(&release("v0.10.0", &both)).as_deref(),
            Some("0.10.0")
        );
        assert_eq!(
            published_version(&release("v0.10.0", &["Chauffeur-0.10.0.dmg"])),
            None
        );
        assert_eq!(published_version(&release("v0.10.0-rc.1", &both)), None);
        assert_eq!(published_version(&release("0.10.0", &both)), None);
        assert_eq!(published_version(b"not json"), None);
    }

    #[test]
    fn versions_compare_by_number_not_as_text() {
        assert!(newer_than("0.10.0", "0.9.0"));
        assert!(newer_than("1.0.0", "0.99.99"));
        assert!(!newer_than("0.9.0", "0.9.0"));
        assert!(!newer_than("0.8.9", "0.9.0"));
        assert!(!newer_than("0.10", "0.9.0"));
    }

    #[test]
    fn only_a_daemon_inside_an_app_bundle_updates() {
        let root = tempfile::tempdir().unwrap();
        let macos = root.path().join("Chauffeur.app/Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::write(macos.join("chauffeurd"), "").unwrap();
        assert_eq!(
            bundle_of(&macos.join("chauffeurd")),
            None,
            "no app beside it"
        );
        std::fs::write(macos.join("chauffeur-app"), "").unwrap();
        assert_eq!(
            bundle_of(&macos.join("chauffeurd")),
            Some(root.path().join("Chauffeur.app"))
        );
        let target = root.path().join("target/debug");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("chauffeur-app"), "").unwrap();
        assert_eq!(bundle_of(&target.join("chauffeurd")), None);
    }

    /// A `gh` that "downloads" the files of `assets` into the folder it is given.
    fn fake_gh(root: &Path, assets: &Path) -> Env {
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let script = format!(
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do [ \"$1\" = --dir ] && dir=$2; shift; done\n\
             cp {}/* \"$dir\"/\n",
            assets.display()
        );
        let gh = bin.join("gh");
        std::fs::write(&gh, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        Env::from([(
            "PATH".to_string(),
            format!("{}:/usr/bin:/bin", bin.display()),
        )])
    }

    #[test]
    fn a_disk_image_that_does_not_match_its_checksum_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let assets = root.path().join("assets");
        std::fs::create_dir_all(&assets).unwrap();
        std::fs::write(assets.join("Chauffeur-0.10.0.dmg"), "tampered").unwrap();
        std::fs::write(
            assets.join("Chauffeur-0.10.0.dmg.sha256"),
            format!("{}  Chauffeur-0.10.0.dmg\n", "0".repeat(64)),
        )
        .unwrap();
        let env = fake_gh(root.path(), &assets);
        let error = download(
            &env,
            "acme/chauffeur",
            "0.10.0",
            &root.path().join("updates"),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("does not match its published SHA-256"));

        let good = sha256_of(&assets.join("Chauffeur-0.10.0.dmg")).unwrap();
        std::fs::write(
            assets.join("Chauffeur-0.10.0.dmg.sha256"),
            format!("{good}  Chauffeur-0.10.0.dmg\n"),
        )
        .unwrap();
        let dmg = download(
            &env,
            "acme/chauffeur",
            "0.10.0",
            &root.path().join("updates"),
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(dmg).unwrap(), "tampered");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_app_of_the_disk_image_replaces_the_installed_one() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let stage = root.join("stage");
        let macos = stage.join("Chauffeur.app/Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::write(macos.join("chauffeurd"), "new daemon").unwrap();
        std::os::unix::fs::symlink("/Applications", stage.join("Applications")).unwrap();
        let dmg = root.join("Chauffeur-0.10.0.dmg");
        run(
            Command::new("/usr/bin/hdiutil")
                .args(["create", "-quiet", "-format", "UDZO", "-srcfolder"])
                .arg(&stage)
                .arg(&dmg),
            "hdiutil create",
        )
        .unwrap();

        let applications = root.join("Applications");
        let installed = applications.join("Chauffeur.app/Contents/MacOS");
        std::fs::create_dir_all(&installed).unwrap();
        std::fs::write(installed.join("chauffeurd"), "old daemon").unwrap();
        std::fs::write(installed.join("leftover"), "").unwrap();
        let mount = root.join("mnt");
        install_image(&dmg, &mount, &applications.join("Chauffeur.app")).unwrap();

        assert_eq!(
            std::fs::read_to_string(installed.join("chauffeurd")).unwrap(),
            "new daemon"
        );
        assert!(!installed.join("leftover").exists());
        let mut left: Vec<_> = std::fs::read_dir(&applications)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        left.sort();
        assert_eq!(left, ["Chauffeur.app"], "no staged or old copy is left");
        assert!(
            !mount.join("Chauffeur.app").exists(),
            "the disk image is detached"
        );
    }
}
