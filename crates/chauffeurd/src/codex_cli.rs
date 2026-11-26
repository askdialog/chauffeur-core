//! The Codex CLI chats run, and updating it from Chauffeur (`codex.status`, `codex.update`;
//! decided with Justin 2026-11-26, see docs/adr/update-codex-from-chauffeur.md). OpenAI offers a
//! new model only to a Codex recent enough for it: an older one leaves it out of its catalog, and
//! a chat on that model cannot start. `codex update` updates Codex the way it was installed (npm,
//! Homebrew, its own installer); the daemon runs it with the login shell's environment, the one
//! the sidecar runs `codex` with, so it updates that `codex`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chauffeur_protocol::client::CodexCliStatus;
use chauffeur_protocol::model::AgentKind;
use tokio::process::Command;

use crate::{Daemon, Error, Result};

/// npm's record of Codex's latest release: Codex publishes every release there.
const LATEST_URL: &str = "https://registry.npmjs.org/@openai/codex/latest";
/// How long the latest version read stays good.
const LATEST_TTL: Duration = Duration::from_secs(60 * 60);
/// `codex update` downloads a few hundred MB through npm or Homebrew.
const UPDATE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Lines of `codex update`'s output kept to say why it failed.
const OUTPUT_LINES: usize = 12;

pub struct CodexCli {
    http: reqwest::Client,
    /// Tests serve their own.
    latest_url: Mutex<String>,
    latest: tokio::sync::Mutex<Option<(Instant, Option<String>)>>,
    updating: Mutex<bool>,
    error: Mutex<Option<String>>,
}

impl Default for CodexCli {
    fn default() -> Self {
        CodexCli {
            http: reqwest::Client::new(),
            latest_url: Mutex::new(LATEST_URL.into()),
            latest: tokio::sync::Mutex::new(None),
            updating: Mutex::new(false),
            error: Mutex::new(None),
        }
    }
}

impl CodexCli {
    /// Tests read the latest version from a server of their own.
    pub fn set_latest_url(&self, url: &str) {
        *self.latest_url.lock().unwrap() = url.into();
    }

    pub async fn status(&self, daemon: &Arc<Daemon>) -> CodexCliStatus {
        let env = daemon.shell_env.vars();
        let path = executable(&env);
        let version = match &path {
            Some(path) => version_of(path, &env).await,
            None => None,
        };
        CodexCliStatus {
            path: path.map(|path| path.display().to_string()),
            version,
            latest: self.latest().await,
            updating: *self.updating.lock().unwrap(),
            error: self.error.lock().unwrap().clone(),
        }
    }

    /// Starts `codex update`; `codex.status` follows it.
    pub async fn update(&self, daemon: &Arc<Daemon>) -> Result<CodexCliStatus> {
        let env = daemon.shell_env.vars();
        let path = executable(&env).ok_or_else(|| {
            Error::BadRequest("Codex is not installed: no `codex` on the login shell's PATH".into())
        })?;
        {
            let mut updating = self.updating.lock().unwrap();
            if *updating {
                return Err(Error::Conflict("Codex is already being updated".into()));
            }
            *updating = true;
        }
        *self.error.lock().unwrap() = None;
        let before = version_of(&path, &env).await;
        let task = daemon.clone();
        tokio::spawn(async move {
            let result = run_update(&path, &env).await;
            let cli = &task.codex_cli;
            let result = match result {
                Ok(()) => {
                    let after = executable(&env);
                    let after = match &after {
                        Some(path) => version_of(path, &env).await,
                        None => None,
                    };
                    let latest = cli.latest().await;
                    match (&after, &latest) {
                        (Some(after), Some(latest)) if older(after, latest) => Err(format!(
                            "`codex update` ended, but Codex is still {after} ({latest} is out)"
                        )),
                        _ => {
                            tracing::info!(?before, ?after, "Codex updated");
                            Ok(())
                        }
                    }
                }
                Err(error) => Err(error),
            };
            match result {
                Ok(()) => {
                    // The catalog of the new Codex, and its sessions from the next message.
                    task.models.forget(AgentKind::Codex).await;
                    task.chats.agent_updated(AgentKind::Codex).await;
                }
                Err(error) => {
                    tracing::warn!(%error, "could not update Codex");
                    *cli.error.lock().unwrap() = Some(error);
                }
            }
            *cli.updating.lock().unwrap() = false;
        });
        let mut status = self.status(daemon).await;
        status.updating = true;
        Ok(status)
    }

    /// The latest version published, read at most hourly; none when npm could not say.
    async fn latest(&self) -> Option<String> {
        let mut cached = self.latest.lock().await;
        if let Some((at, latest)) = cached.as_ref()
            && at.elapsed() < LATEST_TTL
        {
            return latest.clone();
        }
        let url = self.latest_url.lock().unwrap().clone();
        let read = async {
            let response = self
                .http
                .get(&url)
                .timeout(Duration::from_secs(5))
                .send()
                .await?
                .error_for_status()?;
            let body: serde_json::Value = response.json().await?;
            anyhow::Ok(body["version"].as_str().map(str::to_string))
        };
        let latest = match read.await {
            Ok(latest) => latest,
            Err(error) => {
                tracing::debug!(%error, "could not read Codex's latest version");
                None
            }
        };
        // A failure is asked again at the next status, not kept for an hour.
        if latest.is_some() {
            *cached = Some((Instant::now(), latest.clone()));
        }
        latest
    }
}

/// The `codex` the sidecar runs: the first on the login shell's PATH (its environment drops
/// `CHAUFFEUR_*`, so `CHAUFFEUR_CODEX_PATH` never reaches it from there).
fn executable(env: &BTreeMap<String, String>) -> Option<PathBuf> {
    crate::github::find(env, "codex")
}

/// `codex --version` → `0.167.0`.
async fn version_of(path: &PathBuf, env: &BTreeMap<String, String>) -> Option<String> {
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new(path)
            .arg("--version")
            .env_clear()
            .envs(env)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_version(&String::from_utf8_lossy(&output.stdout))
}

/// The version in `codex-cli 0.167.0`.
fn parse_version(text: &str) -> Option<String> {
    text.split_whitespace()
        .find(|word| word.split('.').count() >= 2 && word.starts_with(|c: char| c.is_ascii_digit()))
        .map(str::to_string)
}

/// Whether version `a` comes before `b`, by their numbers (`0.99.0` before `0.167.0`).
fn older(a: &str, b: &str) -> bool {
    let numbers = |v: &str| -> Vec<u64> {
        v.split(['.', '-', '+'])
            .map_while(|part| part.parse().ok())
            .collect()
    };
    numbers(a) < numbers(b)
}

async fn run_update(
    path: &PathBuf,
    env: &BTreeMap<String, String>,
) -> std::result::Result<(), String> {
    let child = Command::new(path)
        .arg("update")
        .env_clear()
        .envs(env)
        // Its installer asks nothing then; nothing can answer a question anyway.
        .env("CODEX_NON_INTERACTIVE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("could not run `codex update`: {error}"))?;
    let output = tokio::time::timeout(UPDATE_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| "`codex update` did not finish in 15 minutes".to_string())?
        .map_err(|error| format!("`codex update` failed: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let tail = lines[lines.len().saturating_sub(OUTPUT_LINES)..].join("\n");
    Err(format!(
        "`codex update` failed ({}):\n{tail}",
        output.status
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_version_codex_prints() {
        assert_eq!(
            parse_version("codex-cli 0.167.0\n").as_deref(),
            Some("0.167.0")
        );
        assert_eq!(parse_version("codex-cli\n"), None);
    }

    #[test]
    fn compares_versions_by_their_numbers() {
        assert!(older("0.167.0", "0.172.0"));
        assert!(older("0.99.0", "0.167.0"));
        assert!(!older("0.172.0", "0.172.0"));
        assert!(!older("0.173.0", "0.172.0"));
        assert!(older("0.172.0-alpha.1", "0.172.1"));
    }
}
