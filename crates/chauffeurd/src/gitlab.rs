//! A self-hosted GitLab in place of GitHub, when the installation is set to it
//! (docs/adr/gitlab-as-the-forge.md). Chauffeur reaches it through `glab`: the user's own when one
//! is on the PATH, else a copy Chauffeur installs (pinned, checked against its SHA-256) next to its
//! `gh`, and only for an installation set to GitLab. Signing in hands a personal access token to
//! `glab auth login --stdin`, which keeps it in the system's keyring and lets git push over HTTPS
//! through it: Chauffeur never writes the token.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, Weak};
use std::time::{Duration, Instant};

use anyhow::Context;
use chauffeur_protocol::client::{Forge, GitlabConnecting, GitlabStatus};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::github::{Release, find, is_executable, last_line, tools_dir};
use crate::{Daemon, Error, Result};

/// The `glab` Chauffeur installs: an upgrade changes it and the checksums of `release_for` together.
pub const VERSION: &str = "1.130.0";
const RELEASES: &str = "https://gitlab.com/api/v4/projects/gitlab-org%2Fcli/packages/generic/glab";
/// How long the account read from `glab` stays good: reading it asks the GitLab.
const ACCOUNT_TTL: Duration = Duration::from_secs(60);

/// The archive for this system; none where Chauffeur does not install `glab`.
fn release_for(os: &str, arch: &str) -> Option<Release> {
    let (platform, sha256, size) = match (os, arch) {
        ("macos", "aarch64") => (
            "darwin_arm64",
            "5791b652e774e4eca85afba1ebfd9db8498138c3311e0ed0a39d19590311f232",
            19_221_663,
        ),
        ("macos", "x86_64") => (
            "darwin_amd64",
            "ea0e53754ed6b38fc92eb8ddaee5b103a5e2bd54db654686f39512022be5744f",
            20_984_955,
        ),
        ("linux", "x86_64") => (
            "linux_amd64",
            "ce06313b2075af25180cd195ab3496d42bc5857c85a1c12ba996838dfe9d822c",
            20_206_088,
        ),
        ("linux", "aarch64") => (
            "linux_arm64",
            "062d53acb0f2cbcc79a014600df1aa0fb440bb3a08a5d4900d3b49b553b22767",
            18_484_645,
        ),
        _ => return None,
    };
    Some(Release {
        name: format!("glab_{VERSION}_{platform}.tar.gz"),
        sha256: sha256.into(),
        size,
    })
}

/// A GitLab host as the settings keep it: `gitlab.example.com` or `gitlab.example.com:8443`,
/// lowercase. A scheme or a trailing slash pasted with it is dropped; a path is refused.
pub fn check_host(host: &str) -> Result<String> {
    let host = host.trim().to_lowercase();
    let host = host
        .strip_prefix("https://")
        .or_else(|| host.strip_prefix("http://"))
        .unwrap_or(&host)
        .trim_end_matches('/');
    let (name, port) = host.split_once(':').unwrap_or((host, ""));
    let valid = name.is_empty()
        || (name.contains('.')
            && name.split('.').all(|label| {
                !label.is_empty()
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            })
            && (port.is_empty() || port.parse::<u16>().is_ok_and(|port| port > 0)));
    if !valid {
        return Err(Error::BadRequest(format!(
            "\"{host}\" is not a GitLab host: give its name, such as gitlab.example.com"
        )));
    }
    Ok(host.to_string())
}

pub struct Gitlab {
    daemon: Weak<Daemon>,
    http: reqwest::Client,
    /// Where releases are downloaded from, and the archive to take; tests serve their own.
    releases: Mutex<(String, Option<Release>)>,
    connecting: Mutex<Option<GitlabConnecting>>,
    /// The account last read from `glab`, the host it was read on, and when.
    account: tokio::sync::Mutex<Option<(Instant, String, Option<String>)>>,
}

impl Gitlab {
    pub fn new(daemon: Weak<Daemon>) -> Self {
        Gitlab {
            daemon,
            http: reqwest::Client::new(),
            releases: Mutex::new((
                RELEASES.into(),
                release_for(std::env::consts::OS, std::env::consts::ARCH),
            )),
            connecting: Mutex::new(None),
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
        tools_dir(&self.d().paths.home).join("glab")
    }

    /// The host of an installation set to GitLab; none otherwise.
    fn host(&self) -> Option<String> {
        let settings = self.d().settings.get();
        (settings.forge == Forge::Gitlab && !settings.gitlab_host.is_empty())
            .then(|| settings.gitlab_host.clone())
    }

    pub async fn status(&self) -> GitlabStatus {
        let settings = self.d().settings.get();
        let enabled = settings.forge == Forge::Gitlab;
        let env = self.d().shell_env.vars();
        let glab = find(&env, "glab");
        let account = match (&glab, self.host()) {
            (Some(glab), Some(host)) => self.account(glab, &host, &env).await,
            _ => None,
        };
        GitlabStatus {
            enabled,
            host: settings.gitlab_host.clone(),
            own: glab
                .as_ref()
                .is_some_and(|glab| *glab == self.own_executable()),
            glab: glab.map(|glab| glab.to_string_lossy().into_owned()),
            version: VERSION.into(),
            download_size: self.releases.lock().unwrap().1.as_ref().map(|r| r.size),
            account,
            connecting: self.connecting.lock().unwrap().clone(),
        }
    }

    /// The account `glab` is signed in to on `host`, read again once stale or for another host.
    async fn account(
        &self,
        glab: &Path,
        host: &str,
        env: &BTreeMap<String, String>,
    ) -> Option<String> {
        let mut cached = self.account.lock().await;
        if let Some((at, read_on, account)) = cached.as_ref()
            && read_on == host
            && at.elapsed() < ACCOUNT_TTL
        {
            return account.clone();
        }
        let account = read_account(glab, host, env).await;
        *cached = Some((Instant::now(), host.to_string(), account.clone()));
        account
    }

    async fn forget_account(&self) {
        *self.account.lock().await = None;
    }

    /// Starts signing in in the background; `status` follows it. The token goes to `glab` only.
    pub fn connect(&self, token: String) -> Result<()> {
        let host = self.host().ok_or_else(|| {
            Error::BadRequest(
                "Chauffeur is not set to GitLab: choose GitLab and its host in Settings first"
                    .into(),
            )
        })?;
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err(Error::BadRequest(
                "Paste a personal access token of your GitLab account".into(),
            ));
        }
        let env = self.d().shell_env.vars();
        if find(&env, "glab").is_none() && self.releases.lock().unwrap().1.is_none() {
            return Err(Error::BadRequest(
                "Chauffeur installs GitLab CLI on macOS and on Linux (x86_64, arm64) only: install glab, then connect again".into(),
            ));
        }
        {
            let mut connecting = self.connecting.lock().unwrap();
            if connecting.as_ref().is_some_and(|step| step.error.is_none()) {
                return Err(Error::Conflict("GitLab is already being connected".into()));
            }
            *connecting = Some(step("starting"));
        }
        let daemon = self.d();
        tokio::spawn(async move {
            let result = daemon.gitlab.run_connect(&host, &token).await;
            daemon.gitlab.forget_account().await;
            let mut connecting = daemon.gitlab.connecting.lock().unwrap();
            match result {
                Err(error) => {
                    tracing::warn!(%error, host, "could not connect gitlab");
                    if let Some(step) = connecting.as_mut() {
                        step.error = Some(format!("{error:#}"));
                    }
                }
                Ok(()) => *connecting = None,
            }
        });
        Ok(())
    }

    pub async fn disconnect(&self) -> Result<()> {
        let host = self
            .host()
            .ok_or_else(|| Error::BadRequest("Chauffeur is not set to GitLab".into()))?;
        let env = self.d().shell_env.vars();
        let glab = find(&env, "glab").ok_or_else(|| {
            Error::BadRequest("GitLab CLI is not installed: no account is connected".into())
        })?;
        let output = Command::new(&glab)
            .args(["auth", "logout", "--hostname", &host])
            .env_clear()
            .envs(&env)
            .stdin(Stdio::null())
            .output()
            .await
            .context("running glab auth logout")?;
        self.forget_account().await;
        if !output.status.success() {
            return Err(Error::Unavailable(format!(
                "Could not disconnect {host}: {}",
                last_line(&output.stderr)
            )));
        }
        Ok(())
    }

    fn step(&self, phase: &str, update: impl FnOnce(&mut GitlabConnecting)) {
        if let Some(step) = self.connecting.lock().unwrap().as_mut() {
            step.phase = phase.into();
            update(step);
        }
    }

    async fn run_connect(&self, host: &str, token: &str) -> anyhow::Result<()> {
        let env = self.d().shell_env.vars();
        let glab = match find(&env, "glab") {
            Some(glab) => glab,
            None => self.install().await?,
        };
        self.step("signing_in", |_| {});
        check_token(&glab, host, token, &env).await?;
        sign_in(&glab, host, token, &env).await?;
        tracing::info!(glab = %glab.display(), host, "gitlab connected");
        Ok(())
    }

    /// Downloads, checks and unpacks Chauffeur's own `glab`; returns it.
    async fn install(&self) -> anyhow::Result<PathBuf> {
        use sha2::Digest;
        let (base, release) = self.releases.lock().unwrap().clone();
        let release = release.context("Chauffeur does not install GitLab CLI on this system")?;
        let home = self.d().paths.home.clone();
        let root = home.join("glab");
        std::fs::create_dir_all(&root)?;
        let download = root.join(format!(".download-{}", release.name));
        let url = format!("{base}/{VERSION}/{}", release.name);
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
        std::os::unix::fs::symlink(dir.join("bin").join("glab"), &link)?;
        for entry in std::fs::read_dir(&root)?.flatten() {
            if entry.file_name() != VERSION {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
        tracing::info!(version = VERSION, dir = %dir.display(), "glab installed");
        Ok(link)
    }
}

fn step(phase: &str) -> GitlabConnecting {
    GitlabConnecting {
        phase: phase.into(),
        completed: 0,
        total: 0,
        error: None,
    }
}

/// Asks `host` who the token is before anything keeps it: `glab auth login` stores a token without
/// asking the GitLab, and a refused one would stay in the keyring and the git configuration. The
/// token goes in the environment of that one `glab`, whose configuration is a temporary folder.
async fn check_token(
    glab: &Path,
    host: &str,
    token: &str,
    env: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    let config = tempfile::tempdir()?;
    let output = Command::new(glab)
        .args(["api", "--hostname", host, "user"])
        .env_clear()
        .envs(env)
        .env("GLAB_CONFIG_DIR", config.path())
        .env("GITLAB_HOST", host)
        .env("GITLAB_TOKEN", token)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .context("running glab api user")?;
    anyhow::ensure!(
        output.status.success() && username(&output.stdout).is_some(),
        "{host} did not accept the token ({}): check it is a personal access token with the api and write_repository scopes",
        last_line(&output.stderr)
    );
    Ok(())
}

fn username(json: &[u8]) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct User {
        username: String,
    }
    serde_json::from_slice::<User>(json)
        .ok()
        .map(|user| user.username)
        .filter(|username| !username.is_empty())
}

/// Signs `glab` in to `host` with the token on its standard input, git over HTTPS through it.
async fn sign_in(
    glab: &Path,
    host: &str,
    token: &str,
    env: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    let mut child = Command::new(glab)
        .args([
            "auth",
            "login",
            "--hostname",
            host,
            "--stdin",
            "--git-protocol",
            "https",
            "--api-protocol",
            "https",
            "--setup-git-credential-helper",
        ])
        .env_clear()
        .envs(env)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("running glab auth login")?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    stdin.write_all(token.as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    drop(stdin);
    let output = child.wait_with_output().await?;
    anyhow::ensure!(
        output.status.success(),
        "{host} did not accept the token ({}): check it has the api and write_repository scopes",
        last_line(&output.stderr)
    );
    Ok(())
}

/// The username `glab` is signed in as on `host`; none when signed out, or when the GitLab
/// refuses the token.
async fn read_account(glab: &Path, host: &str, env: &BTreeMap<String, String>) -> Option<String> {
    let output = Command::new(glab)
        .args(["api", "--hostname", host, "user"])
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .output()
        .await;
    match output {
        Ok(output) if output.status.success() => username(&output.stdout),
        Ok(output) => {
            tracing::debug!(error = last_line(&output.stderr), host, "no gitlab account");
            None
        }
        Err(error) => {
            tracing::warn!(%error, "could not ask glab for its account");
            None
        }
    }
}

/// Unpacks a release archive into a new folder of `root` and returns it, `bin/glab` in it; a
/// failure leaves nothing. GitLab CLI's archives hold `bin/glab` at their root.
fn unpack(archive: &Path, root: &Path) -> anyhow::Result<PathBuf> {
    let dir = tempfile::Builder::new()
        .prefix(".unpack-")
        .tempdir_in(root)?;
    let file = std::fs::File::open(archive)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    tar.set_preserve_permissions(true);
    tar.unpack(dir.path())
        .context("unpacking GitLab CLI's archive")?;
    anyhow::ensure!(
        is_executable(&dir.path().join("bin").join("glab")),
        "GitLab CLI's archive holds no `bin/glab`"
    );
    let unpacked = root.join(format!(".unpacked-{}", uuid::Uuid::now_v7()));
    std::fs::rename(dir.keep(), &unpacked)?;
    Ok(unpacked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_is_kept_as_its_name() {
        for (given, kept) in [
            ("gitlab.example.com", "gitlab.example.com"),
            ("  GitLab.Example.com ", "gitlab.example.com"),
            ("https://gitlab.example.com/", "gitlab.example.com"),
            ("gitlab.example.com:8443", "gitlab.example.com:8443"),
            ("", ""),
        ] {
            assert_eq!(check_host(given).unwrap(), kept, "{given}");
        }
        for refused in [
            "gitlab.example.com/group",
            "localhost",
            "gitlab..example.com",
            "gitlab.example.com:0",
            "git lab.example.com",
            "-gitlab.example.com",
        ] {
            assert!(check_host(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn chauffeur_installs_glab_on_macos_and_linux_only() {
        for (os, arch) in [
            ("macos", "aarch64"),
            ("macos", "x86_64"),
            ("linux", "x86_64"),
            ("linux", "aarch64"),
        ] {
            let release = release_for(os, arch).unwrap();
            assert!(release.name.starts_with(&format!("glab_{VERSION}_")));
            assert_eq!(release.sha256.len(), 64);
        }
        assert!(release_for("windows", "x86_64").is_none());
    }

    #[test]
    fn the_archive_unpacks_with_glab_at_its_root() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("glab.tar.gz");
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
            std::fs::File::create(&archive).unwrap(),
            flate2::Compression::fast(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_size(10);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, "bin/glab", &b"#!/bin/sh\n"[..])
            .unwrap();
        tar.into_inner().unwrap().finish().unwrap();
        let unpacked = unpack(&archive, root.path()).unwrap();
        assert!(is_executable(&unpacked.join("bin").join("glab")));
    }

    #[tokio::test]
    async fn a_refused_token_is_never_handed_to_glab_auth_login() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let glab = dir.path().join("glab");
        let seen = dir.path().join("seen");
        // The GitLab refuses the token: `api user` fails, and `auth login` must not run.
        std::fs::write(
            &glab,
            format!(
                "#!/bin/sh\necho \"$1 $2 token=$GITLAB_TOKEN\" >> {seen}\n[ \"$1\" = api ] && {{ echo '401 Unauthorized' >&2; exit 1; }}\nexit 0\n",
                seen = seen.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&glab, std::fs::Permissions::from_mode(0o755)).unwrap();
        let refused = check_token(&glab, "gitlab.example.com", "glpat-bad", &BTreeMap::new())
            .await
            .unwrap_err();
        assert!(format!("{refused:#}").contains("did not accept the token (401 Unauthorized)"));
        let seen = std::fs::read_to_string(seen).unwrap();
        assert_eq!(seen.trim(), "api --hostname token=glpat-bad");
    }

    #[tokio::test]
    async fn the_token_reaches_glab_on_its_standard_input_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let glab = dir.path().join("glab");
        let seen = dir.path().join("seen");
        std::fs::write(
            &glab,
            format!(
                "#!/bin/sh\necho \"$*\" > {seen}\ncat >> {seen}\n",
                seen = seen.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&glab, std::fs::Permissions::from_mode(0o755)).unwrap();
        sign_in(
            &glab,
            "gitlab.example.com",
            "glpat-secret",
            &BTreeMap::new(),
        )
        .await
        .unwrap();
        let seen = std::fs::read_to_string(seen).unwrap();
        let (args, stdin) = seen.split_once('\n').unwrap();
        assert!(args.starts_with("auth login --hostname gitlab.example.com --stdin"));
        assert!(!args.contains("glpat-secret"));
        assert_eq!(stdin.trim(), "glpat-secret");
    }
}
