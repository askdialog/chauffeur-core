//! This Mac's GitHub and AWS SSO access, handed to a GCP worker on each send so that its user
//! signs in to neither again there (docs/cloud-workers.md). The agents' logins are not copied:
//! refreshing one machine's token would log the other out. The secrets are never logged, nor put
//! in a result, an event or a transfer's log.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, bail};
use chauffeur_protocol::worker::{
    WorkerAwsSsoFile, WorkerCredentials, WorkerCredentialsInstalled, WorkerGithubToken,
};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;

use crate::{Daemon, Result};

/// The home folder `gh`, `git` and the AWS CLI resolve: the login shell's, else this process's.
fn home(daemon: &Daemon) -> Option<PathBuf> {
    daemon
        .shell_env
        .vars()
        .get("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
}

/// What this Mac's `gh` and AWS CLI can hand a worker, and what could not be read of them. A `gh`
/// signed out or missing, and no AWS configuration, give nothing and say nothing: the worker's
/// own check says gh is not signed in there.
pub(crate) async fn gather(daemon: &Daemon) -> (WorkerCredentials, Vec<String>) {
    let mut warnings = Vec::new();
    let github = github_tokens(daemon).await;
    let (aws_config, aws_sso_cache) = match home(daemon).map(|home| read_aws(&home)) {
        Some(Ok(aws)) => aws,
        Some(Err(error)) => {
            warnings.push(format!("Could not read this Mac's AWS sign-in: {error:#}"));
            Default::default()
        }
        None => Default::default(),
    };
    let credentials = WorkerCredentials {
        github,
        aws_config,
        aws_sso_cache,
    };
    (credentials, warnings)
}

/// `gh auth status --json hosts`: each host's accounts.
#[derive(Deserialize)]
struct GhStatus {
    hosts: std::collections::BTreeMap<String, Vec<GhAccount>>,
}

#[derive(Deserialize)]
struct GhAccount {
    state: String,
}

/// The token of each host `gh` is signed in to here; github.com's only from a `gh` too old for
/// `--json`.
async fn github_tokens(daemon: &Daemon) -> Vec<WorkerGithubToken> {
    let hosts = match gh(
        daemon,
        &["auth", "status", "--active", "--json", "hosts"],
        None,
    )
    .await
    {
        Ok(status) => match serde_json::from_str::<GhStatus>(&status) {
            Ok(status) => status
                .hosts
                .into_iter()
                .filter(|(_, accounts)| accounts.iter().any(|a| a.state == "success"))
                .map(|(host, _)| host)
                .collect(),
            Err(_) => vec!["github.com".to_string()],
        },
        Err(_) => return Vec::new(),
    };
    let mut tokens = Vec::new();
    for host in hosts {
        if let Ok(token) = gh(daemon, &["auth", "token", "--hostname", &host], None).await {
            let token = token.trim().to_string();
            if !token.is_empty() {
                tokens.push(WorkerGithubToken { host, token });
            }
        }
    }
    tokens
}

/// The AWS CLI's `config` and SSO token cache under `home`; neither when absent.
fn read_aws(home: &Path) -> anyhow::Result<(Option<String>, Vec<WorkerAwsSsoFile>)> {
    let aws = home.join(".aws");
    let config = match fs::read_to_string(aws.join("config")) {
        Ok(config) => Some(config),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("reading ~/.aws/config"),
    };
    let mut cache = Vec::new();
    let entries = match fs::read_dir(aws.join("sso").join("cache")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok((config, cache)),
        Err(error) => return Err(error).context("reading ~/.aws/sso/cache"),
    };
    for entry in entries {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !cache_name(&name) || !entry.file_type()?.is_file() {
            continue;
        }
        let contents = fs::read_to_string(entry.path())
            .with_context(|| format!("reading ~/.aws/sso/cache/{name}"))?;
        cache.push(WorkerAwsSsoFile { name, contents });
    }
    cache.sort_by(|a, b| a.name.cmp(&b.name));
    Ok((config, cache))
}

/// A file of the SSO token cache: `<name>.json`, no folder, nothing hidden.
fn cache_name(name: &str) -> bool {
    name.len() > ".json".len()
        && name.ends_with(".json")
        && !name.starts_with('.')
        && !name.contains('/')
}

/// On the worker: signs its `gh` in with each token and has `git` push through it, then writes the
/// AWS files, owner-only, over the previous copies. A part that fails is a warning: the transfer
/// goes on without it.
pub(crate) async fn install(
    daemon: &Daemon,
    credentials: WorkerCredentials,
) -> Result<WorkerCredentialsInstalled> {
    let machine = crate::cloud_transfer::host_name();
    let Some(home) = home(daemon) else {
        return Ok(WorkerCredentialsInstalled {
            warnings: vec![format!(
                "{machine} has no home folder: it got neither GitHub nor AWS access"
            )],
        });
    };
    let mut warnings = Vec::new();
    for token in &credentials.github {
        if let Err(error) = install_github(daemon, &home, token).await {
            warnings.push(format!(
                "Could not sign gh in to {} on {machine}: {error:#}",
                token.host
            ));
        }
    }
    if let Err(error) = install_aws(&home, &credentials) {
        warnings.push(format!(
            "Could not copy this Mac's AWS sign-in to {machine}: {error:#}"
        ));
    }
    Ok(WorkerCredentialsInstalled { warnings })
}

async fn install_github(
    daemon: &Daemon,
    home: &Path,
    token: &WorkerGithubToken,
) -> anyhow::Result<()> {
    let host = &token.host;
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        bail!("not a host name");
    }
    gh(
        daemon,
        &[
            "auth",
            "login",
            "--hostname",
            host,
            "--git-protocol",
            "https",
            "--with-token",
        ],
        Some(&token.token),
    )
    .await?;
    gh(daemon, &["auth", "setup-git", "--hostname", host], None).await?;
    // The worktrees' `origin` keeps the Mac's URL, often SSH, and the worker has no SSH key for
    // the host: its git goes through HTTPS, which gh signs.
    let https = format!("url.https://{host}/.insteadOf");
    let git = crate::git::Git::new(daemon.shell_env.clone());
    for ssh in [format!("git@{host}:"), format!("ssh://git@{host}/")] {
        git.run(
            home,
            &[
                "config",
                "--global",
                "--fixed-value",
                "--replace-all",
                &https,
                &ssh,
                &ssh,
            ],
        )
        .await?;
    }
    Ok(())
}

fn install_aws(home: &Path, credentials: &WorkerCredentials) -> anyhow::Result<()> {
    if credentials.aws_config.is_none() && credentials.aws_sso_cache.is_empty() {
        return Ok(());
    }
    let aws = home.join(".aws");
    private_dir_all(&aws)?;
    if let Some(config) = &credentials.aws_config {
        replace_private(&aws.join("config"), config)?;
    }
    if credentials.aws_sso_cache.is_empty() {
        return Ok(());
    }
    let cache = aws.join("sso").join("cache");
    private_dir_all(&aws.join("sso"))?;
    private_dir_all(&cache)?;
    for file in &credentials.aws_sso_cache {
        if !cache_name(&file.name) {
            bail!("{} is not an SSO cache file", file.name);
        }
        replace_private(&cache.join(&file.name), &file.contents)?;
    }
    Ok(())
}

pub(crate) fn private_dir_all(dir: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    crate::paths::private_dir(dir).with_context(|| format!("restricting {}", dir.display()))
}

/// Writes `contents` beside `path`, `0600` from creation, then renames it over `path`.
pub(crate) fn replace_private(path: &Path, contents: &str) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let name = path
        .file_name()
        .context("a file has a name")?
        .to_string_lossy();
    let temporary = path.with_file_name(format!(".{name}.{}", uuid::Uuid::now_v7()));
    let written = (|| {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written.with_context(|| format!("writing {}", path.display()))
}

/// Runs `gh` with the login shell's environment, `input` on its standard input; its error is the
/// last line it printed.
async fn gh(daemon: &Daemon, args: &[&str], input: Option<&str>) -> anyhow::Result<String> {
    let mut child = tokio::process::Command::new("gh")
        .args(args)
        .env_clear()
        .envs(daemon.shell_env.vars())
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("could not run gh")?;
    if let Some(input) = input {
        let mut stdin = child.stdin.take().context("gh has a standard input")?;
        stdin.write_all(input.as_bytes()).await?;
    }
    let output = child.wait_with_output().await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{}", stderr.trim().lines().last().unwrap_or("gh failed"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn the_aws_config_and_sso_cache_are_read_without_hidden_or_foreign_files() {
        let home = tempfile::tempdir().unwrap();
        let cache = home.path().join(".aws/sso/cache");
        fs::create_dir_all(&cache).unwrap();
        fs::write(
            home.path().join(".aws/config"),
            "[profile p]\nsso_session = s\n",
        )
        .unwrap();
        fs::write(cache.join("b.json"), "{\"accessToken\":\"b\"}").unwrap();
        fs::write(cache.join("a.json"), "{\"accessToken\":\"a\"}").unwrap();
        fs::write(cache.join(".hidden.json"), "{}").unwrap();
        fs::write(cache.join("notes.txt"), "x").unwrap();
        fs::create_dir(cache.join("dir.json")).unwrap();
        fs::write(home.path().join(".aws/credentials"), "[default]\n").unwrap();

        let (config, files) = read_aws(home.path()).unwrap();
        assert_eq!(config.as_deref(), Some("[profile p]\nsso_session = s\n"));
        let names: Vec<_> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["a.json", "b.json"]);
        assert_eq!(files[0].contents, "{\"accessToken\":\"a\"}");
    }

    #[test]
    fn a_mac_without_aws_gives_nothing_and_no_error() {
        let home = tempfile::tempdir().unwrap();
        let (config, files) = read_aws(home.path()).unwrap();
        assert!(config.is_none() && files.is_empty());
    }

    #[test]
    fn the_worker_keeps_the_aws_files_owner_only_and_replaces_its_previous_copies() {
        let home = tempfile::tempdir().unwrap();
        let aws = home.path().join(".aws");
        fs::create_dir_all(aws.join("sso/cache")).unwrap();
        fs::set_permissions(&aws, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(aws.join("config"), "old").unwrap();
        fs::set_permissions(aws.join("config"), fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(aws.join("sso/cache/a.json"), "old").unwrap();
        fs::write(aws.join("sso/cache/own.json"), "the worker's own").unwrap();
        let credentials = WorkerCredentials {
            github: Vec::new(),
            aws_config: Some("new config".into()),
            aws_sso_cache: vec![WorkerAwsSsoFile {
                name: "a.json".into(),
                contents: "new token".into(),
            }],
        };

        install_aws(home.path(), &credentials).unwrap();

        assert_eq!(
            fs::read_to_string(aws.join("config")).unwrap(),
            "new config"
        );
        assert_eq!(
            fs::read_to_string(aws.join("sso/cache/a.json")).unwrap(),
            "new token"
        );
        assert_eq!(
            fs::read_to_string(aws.join("sso/cache/own.json")).unwrap(),
            "the worker's own"
        );
        for path in [&aws, &aws.join("sso"), &aws.join("sso/cache")] {
            assert_eq!(mode(path), 0o700, "{}", path.display());
        }
        for path in [aws.join("config"), aws.join("sso/cache/a.json")] {
            assert_eq!(mode(&path), 0o600, "{}", path.display());
        }
        let leftovers: Vec<_> = fs::read_dir(&aws)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn an_sso_cache_name_reaching_out_of_the_cache_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let credentials = WorkerCredentials {
            aws_sso_cache: vec![WorkerAwsSsoFile {
                name: "../../config.json".into(),
                contents: "x".into(),
            }],
            ..Default::default()
        };
        assert!(install_aws(home.path(), &credentials).is_err());
        assert!(!home.path().join(".aws/config.json").exists());
    }

    #[test]
    fn no_aws_from_the_mac_leaves_the_worker_without_an_aws_folder() {
        let home = tempfile::tempdir().unwrap();
        install_aws(home.path(), &WorkerCredentials::default()).unwrap();
        assert!(!home.path().join(".aws").exists());
    }

    #[test]
    fn debug_output_names_hosts_and_files_but_never_a_token() {
        let credentials = WorkerCredentials {
            github: vec![WorkerGithubToken {
                host: "github.com".into(),
                token: "gho_secret".into(),
            }],
            aws_config: Some("aws_secret".into()),
            aws_sso_cache: vec![WorkerAwsSsoFile {
                name: "a.json".into(),
                contents: "sso_secret".into(),
            }],
        };
        let debug = format!("{credentials:?}");
        assert!(
            debug.contains("github.com") && debug.contains("a.json"),
            "{debug}"
        );
        assert!(!debug.contains("secret"), "{debug}");
    }
}
