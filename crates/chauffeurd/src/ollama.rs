//! Ollama, which serves the local models (story 52). The daemon finds it, starts `ollama serve`
//! when a local chat, a pull or a delete needs it, and stops the one it started once no model has
//! been loaded for a while. An Ollama someone else started is used as is and never stopped.
//! The models are read from Ollama's manifests on disk, so listing them starts nothing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, Weak};
use std::time::{Duration, Instant};

use anyhow::Context;
use chauffeur_protocol::client::{
    OllamaInstalling, OllamaModel, OllamaPull, OllamaRunning, OllamaStatus,
};
use chauffeur_protocol::model::AgentModel;
use serde::Deserialize;
use tokio::process::{Child, Command};

use crate::{Daemon, Error, Result};

/// A chat's model that Ollama serves is `ollama/<name>`.
pub const MODEL_PREFIX: &str = "ollama/";
/// The context Ollama gives a model; the sidecar tells Codex the same window. Codex's prompt alone
/// is about 10,000 tokens, which Ollama's default context would truncate.
pub const CONTEXT_LENGTH: u32 = 65_536;
/// Where Codex's built-in Ollama provider looks for it.
const ADDRESS: &str = "127.0.0.1:11434";
const START_TIMEOUT: Duration = Duration::from_secs(20);
const PING_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_REGISTRY: &str = "registry.ollama.ai";
/// The Ollama Chauffeur installs: an upgrade changes it and the checksums of `release_for` together.
pub const VERSION: &str = "0.40.11";
const RELEASES: &str = "https://github.com/ollama/ollama/releases/download";
const DEFAULT_NAMESPACE: &str = "library";
/// The context of an embedding model: search's pieces of text are about 300 tokens.
const EMBEDDING_CONTEXT: u32 = 2048;

pub fn is_local(model: &str) -> bool {
    model.starts_with(MODEL_PREFIX)
}

/// Where a local model's Codex keeps its config and threads, apart from the user's `~/.codex`:
/// with the user's config, Codex brings every MCP server, plugin and app, whose tools it puts in
/// the prompt of a model it has no metadata for (about 256,000 tokens), past a local model's
/// context. Made here, the user's alone.
pub fn codex_home(state: &Path) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let dir = state.join("codex-local");
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

/// The Codex environment of a chat on `model`: its own `CODEX_HOME` when the model is local.
pub fn codex_env(
    state: &Path,
    agent: chauffeur_protocol::model::AgentKind,
    model: Option<&str>,
    env: &mut BTreeMap<String, String>,
) -> Result<()> {
    if agent == chauffeur_protocol::model::AgentKind::Codex && model.is_some_and(is_local) {
        env.insert(
            "CODEX_HOME".into(),
            codex_home(state)?.to_string_lossy().into_owned(),
        );
    }
    Ok(())
}

/// One of Ollama's release archives for `VERSION`, as GitHub publishes it.
#[derive(Clone, Debug)]
pub struct Release {
    pub name: String,
    pub sha256: String,
    pub size: u64,
    /// `.tar.zst` (Linux), else `.tgz` (macOS).
    pub zstd: bool,
}

/// The archive for this system; none where Chauffeur does not install Ollama.
fn release_for(os: &str, arch: &str) -> Option<Release> {
    let (name, sha256, size, zstd) = match (os, arch) {
        ("macos", _) => (
            "ollama-darwin.tgz",
            "aea7d3f2e32371b7644a6e2ec8d4f26820a0e3965918cce8a46cef3e947f6291",
            181_324_349,
            false,
        ),
        ("linux", "x86_64") => (
            "ollama-linux-amd64.tar.zst",
            "7bedab90dd122661be78798e9f0edf37fccf0cccf2699d2c58ea1a3afdc04b6d",
            1_508_659_610,
            true,
        ),
        ("linux", "aarch64") => (
            "ollama-linux-arm64.tar.zst",
            "77b278837b9e62c172f77d703675be4c52c8054e9075639031a9a6d5a53f1924",
            1_564_235_434,
            true,
        ),
        _ => return None,
    };
    Some(Release {
        name: name.into(),
        sha256: sha256.into(),
        size,
        zstd,
    })
}

/// Chauffeur's own Ollama, when installed: the macOS archive holds it at its root, Linux's in `bin/`.
fn own_executable(state: &Path) -> Option<PathBuf> {
    let dir = state.join("ollama").join(VERSION);
    [dir.join("ollama"), dir.join("bin").join("ollama")]
        .into_iter()
        .find(|path| is_executable(path))
}

/// The command that installs Ollama by hand on this system.
pub fn install_command() -> &'static str {
    if cfg!(target_os = "macos") {
        "brew install ollama"
    } else {
        "curl -fsSL https://ollama.com/install.sh | sh"
    }
}

pub struct Ollama {
    daemon: Weak<Daemon>,
    http: reqwest::Client,
    /// Tests point these elsewhere.
    address: Mutex<String>,
    binary: Mutex<Option<PathBuf>>,
    models: Mutex<Option<PathBuf>>,
    /// The `ollama serve` the daemon started.
    child: tokio::sync::Mutex<Option<Child>>,
    pulls: Mutex<BTreeMap<String, OllamaPull>>,
    installing: Mutex<Option<OllamaInstalling>>,
    /// Where releases are downloaded from, and the archive to take; tests serve their own.
    releases: Mutex<(String, Option<Release>)>,
    /// Since when the daemon's Ollama has had no model loaded and no pull running.
    unused_since: Mutex<Option<Instant>>,
}

impl Ollama {
    pub fn new(daemon: Weak<Daemon>) -> Self {
        Ollama {
            daemon,
            http: reqwest::Client::new(),
            address: Mutex::new(ADDRESS.into()),
            binary: Mutex::new(None),
            models: Mutex::new(None),
            child: tokio::sync::Mutex::new(None),
            pulls: Mutex::new(BTreeMap::new()),
            installing: Mutex::new(None),
            releases: Mutex::new((
                RELEASES.into(),
                release_for(std::env::consts::OS, std::env::consts::ARCH),
            )),
            unused_since: Mutex::new(None),
        }
    }

    /// Tests run a fake Ollama at another address, with its models in a folder of theirs.
    pub fn use_binary(&self, binary: PathBuf, address: &str, models: PathBuf) {
        *self.binary.lock().unwrap() = Some(binary);
        *self.address.lock().unwrap() = address.into();
        *self.models.lock().unwrap() = Some(models);
    }

    /// Tests serve a release of their own.
    pub fn use_release(&self, base: &str, release: Release) {
        *self.releases.lock().unwrap() = (base.into(), Some(release));
    }

    fn d(&self) -> std::sync::Arc<Daemon> {
        self.daemon.upgrade().expect("daemon alive")
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address.lock().unwrap())
    }

    fn executable(&self, env: &BTreeMap<String, String>) -> Option<PathBuf> {
        if let Some(binary) = self.binary.lock().unwrap().clone() {
            return Some(binary);
        }
        own_executable(&self.d().paths.home).or_else(|| find(env))
    }

    fn models_dir(&self, env: &BTreeMap<String, String>) -> PathBuf {
        if let Some(dir) = self.models.lock().unwrap().clone() {
            return dir;
        }
        models_dir(env)
    }

    /// The models on disk, as Codex's catalog lists them.
    pub fn agent_models(&self) -> Vec<AgentModel> {
        let env = self.d().shell_env.vars();
        models_on_disk(&self.models_dir(&env))
            .into_iter()
            // Search's embedding models give vectors, not answers.
            .filter(|model| !model.name.contains("embed"))
            .map(|model| AgentModel {
                id: format!("{MODEL_PREFIX}{}", model.name),
                resolved: None,
                name: format!("{} (local)", model.name),
                description: format!(
                    "Runs on this machine through Ollama ({}): slower and weaker than a hosted model.",
                    size_label(model.size)
                ),
                efforts: Vec::new(),
                default_effort: None,
                supports_fast: false,
                is_default: false,
            })
            .collect()
    }

    pub async fn status(&self) -> OllamaStatus {
        let env = self.d().shell_env.vars();
        let running = if self.ours_alive().await {
            OllamaRunning::Daemon
        } else if self.answers().await {
            OllamaRunning::External
        } else {
            OllamaRunning::Stopped
        };
        let executable = self.executable(&env);
        let own = executable.is_some() && executable == own_executable(&self.d().paths.home);
        OllamaStatus {
            executable: executable.map(|path| path.display().to_string()),
            own,
            version: VERSION.into(),
            download_size: self.releases.lock().unwrap().1.as_ref().map(|r| r.size),
            install: install_command().into(),
            installing: self.installing.lock().unwrap().clone(),
            running,
            models: models_on_disk(&self.models_dir(&env)),
            pulls: self.pulls.lock().unwrap().values().cloned().collect(),
        }
    }

    /// Whether `model` is on disk (`name` and `name:latest` are the same).
    pub fn has_model(&self, model: &str) -> bool {
        let env = self.d().shell_env.vars();
        let model = model.strip_suffix(":latest").unwrap_or(model);
        models_on_disk(&self.models_dir(&env))
            .iter()
            .any(|known| known.name == model)
    }

    /// Whether an Ollama can be run: found, or Chauffeur's own.
    pub fn installed(&self) -> bool {
        self.executable(&self.d().shell_env.vars()).is_some()
    }

    /// The vectors of `inputs` from an embedding model, through an Ollama that answers already
    /// (`ensure` first). `keep_alive` is how long Ollama keeps the model loaded after it: once
    /// unloaded, the Ollama the daemon started can stop as unused.
    pub async fn embed(
        &self,
        model: &str,
        inputs: &[String],
        keep_alive: &str,
        timeout: Duration,
    ) -> Result<Vec<Vec<f32>>> {
        #[derive(Deserialize)]
        struct Embeddings {
            #[serde(default)]
            embeddings: Vec<Vec<f32>>,
            #[serde(default)]
            error: Option<String>,
        }
        let response = self
            .http
            .post(self.url("/api/embed"))
            .timeout(timeout)
            .json(&serde_json::json!({
                "model": model,
                "input": inputs,
                "keep_alive": keep_alive,
                // embeddinggemma reads 2,048 tokens at most; a larger context only takes memory.
                "options": { "num_ctx": EMBEDDING_CONTEXT },
            }))
            .send()
            .await
            .map_err(|error| Error::Unavailable(format!("Ollama did not answer: {error}")))?;
        let status = response.status();
        let body: Embeddings = response.json().await.map_err(|error| {
            Error::Unavailable(format!("Ollama's answer was unreadable: {error}"))
        })?;
        if let Some(error) = body.error.filter(|_| !status.is_success()) {
            return Err(Error::Unavailable(format!(
                "Ollama could not embed: {error}"
            )));
        }
        if body.embeddings.len() != inputs.len() {
            return Err(Error::Unavailable(format!(
                "Ollama gave {} vectors for {} texts",
                body.embeddings.len(),
                inputs.len()
            )));
        }
        Ok(body.embeddings)
    }

    /// Whether an Ollama, the daemon's or another, answers.
    pub async fn answers(&self) -> bool {
        self.http
            .get(self.url("/api/version"))
            .timeout(PING_TIMEOUT)
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
    }

    async fn ours_alive(&self) -> bool {
        let mut child = self.child.lock().await;
        match child.as_mut().map(|child| child.try_wait()) {
            Some(Ok(None)) => true,
            Some(_) => {
                *child = None;
                false
            }
            None => false,
        }
    }

    /// Makes sure an Ollama answers, starting one when none does. Waits for it.
    pub async fn ensure(&self) -> Result<()> {
        *self.unused_since.lock().unwrap() = None;
        let mut child = self.child.lock().await;
        let ours = matches!(child.as_mut().map(|child| child.try_wait()), Some(Ok(None)));
        if !ours {
            *child = None;
            if self.answers().await {
                return Ok(());
            }
            *child = Some(self.spawn()?);
        }
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if self.answers().await {
                return Ok(());
            }
            let exited = child
                .as_mut()
                .is_none_or(|child| !matches!(child.try_wait(), Ok(None)));
            if exited || Instant::now() >= deadline {
                if let Some(child) = child.take() {
                    stop(child).await;
                }
                let log = self.d().paths.logs_dir.join("ollama.log");
                return Err(Error::Unavailable(format!(
                    "Ollama did not start: see {} for why.",
                    log.display()
                )));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    fn spawn(&self) -> Result<Child> {
        let daemon = self.d();
        let env = daemon.shell_env.vars();
        let binary = self.executable(&env).ok_or_else(|| {
            Error::Unavailable(
                "Ollama is not installed: install it from Settings › Local models, or with `chauffeur ollama install`, then try again."
                    .into(),
            )
        })?;
        let log = crate::paths::open_private_log(&daemon.paths.logs_dir.join("ollama.log"))?;
        let address = self.address.lock().unwrap().clone();
        let models = self.models_dir(&env);
        let child = Command::new(&binary)
            .arg("serve")
            .current_dir(&daemon.paths.home)
            .env_clear()
            .envs(env)
            .env("OLLAMA_MODELS", models)
            .env("OLLAMA_HOST", address)
            .env("OLLAMA_CONTEXT_LENGTH", CONTEXT_LENGTH.to_string())
            // A start must not delete a download left unfinished.
            .env("OLLAMA_NOPRUNE", "1")
            .env("OLLAMA_FLASH_ATTENTION", "1")
            .env("OLLAMA_KV_CACHE_TYPE", "q8_0")
            .env("OLLAMA_KEEP_ALIVE", "30m")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("starting {} serve", binary.display()))?;
        tracing::info!(binary = %binary.display(), "ollama started");
        Ok(child)
    }

    /// Stops the Ollama the daemon started once it has had no model loaded and no pull running
    /// for `after`.
    pub async fn stop_if_unused(&self, after: Duration) {
        if !self.ours_alive().await {
            return;
        }
        let pulling = self
            .pulls
            .lock()
            .unwrap()
            .values()
            .any(|pull| pull.error.is_none());
        if pulling || self.models_loaded().await {
            *self.unused_since.lock().unwrap() = None;
            return;
        }
        let since = *self
            .unused_since
            .lock()
            .unwrap()
            .get_or_insert_with(Instant::now);
        if since.elapsed() >= after {
            self.shutdown().await;
            tracing::info!("ollama stopped, unused");
        }
    }

    async fn models_loaded(&self) -> bool {
        #[derive(Deserialize)]
        struct Loaded {
            #[serde(default)]
            models: Vec<serde_json::Value>,
        }
        let Ok(response) = self
            .http
            .get(self.url("/api/ps"))
            .timeout(PING_TIMEOUT)
            .send()
            .await
        else {
            return false;
        };
        response
            .json::<Loaded>()
            .await
            .is_ok_and(|loaded| !loaded.models.is_empty())
    }

    /// Stops the Ollama the daemon started, if any.
    pub async fn shutdown(&self) {
        if let Some(child) = self.child.lock().await.take() {
            stop(child).await;
        }
        *self.unused_since.lock().unwrap() = None;
    }

    /// Starts pulling `model` in the background; `status` follows it.
    pub fn pull(&self, model: &str) -> Result<()> {
        check_name(model)?;
        {
            let mut pulls = self.pulls.lock().unwrap();
            if pulls.get(model).is_some_and(|pull| pull.error.is_none()) {
                return Err(Error::Conflict(format!(
                    "{model} is already being downloaded"
                )));
            }
            pulls.insert(
                model.into(),
                OllamaPull {
                    model: model.into(),
                    status: "starting Ollama".into(),
                    completed: 0,
                    total: 0,
                    error: None,
                },
            );
        }
        let daemon = self.d();
        let model = model.to_string();
        tokio::spawn(async move {
            let result = daemon.ollama.run_pull(&model).await;
            let mut pulls = daemon.ollama.pulls.lock().unwrap();
            match result {
                Ok(()) => {
                    pulls.remove(&model);
                }
                Err(error) => {
                    tracing::warn!(%error, model, "could not pull the model");
                    if let Some(pull) = pulls.get_mut(&model) {
                        pull.error = Some(format!("{error:#}"));
                    }
                }
            }
        });
        Ok(())
    }

    async fn run_pull(&self, model: &str) -> anyhow::Result<()> {
        self.ensure().await?;
        let mut response = self
            .http
            .post(self.url("/api/pull"))
            .json(&serde_json::json!({ "model": model, "stream": true }))
            .send()
            .await
            .context("asking Ollama to download the model")?
            .error_for_status()?;
        let mut pending = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            pending.extend_from_slice(&chunk);
            while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = pending.drain(..=end).collect();
                self.progress(model, &line)?;
            }
        }
        self.progress(model, &pending)
    }

    /// One line of `/api/pull`'s answer: progress, or the error that ends it.
    fn progress(&self, model: &str, line: &[u8]) -> anyhow::Result<()> {
        #[derive(Deserialize)]
        struct Progress {
            #[serde(default)]
            status: String,
            #[serde(default)]
            completed: u64,
            #[serde(default)]
            total: u64,
            error: Option<String>,
        }
        if line.trim_ascii().is_empty() {
            return Ok(());
        }
        let progress: Progress =
            serde_json::from_slice(line).context("reading Ollama's progress")?;
        if let Some(error) = progress.error {
            anyhow::bail!(error);
        }
        if let Some(pull) = self.pulls.lock().unwrap().get_mut(model) {
            pull.status = progress.status;
            pull.completed = progress.completed;
            pull.total = progress.total;
        }
        Ok(())
    }

    /// Starts installing Chauffeur's own Ollama in the background; `status` follows it.
    pub fn install(&self) -> Result<()> {
        let release = self.releases.lock().unwrap().1.clone().ok_or_else(|| {
            Error::BadRequest(format!(
                "Chauffeur installs Ollama on macOS and on Linux (x86_64, arm64) only: install it with `{}`",
                install_command()
            ))
        })?;
        {
            let mut installing = self.installing.lock().unwrap();
            if installing.as_ref().is_some_and(|step| step.error.is_none()) {
                return Err(Error::Conflict("Ollama is already being installed".into()));
            }
            *installing = Some(OllamaInstalling {
                phase: "downloading".into(),
                completed: 0,
                total: release.size,
                error: None,
            });
        }
        let daemon = self.d();
        tokio::spawn(async move {
            let result = daemon.ollama.run_install(&release).await;
            let mut installing = daemon.ollama.installing.lock().unwrap();
            match result {
                Ok(()) => *installing = None,
                Err(error) => {
                    tracing::warn!(%error, "could not install ollama");
                    if let Some(step) = installing.as_mut() {
                        step.error = Some(format!("{error:#}"));
                    }
                }
            }
        });
        Ok(())
    }

    fn installing(&self, phase: &str, completed: u64, total: u64) {
        if let Some(step) = self.installing.lock().unwrap().as_mut() {
            step.phase = phase.into();
            step.completed = completed;
            step.total = total;
        }
    }

    async fn run_install(&self, release: &Release) -> anyhow::Result<()> {
        use sha2::Digest;
        use tokio::io::AsyncWriteExt;
        let root = self.d().paths.home.join("ollama");
        std::fs::create_dir_all(&root)?;
        let download = root.join(format!(".download-{}", release.name));
        let url = format!(
            "{}/v{VERSION}/{}",
            self.releases.lock().unwrap().0,
            release.name
        );
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
            self.installing("downloading", completed, total);
        }
        file.flush().await?;
        drop(file);
        self.installing("checking", completed, total);
        if hex::encode(hasher.finalize()) != release.sha256 {
            let _ = std::fs::remove_file(&download);
            anyhow::bail!(
                "the download of {} does not match its published checksum: try again",
                release.name
            );
        }
        self.installing("unpacking", completed, total);
        let unpacked = {
            let (download, root, zstd) = (download.clone(), root.clone(), release.zstd);
            tokio::task::spawn_blocking(move || unpack(&download, zstd, &root)).await??
        };
        let _ = std::fs::remove_file(&download);
        // The Ollama the daemon runs may be the copy being replaced.
        self.shutdown().await;
        let dir = root.join(VERSION);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        std::fs::rename(&unpacked, &dir)?;
        for (name, path) in entries(&root, true) {
            if name != VERSION {
                let _ = std::fs::remove_dir_all(path);
            }
        }
        tracing::info!(version = VERSION, dir = %dir.display(), "ollama installed");
        Ok(())
    }

    pub async fn delete(&self, model: &str) -> Result<()> {
        check_name(model)?;
        self.ensure().await?;
        let response = self
            .http
            .delete(self.url("/api/delete"))
            .json(&serde_json::json!({ "model": model }))
            .send()
            .await
            .context("asking Ollama to delete the model")?;
        match response.status() {
            status if status.is_success() => Ok(()),
            reqwest::StatusCode::NOT_FOUND => {
                Err(Error::NotFound(format!("Ollama has no model {model}")))
            }
            status => Err(anyhow::anyhow!("Ollama answered {status} to deleting {model}").into()),
        }
    }
}

/// Stops an `ollama serve` and what it started (its runners), giving it a moment to unload.
async fn stop(mut child: Child) {
    if let Some(pid) = child.id().and_then(|pid| i32::try_from(pid).ok()) {
        // SAFETY: a signal to the process group the daemon gave this child.
        unsafe { libc::kill(-pid, libc::SIGTERM) };
    }
    if tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
    }
}

/// Unpacks a release archive into a new folder of `root` and returns it, an `ollama` executable
/// in it; a failure leaves nothing. Entries that would land outside it are skipped.
fn unpack(archive: &Path, zstd: bool, root: &Path) -> anyhow::Result<PathBuf> {
    let dir = tempfile::Builder::new()
        .prefix(".unpack-")
        .tempdir_in(root)?;
    let file = std::fs::File::open(archive)?;
    let reader: Box<dyn std::io::Read> = if zstd {
        Box::new(zstd::Decoder::new(file)?)
    } else {
        Box::new(flate2::read::GzDecoder::new(file))
    };
    let mut tar = tar::Archive::new(reader);
    tar.set_preserve_permissions(true);
    tar.unpack(dir.path())
        .context("unpacking Ollama's archive")?;
    if !["ollama", "bin/ollama"]
        .iter()
        .any(|path| is_executable(&dir.path().join(path)))
    {
        anyhow::bail!("Ollama's archive holds no `ollama` executable");
    }
    Ok(dir.keep())
}

/// The `ollama` executable: on the PATH, else where Homebrew and Ollama's app put it.
fn find(env: &BTreeMap<String, String>) -> Option<PathBuf> {
    let on_path: Vec<PathBuf> = env
        .get("PATH")
        .map(|path| {
            std::env::split_paths(path)
                .map(|dir| dir.join("ollama"))
                .collect()
        })
        .unwrap_or_default();
    let known = [
        "/opt/homebrew/bin/ollama",
        "/usr/local/bin/ollama",
        "/Applications/Ollama.app/Contents/Resources/ollama",
    ]
    .map(PathBuf::from);
    on_path
        .into_iter()
        .chain(known)
        .find(|path| is_executable(path))
}

/// Where Ollama keeps its models: `$OLLAMA_MODELS`, else `~/.ollama/models`.
fn models_dir(env: &BTreeMap<String, String>) -> PathBuf {
    if let Some(dir) = env.get("OLLAMA_MODELS").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    let home = env
        .get("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_default();
    home.join(".ollama").join("models")
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// A model name as Ollama takes it: letters, digits and `._:-/`.
pub fn check_name(model: &str) -> Result<()> {
    let valid = !model.is_empty()
        && model.len() <= 200
        && !model.starts_with(['-', '/', '.'])
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._:-/".contains(c));
    if valid {
        Ok(())
    } else {
        Err(Error::BadRequest(format!(
            "`{model}` is not a model name: letters, digits and `._:-/` only, as in `qwen3.8`"
        )))
    }
}

/// The models whose manifests Ollama keeps under `<dir>/manifests/<registry>/<namespace>/<name>/<tag>`.
fn models_on_disk(dir: &Path) -> Vec<OllamaModel> {
    let mut models = Vec::new();
    for (registry, registry_dir) in entries(&dir.join("manifests"), true) {
        for (namespace, namespace_dir) in entries(&registry_dir, true) {
            for (name, name_dir) in entries(&namespace_dir, true) {
                for (tag, manifest) in entries(&name_dir, false) {
                    if let Some(size) = manifest_size(&manifest) {
                        models.push(OllamaModel {
                            name: model_name(&registry, &namespace, &name, &tag),
                            size,
                        });
                    }
                }
            }
        }
    }
    models.sort_by(|a, b| a.name.cmp(&b.name));
    models
}

/// The folders (or files) of `dir` by name, dotfiles left out.
fn entries(dir: &Path, folders: bool) -> Vec<(String, PathBuf)> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    read.flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir() == folders))
        .filter_map(|entry| Some((entry.file_name().into_string().ok()?, entry.path())))
        .filter(|(name, _)| !name.starts_with('.'))
        .collect()
}

fn manifest_size(path: &Path) -> Option<u64> {
    #[derive(Deserialize)]
    struct Manifest {
        layers: Vec<Layer>,
    }
    #[derive(Deserialize)]
    struct Layer {
        size: u64,
    }
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    Some(manifest.layers.iter().map(|layer| layer.size).sum())
}

/// As `ollama list` names a model, without the `latest` tag.
fn model_name(registry: &str, namespace: &str, name: &str, tag: &str) -> String {
    let mut model = match (registry == DEFAULT_REGISTRY, namespace == DEFAULT_NAMESPACE) {
        (true, true) => name.to_string(),
        (true, false) => format!("{namespace}/{name}"),
        (false, _) => format!("{registry}/{namespace}/{name}"),
    };
    if tag != "latest" {
        model.push(':');
        model.push_str(tag);
    }
    model
}

fn size_label(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(dir: &Path, path: &str, sizes: &[u64]) {
        let file = dir.join("manifests").join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let layers: Vec<_> = sizes
            .iter()
            .map(|size| serde_json::json!({ "digest": "sha256:x", "size": size }))
            .collect();
        std::fs::write(file, serde_json::json!({ "layers": layers }).to_string()).unwrap();
    }

    #[test]
    fn models_on_disk_are_named_as_ollama_names_them_and_sized_by_their_layers() {
        let dir = tempfile::tempdir().unwrap();
        manifest(
            dir.path(),
            "registry.ollama.ai/library/qwen3.8/latest",
            &[10, 5],
        );
        manifest(
            dir.path(),
            "registry.ollama.ai/library/glm-4.7-flash/q8_0",
            &[7],
        );
        manifest(
            dir.path(),
            "registry.ollama.ai/rafw007/glm-opencode/latest",
            &[3],
        );
        manifest(dir.path(), "hf.co/unsloth/devstral/Q4_K_M", &[2]);
        std::fs::write(
            dir.path()
                .join("manifests/registry.ollama.ai/library/.DS_Store"),
            "",
        )
        .unwrap();
        let broken = dir
            .path()
            .join("manifests/registry.ollama.ai/library/broken/latest");
        std::fs::create_dir_all(broken.parent().unwrap()).unwrap();
        std::fs::write(broken, "not json").unwrap();

        let models: Vec<_> = models_on_disk(dir.path())
            .into_iter()
            .map(|model| (model.name, model.size))
            .collect();
        assert_eq!(
            models,
            [
                ("glm-4.7-flash:q8_0".to_string(), 7),
                ("hf.co/unsloth/devstral:Q4_K_M".to_string(), 2),
                ("qwen3.8".to_string(), 15),
                ("rafw007/glm-opencode".to_string(), 3),
            ]
        );
    }

    #[test]
    fn no_models_folder_lists_no_models() {
        let dir = tempfile::tempdir().unwrap();
        assert!(models_on_disk(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn the_models_folder_is_ollamas_own_unless_ollama_models_names_one() {
        let mut env = BTreeMap::from([("HOME".to_string(), "/Users/me".to_string())]);
        assert_eq!(models_dir(&env), PathBuf::from("/Users/me/.ollama/models"));
        env.insert("OLLAMA_MODELS".into(), "/Volumes/big/models".into());
        assert_eq!(models_dir(&env), PathBuf::from("/Volumes/big/models"));
    }

    #[test]
    fn ollama_is_found_on_the_path_before_the_known_places() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("ollama");
        std::fs::write(&binary, "#!/bin/sh\n").unwrap();
        let env = BTreeMap::from([("PATH".to_string(), dir.path().display().to_string())]);
        // Not executable: not Ollama.
        assert_ne!(find(&env), Some(binary.clone()));
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(find(&env), Some(binary));
    }

    #[test]
    fn model_names_take_ollamas_characters_only() {
        for good in [
            "qwen3.8",
            "glm-4.7-flash:q8_0",
            "rafw007/glm-opencode",
            "hf.co/unsloth/devstral:Q4_K_M",
        ] {
            assert!(check_name(good).is_ok(), "{good}");
        }
        for bad in ["", "-rm", "/etc", "../x", "qwen 3", "qwen;rm", "a\"b"] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
    }

    /// A release archive as Ollama ships it: `files` at their paths, `links` as symlinks.
    pub(crate) fn archive(
        zstd: bool,
        files: &[(&str, &[u8], u32)],
        links: &[(&str, &str)],
    ) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, body, mode) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(*mode);
            header.set_cksum();
            builder.append_data(&mut header, path, *body).unwrap();
        }
        for (path, target) in links {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            builder.append_link(&mut header, path, target).unwrap();
        }
        let tar = builder.into_inner().unwrap();
        if zstd {
            zstd::encode_all(&tar[..], 0).unwrap()
        } else {
            use std::io::Write;
            let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            gz.write_all(&tar).unwrap();
            gz.finish().unwrap()
        }
    }

    #[test]
    fn macos_and_linux_archives_unpack_with_their_executable_and_links() {
        let root = tempfile::tempdir().unwrap();
        let mac = root.path().join("mac.tgz");
        std::fs::write(
            &mac,
            archive(
                false,
                &[
                    ("ollama", b"#!/bin/sh\n", 0o755),
                    ("libggml.dylib", b"lib", 0o644),
                ],
                &[],
            ),
        )
        .unwrap();
        let dir = unpack(&mac, false, root.path()).unwrap();
        assert!(is_executable(&dir.join("ollama")));
        assert!(dir.join("libggml.dylib").is_file());

        let linux = root.path().join("linux.tar.zst");
        std::fs::write(
            &linux,
            archive(
                true,
                &[
                    ("bin/ollama", b"#!/bin/sh\n", 0o755),
                    ("lib/ollama/libcudart.so.12.8", b"lib", 0o644),
                ],
                &[("lib/ollama/libcudart.so.12", "libcudart.so.12.8")],
            ),
        )
        .unwrap();
        let dir = unpack(&linux, true, root.path()).unwrap();
        assert!(is_executable(&dir.join("bin/ollama")));
        assert_eq!(
            std::fs::read(dir.join("lib/ollama/libcudart.so.12")).unwrap(),
            b"lib"
        );
    }

    #[test]
    fn an_archive_without_ollama_leaves_nothing() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("other.tgz");
        std::fs::write(&path, archive(false, &[("README", b"hi", 0o644)], &[])).unwrap();
        assert!(unpack(&path, false, root.path()).is_err());
        assert_eq!(entries(root.path(), true), []);
    }

    #[test]
    fn chauffeur_installs_ollama_on_macos_and_linux_only() {
        assert!(
            release_for("macos", "aarch64")
                .is_some_and(|r| r.name == "ollama-darwin.tgz" && !r.zstd)
        );
        assert!(release_for("macos", "x86_64").is_some());
        assert!(
            release_for("linux", "x86_64")
                .is_some_and(|r| r.name == "ollama-linux-amd64.tar.zst" && r.zstd)
        );
        assert!(
            release_for("linux", "aarch64").is_some_and(|r| r.name == "ollama-linux-arm64.tar.zst")
        );
        assert!(release_for("linux", "riscv64").is_none());
        assert!(release_for("windows", "x86_64").is_none());
    }

    #[test]
    fn a_local_model_is_one_ollama_serves() {
        assert!(is_local("ollama/qwen3.8"));
        assert!(!is_local("gpt-6.1-sol"));
        assert!(!is_local("qwen3.8"));
    }
}
