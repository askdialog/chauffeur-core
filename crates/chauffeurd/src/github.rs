//! GitHub without Homebrew (docs/adr/0051-github-without-homebrew.md). Chauffeur reaches GitHub
//! through `gh`: the user's own when one is on the PATH, else a copy Chauffeur installs (pinned,
//! checked against its SHA-256) in the state folder, last on every child's PATH so that agents
//! find it too. Connecting signs `gh` in through GitHub's device flow in the browser and lets git
//! push over HTTPS through it, so no SSH key is needed. The token stays in `gh`'s keyring:
//! Chauffeur never reads it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, Weak};
use std::time::{Duration, Instant};

use anyhow::Context;
use chauffeur_protocol::client::{GithubConnecting, GithubStatus};
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::{Daemon, Error, Result};

/// The `gh` Chauffeur installs: an upgrade changes it and the checksums of `release_for` together.
pub const VERSION: &str = "2.98.0";
const RELEASES: &str = "https://github.com/cli/cli/releases/download";
const HOST: &str = "github.com";
/// How long the account read from `gh` stays good: reading it asks GitHub.
const ACCOUNT_TTL: Duration = Duration::from_secs(60);
/// What a feature that needs `gh` says when there is none.
pub const NOT_CONNECTED: &str =
    "GitHub is not connected: connect it in Settings › GitHub, or with `chauffeur github login`";
/// The key under which `WorkspaceManager::github_login` keeps the login it read.
const LOGIN_KEY: &str = "gh_login";

#[derive(Clone, Debug)]
pub struct Release {
    /// `.zip` on macOS, `.tar.gz` on Linux, as GitHub CLI publishes them.
    pub name: String,
    pub sha256: String,
    pub size: u64,
}

/// The archive for this system; none where Chauffeur does not install `gh`.
fn release_for(os: &str, arch: &str) -> Option<Release> {
    let (platform, sha256, size) = match (os, arch) {
        ("macos", "aarch64") => (
            "macOS_arm64.zip",
            "8cfb027cc5310675f2b830eac8f9865c1155a45ffcf9757f699fdd5a22046ca4",
            13_938_611,
        ),
        ("macos", "x86_64") => (
            "macOS_amd64.zip",
            "734c7bbd0bc56a3974500ee9aea74d60f0e5b89be09e92b9d9148939a3a1e0e6",
            15_521_035,
        ),
        ("linux", "x86_64") => (
            "linux_amd64.tar.gz",
            "3b8ac6b30336802fc1a858d7c084e11cdf24ac1a761ca90b68022d7d729208de",
            14_863_663,
        ),
        ("linux", "aarch64") => (
            "linux_arm64.tar.gz",
            "cf689084f3a3618f7eae4a2420d335d74626d65f5e594b9828d125d69f800d86",
            13_523_338,
        ),
        _ => return None,
    };
    Some(Release {
        name: format!("gh_{VERSION}_{platform}"),
        sha256: sha256.into(),
        size,
    })
}

/// The folder of Chauffeur's own tools, put last on every child's PATH once it exists.
pub fn tools_dir(home: &Path) -> PathBuf {
    home.join("bin")
}

/// The command that installs git by hand where Chauffeur cannot open an installer.
fn git_install_command() -> &'static str {
    if cfg!(target_os = "macos") {
        return "xcode";
    }
    [
        ("/usr/bin/apt-get", "sudo apt-get install git"),
        ("/usr/bin/pacman", "sudo pacman -S git"),
        ("/usr/bin/dnf", "sudo dnf install git"),
    ]
    .into_iter()
    .find(|(manager, _)| Path::new(manager).is_file())
    .map_or("your system's package manager", |(_, command)| command)
}

/// Opens Apple's installer of the command line developer tools, which bring git; the user
/// follows it in its own window.
pub fn install_git() -> Result<()> {
    if !cfg!(target_os = "macos") {
        return Err(Error::BadRequest(format!(
            "Chauffeur cannot install git here: install it with {}",
            git_install_command()
        )));
    }
    // It answers at once, the installer showing in its own window; when the tools are already
    // there it says so and `github.status` already finds git.
    std::process::Command::new("/usr/bin/xcode-select")
        .arg("--install")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("opening Apple's installer of the command line developer tools")?;
    Ok(())
}

#[derive(Clone, Debug, Default)]
struct Account {
    login: Option<String>,
    organizations: Vec<String>,
}

pub struct Github {
    daemon: Weak<Daemon>,
    http: reqwest::Client,
    /// Where releases are downloaded from, and the archive to take; tests serve their own.
    releases: Mutex<(String, Option<Release>)>,
    connecting: Mutex<Option<GithubConnecting>>,
    /// Ends the connection under way.
    cancel: Mutex<Option<CancellationToken>>,
    /// The account last read from `gh`, and when; held while it is read again.
    account: tokio::sync::Mutex<Option<(Instant, Account)>>,
}

impl Github {
    pub fn new(daemon: Weak<Daemon>) -> Self {
        Github {
            daemon,
            http: reqwest::Client::new(),
            releases: Mutex::new((
                RELEASES.into(),
                release_for(std::env::consts::OS, std::env::consts::ARCH),
            )),
            connecting: Mutex::new(None),
            cancel: Mutex::new(None),
            account: tokio::sync::Mutex::new(None),
        }
    }

    /// Tests serve a release of their own.
    pub fn use_release(&self, base: &str, release: Release) {
        *self.releases.lock().unwrap() = (base.into(), Some(release));
    }

    fn d(&self) -> std::sync::Arc<Daemon> {
        self.daemon.upgrade().expect("daemon alive")
    }

    fn own_executable(&self) -> PathBuf {
        tools_dir(&self.d().paths.home).join("gh")
    }

    pub async fn status(&self) -> GithubStatus {
        let env = self.d().shell_env.vars();
        let gh = find(&env, "gh");
        let account = match &gh {
            Some(gh) => self.account(gh, &env).await,
            None => Account::default(),
        };
        GithubStatus {
            git: git_found(&env).await,
            git_install: git_install_command().into(),
            own: gh.as_ref().is_some_and(|gh| *gh == self.own_executable()),
            gh: gh.map(|gh| gh.to_string_lossy().into_owned()),
            version: VERSION.into(),
            download_size: self.releases.lock().unwrap().1.as_ref().map(|r| r.size),
            account: account.login,
            organizations: account.organizations,
            organization: self.d().settings.get().github_org.clone(),
            connecting: self.connecting.lock().unwrap().clone(),
        }
    }

    /// The account `gh` is signed in to, read again once stale.
    async fn account(&self, gh: &Path, env: &BTreeMap<String, String>) -> Account {
        let mut cached = self.account.lock().await;
        if let Some((at, account)) = cached.as_ref()
            && at.elapsed() < ACCOUNT_TTL
        {
            return account.clone();
        }
        let account = read_account(gh, env).await;
        *cached = Some((Instant::now(), account.clone()));
        account
    }

    /// The account changed: read it again on the next ask, the branch prefix's login too.
    async fn forget_account(&self) {
        *self.account.lock().await = None;
        if let Err(error) = self.d().store.kv_delete(LOGIN_KEY) {
            tracing::warn!(%error, "could not forget the github login");
        }
    }

    /// Starts connecting in the background; `status` follows it.
    pub fn connect(&self) -> Result<()> {
        let env = self.d().shell_env.vars();
        if find(&env, "gh").is_none() && self.releases.lock().unwrap().1.is_none() {
            return Err(Error::BadRequest(
                "Chauffeur installs GitHub CLI on macOS and on Linux (x86_64, arm64) only: install gh from https://cli.github.com, then connect again".into(),
            ));
        }
        {
            let mut connecting = self.connecting.lock().unwrap();
            if connecting.as_ref().is_some_and(|step| step.error.is_none()) {
                return Err(Error::Conflict(
                    "GitHub is already being connected: finish on GitHub's page, or cancel".into(),
                ));
            }
            *connecting = Some(step("starting"));
        }
        let token = CancellationToken::new();
        *self.cancel.lock().unwrap() = Some(token.clone());
        let daemon = self.d();
        tokio::spawn(async move {
            // Cancelling drops the connection's future, and with it the `gh` it runs.
            let result = tokio::select! {
                result = daemon.github.run_connect() => Some(result),
                () = token.cancelled() => None,
            };
            daemon.github.forget_account().await;
            let mut connecting = daemon.github.connecting.lock().unwrap();
            match result {
                Some(Err(error)) => {
                    tracing::warn!(%error, "could not connect github");
                    if let Some(step) = connecting.as_mut() {
                        step.error = Some(format!("{error:#}"));
                    }
                }
                Some(Ok(())) | None => *connecting = None,
            }
        });
        Ok(())
    }

    /// Ends the connection under way; nothing when none runs.
    pub fn cancel(&self) {
        if let Some(token) = self.cancel.lock().unwrap().take() {
            token.cancel();
        }
    }

    pub async fn disconnect(&self) -> Result<()> {
        let env = self.d().shell_env.vars();
        let gh = find(&env, "gh").ok_or_else(|| {
            Error::BadRequest("GitHub CLI is not installed: no account is connected".into())
        })?;
        let Some(login) = self.account(&gh, &env).await.login else {
            return Err(Error::BadRequest("No GitHub account is connected".into()));
        };
        let output = Command::new(&gh)
            .args(["auth", "logout", "--hostname", HOST, "--user", &login])
            .env_clear()
            .envs(&env)
            .stdin(Stdio::null())
            .output()
            .await
            .context("running gh auth logout")?;
        self.forget_account().await;
        if !output.status.success() {
            return Err(Error::Unavailable(format!(
                "Could not disconnect {login} from GitHub: {}",
                last_line(&output.stderr)
            )));
        }
        Ok(())
    }

    fn step(&self, phase: &str, update: impl FnOnce(&mut GithubConnecting)) {
        if let Some(step) = self.connecting.lock().unwrap().as_mut() {
            step.phase = phase.into();
            update(step);
        }
    }

    async fn run_connect(&self) -> anyhow::Result<()> {
        let env = self.d().shell_env.vars();
        let gh = match find(&env, "gh") {
            Some(gh) => gh,
            None => self.install().await?,
        };
        self.step("starting", |_| {});
        // Without a terminal, gh prints the one-time code and the page to enter it on, then waits
        // for GitHub; the `workflow` scope lets an agent push changes to CI files over HTTPS.
        let mut child = Command::new(&gh)
            .args([
                "auth",
                "login",
                "--hostname",
                HOST,
                "--git-protocol",
                "https",
                "--web",
                "--skip-ssh-key",
                "--scopes",
                "workflow",
            ])
            .env_clear()
            .envs(&env)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("running gh auth login")?;
        let stderr = child.stderr.take().expect("stderr is piped");
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        let mut last = String::new();
        while let Some(line) = lines.next_line().await? {
            if let Some(code) = one_time_code(&line) {
                self.step("waiting", |step| step.code = Some(code));
            } else if let Some(url) = line.split_whitespace().find(|w| w.starts_with("https://")) {
                self.step("waiting", |step| step.url = Some(url.into()));
            }
            if !line.trim().is_empty() {
                last = line;
            }
        }
        let status = child.wait().await?;
        let why = last.trim_start_matches(['!', 'X', ' ']);
        anyhow::ensure!(
            status.success(),
            "GitHub did not complete the sign-in ({}): connect again",
            if why.is_empty() {
                "GitHub CLI stopped without saying why"
            } else {
                why
            }
        );
        self.step("configuring", |_| {});
        let output = Command::new(&gh)
            .args(["auth", "setup-git", "--hostname", HOST])
            .env_clear()
            .envs(&env)
            .stdin(Stdio::null())
            .output()
            .await
            .context("running gh auth setup-git")?;
        anyhow::ensure!(
            output.status.success(),
            "signed in, but git could not be set to push through GitHub CLI: {}",
            last_line(&output.stderr)
        );
        tracing::info!(gh = %gh.display(), "github connected");
        Ok(())
    }

    /// Downloads, checks and unpacks Chauffeur's own `gh`; returns it.
    async fn install(&self) -> anyhow::Result<PathBuf> {
        use sha2::Digest;
        use tokio::io::AsyncWriteExt;
        let (base, release) = self.releases.lock().unwrap().clone();
        let release = release.context("Chauffeur does not install GitHub CLI on this system")?;
        let home = self.d().paths.home.clone();
        let root = home.join("gh");
        std::fs::create_dir_all(&root)?;
        let download = root.join(format!(".download-{}", release.name));
        let url = format!("{base}/v{VERSION}/{}", release.name);
        let mut response = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("downloading {url}"))?
            .error_for_status()?;
        let total = response.content_length().unwrap_or(release.size);
        let mut file = tokio::fs::File::create(&download).await?;
        let mut hasher = sha2::Sha256::new();
        let mut completed = 0;
        while let Some(chunk) = response.chunk().await? {
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
            completed += chunk.len() as u64;
            self.step("downloading", |step| {
                step.completed = completed;
                step.total = total;
            });
        }
        file.flush().await?;
        drop(file);
        self.step("checking", |_| {});
        if hex::encode(hasher.finalize()) != release.sha256 {
            let _ = std::fs::remove_file(&download);
            anyhow::bail!(
                "the download of {} does not match its published checksum: try again",
                release.name
            );
        }
        self.step("unpacking", |step| {
            step.completed = 0;
            step.total = 0;
        });
        let unpacked = {
            let (download, root) = (download.clone(), root.clone());
            tokio::task::spawn_blocking(move || unpack(&download, &root)).await??
        };
        let _ = std::fs::remove_file(&download);
        let dir = root.join(VERSION);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        std::fs::rename(&unpacked, &dir)?;
        let link = self.own_executable();
        std::fs::create_dir_all(tools_dir(&home))?;
        let _ = std::fs::remove_file(&link);
        // A link of a stable name: git's credential helper names it, so an upgrade keeps it.
        std::os::unix::fs::symlink(dir.join("bin").join("gh"), &link)?;
        for entry in std::fs::read_dir(&root)?.flatten() {
            if entry.file_name() != VERSION {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
        tracing::info!(version = VERSION, dir = %dir.display(), "gh installed");
        Ok(link)
    }
}

fn step(phase: &str) -> GithubConnecting {
    GithubConnecting {
        phase: phase.into(),
        completed: 0,
        total: 0,
        code: None,
        url: None,
        error: None,
    }
}

/// `! First copy your one-time code: 604A-A032` gives `604A-A032`.
fn one_time_code(line: &str) -> Option<String> {
    let (_, code) = line.split_once("one-time code:")?;
    let code = code.trim();
    (!code.is_empty() && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .then(|| code.into())
}

fn last_line(output: &[u8]) -> String {
    String::from_utf8_lossy(output)
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .unwrap_or("no output")
        .trim()
        .to_string()
}

/// The account `gh` is signed in to on github.com, with its organizations; none when signed out,
/// or when GitHub refuses the token.
async fn read_account(gh: &Path, env: &BTreeMap<String, String>) -> Account {
    let run = |args: &'static [&'static str]| {
        Command::new(gh)
            .args(args)
            .env_clear()
            .envs(env)
            .stdin(Stdio::null())
            .output()
    };
    // Older user-installed gh versions have neither `auth status --active` nor `--json`.
    // Asking /user also verifies the credential gh actually uses, including an environment token.
    let login = match run(&["api", "--hostname", HOST, "user"]).await {
        Ok(output) if output.status.success() => {
            #[derive(serde::Deserialize)]
            struct User {
                login: String,
            }
            serde_json::from_slice::<User>(&output.stdout)
                .ok()
                .map(|user| user.login)
                .filter(|login| !login.is_empty())
        }
        Ok(output) => {
            tracing::warn!(
                error = last_line(&output.stderr),
                "could not read the github account"
            );
            None
        }
        Err(error) => {
            tracing::warn!(%error, "could not ask gh for its account");
            None
        }
    };
    let Some(login) = login else {
        return Account::default();
    };
    let organizations = match run(&[
        "api",
        "--hostname",
        HOST,
        "user/orgs",
        "--paginate",
        "--jq",
        ".[].login",
    ])
    .await
    {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|org| !org.is_empty())
            .map(String::from)
            .collect(),
        Ok(output) => {
            tracing::warn!(
                error = last_line(&output.stderr),
                "could not list the github organizations"
            );
            Vec::new()
        }
        Err(error) => {
            tracing::warn!(%error, "could not list the github organizations");
            Vec::new()
        }
    };
    Account {
        login: Some(login),
        organizations,
    }
}

/// git is there: on macOS, `/usr/bin/git` is only a stub until the command line developer tools
/// are installed, and running it opens Apple's installer, so `xcode-select` is asked instead.
async fn git_found(env: &BTreeMap<String, String>) -> bool {
    let Some(git) = find(env, "git") else {
        return false;
    };
    if cfg!(target_os = "macos") && git == Path::new("/usr/bin/git") {
        return Command::new("/usr/bin/xcode-select")
            .arg("-p")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .is_ok_and(|status| status.success());
    }
    true
}

/// The first executable `name` on the environment's PATH.
fn find(env: &BTreeMap<String, String>, name: &str) -> Option<PathBuf> {
    std::env::split_paths(env.get("PATH")?)
        .map(|dir| dir.join(name))
        .find(|path| is_executable(path))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// Unpacks a release archive into a new folder of `root` and returns it, `bin/gh` in it; a failure
/// leaves nothing. GitHub CLI's archives hold one folder, `gh_<version>_<platform>`.
fn unpack(archive: &Path, root: &Path) -> anyhow::Result<PathBuf> {
    let dir = tempfile::Builder::new()
        .prefix(".unpack-")
        .tempdir_in(root)?;
    if archive.to_string_lossy().ends_with(".zip") {
        // ditto ships with every macOS, the only system whose archive is a zip.
        let status = std::process::Command::new("/usr/bin/ditto")
            .args(["-x", "-k"])
            .arg(archive)
            .arg(dir.path())
            .status()
            .context("unpacking GitHub CLI's archive")?;
        anyhow::ensure!(status.success(), "could not unpack GitHub CLI's archive");
    } else {
        let file = std::fs::File::open(archive)?;
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        tar.set_preserve_permissions(true);
        tar.unpack(dir.path())
            .context("unpacking GitHub CLI's archive")?;
    }
    let top = std::fs::read_dir(dir.path())?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| is_executable(&path.join("bin").join("gh")))
        .context("GitHub CLI's archive holds no `bin/gh`")?;
    let unpacked = root.join(format!(".unpacked-{}", uuid::Uuid::now_v7()));
    std::fs::rename(top, &unpacked)?;
    Ok(unpacked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_one_time_code_is_read_from_what_gh_prints() {
        assert_eq!(
            one_time_code("! First copy your one-time code: 604A-A032").as_deref(),
            Some("604A-A032")
        );
        assert_eq!(
            one_time_code(
                "Open this URL to continue in your web browser: https://github.com/login/device"
            ),
            None
        );
    }

    #[tokio::test]
    async fn an_existing_login_is_detected_without_new_auth_status_flags() {
        let account = account_from_fake_gh(0, r#"{"login":"octocat"}"#, 0).await;
        assert_eq!(account.login.as_deref(), Some("octocat"));
        assert_eq!(account.organizations, ["acme"]);
    }

    #[tokio::test]
    async fn a_refused_or_unreadable_account_is_not_connected() {
        for (exit, body) in [
            (1, r#"{"login":"octocat"}"#),
            (0, "not json"),
            (0, r#"{"login":""}"#),
        ] {
            assert!(account_from_fake_gh(exit, body, 0).await.login.is_none());
        }
    }

    #[tokio::test]
    async fn an_account_stays_connected_when_organizations_cannot_be_listed() {
        let account = account_from_fake_gh(0, r#"{"login":"octocat"}"#, 1).await;
        assert_eq!(account.login.as_deref(), Some("octocat"));
        assert!(account.organizations.is_empty());
    }

    async fn account_from_fake_gh(exit: u8, body: &str, org_exit: u8) -> Account {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let gh = dir.path().join("gh");
        std::fs::write(
            &gh,
            format!(
                r#"#!/bin/sh
case "$*" in
  "api --hostname github.com user") echo '{body}'; exit {exit} ;;
  "api --hostname github.com user/orgs --paginate --jq .[].login") echo acme; exit {org_exit} ;;
  *) echo 'unsupported command or flag' >&2; exit 1 ;;
esac
"#
            ),
        )
        .unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        read_account(&gh, &BTreeMap::new()).await
    }

    #[test]
    fn chauffeur_installs_gh_on_macos_and_linux_only() {
        for (os, arch) in [
            ("macos", "aarch64"),
            ("macos", "x86_64"),
            ("linux", "x86_64"),
            ("linux", "aarch64"),
        ] {
            let release = release_for(os, arch).unwrap();
            assert!(release.name.starts_with(&format!("gh_{VERSION}_")));
            assert_eq!(release.sha256.len(), 64);
        }
        assert!(release_for("windows", "x86_64").is_none());
    }
}
