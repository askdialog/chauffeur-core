//! Daemon-owned package journal and independent process lifetime. A view pins its UI revision;
//! an admitted operation outlives a disconnected client and finishes before a version changes.
use super::{
    Registration, Runtime, download,
    packages::{self, Catalogue, Installed, SignedRelease},
    process::Process,
    services::Broker,
};
use crate::{Daemon, Error, Result, paths::Paths, store::Store};
use chauffeur_extension_api::{HOST_API_VERSION, Init};
use chauffeur_protocol::{client::ClientCall, events::RegistryEvent, extensions::*};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    any::Any,
    path::PathBuf,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Journal {
    epoch: u64,
    removed: bool,
    current: Option<Installed>,
    previous: Option<Installed>,
    pending: Option<Installed>,
    manual: bool,
    rejected: Option<String>,
    switching: bool,
}

struct State {
    journal: Journal,
    available: Option<SignedRelease>,
    phase: ExtensionPackagePhase,
    error: Option<String>,
}

struct Inner {
    daemon: Weak<Daemon>,
    catalogue: Catalogue,
    bootstrap: PathBuf,
    home: PathBuf,
    store: Store,
    state: Mutex<State>,
    process: Mutex<Option<Arc<Process>>>,
    operations: Arc<tokio::sync::RwLock<()>>,
    lifecycle: tokio::sync::Mutex<()>,
    views: AtomicU32,
    viewers: AtomicU32,
    started: AtomicBool,
    running_job: AtomicBool,
    /// When the current engine exited on its own lately (`supervise`).
    crashes: Mutex<Vec<Instant>>,
    /// Core activity (chats, turns, steps, PRs read) no engine received yet, in order: an
    /// Office earns rewards from it and cannot rebuild it, so a version switch only delays it.
    activity: Mutex<std::collections::VecDeque<Value>>,
    nonce: String,
}

/// The activity kept while no engine runs; the oldest is dropped past it. A starting engine gets
/// it all at once, and engines released with host API 1 up to Office 0.30.0 quit when more than
/// 128 requests wait on them: this stays well below, beside the notifications sent with it.
const MAX_ACTIVITY: usize = 64;

pub struct Managed(Arc<Inner>);

pub fn registrations(bootstrap: PathBuf) -> Vec<Registration> {
    [
        (
            ExtensionId::Database,
            "Database",
            "Explore PostgreSQL tables, run SQL and bring results into a chat.",
            "askdialog/chauffeur-extension-database",
            *include_bytes!("database.pub"),
        ),
        (
            ExtensionId::Office,
            "Office",
            "Meet colleagues and see your team's agents in a 3D village.",
            "askdialog/chauffeur-extension-office",
            *include_bytes!("office.pub"),
        ),
    ]
    .into_iter()
    .map(|(id, name, description, repository, public_key)| {
        let bootstrap = bootstrap.clone();
        Registration {
            id,
            name,
            description,
            version: "—",
            capabilities: match id {
                ExtensionId::Database => &[
                    "Workspace Database view",
                    "Protected PostgreSQL connections and queries",
                ],
                ExtensionId::Office => {
                    &["Global 3D Office view", "Team presence, activity and calls"]
                }
            },
            build: Arc::new(move |daemon, paths, store| {
                Arc::new(Managed::new(
                    daemon,
                    paths,
                    store,
                    Catalogue {
                        id,
                        repository,
                        public_key,
                    },
                    bootstrap.clone(),
                ))
            }),
        }
    })
    .collect()
}

impl Managed {
    fn new(
        daemon: Weak<Daemon>,
        paths: &Paths,
        store: Store,
        catalogue: Catalogue,
        bootstrap: PathBuf,
    ) -> Self {
        let key = format!("extension_{}_package", packages::slug(catalogue.id));
        let loaded = store.kv_get(&key).and_then(|value| {
            value
                .map(|value| serde_json::from_str(&value).map_err(Into::into))
                .transpose()
        });
        let (journal, error) = match loaded {
            Ok(value) => (value.unwrap_or_default(), None),
            Err(error) => (
                Journal {
                    removed: true,
                    ..Default::default()
                },
                Some(format!(
                    "Could not read the installed extension journal: {error}; reinstall the extension"
                )),
            ),
        };
        Self(Arc::new(Inner {
            daemon,
            catalogue: catalogue.clone(),
            bootstrap: bootstrap.join(packages::slug(catalogue.id)),
            home: paths
                .home
                .join("extensions")
                .join(packages::slug(catalogue.id)),
            store,
            state: Mutex::new(State {
                journal,
                available: None,
                phase: ExtensionPackagePhase::Missing,
                error,
            }),
            process: Mutex::new(None),
            operations: Arc::new(tokio::sync::RwLock::new(())),
            lifecycle: tokio::sync::Mutex::new(()),
            views: AtomicU32::new(0),
            viewers: AtomicU32::new(0),
            started: AtomicBool::new(false),
            running_job: AtomicBool::new(false),
            crashes: Mutex::new(Vec::new()),
            activity: Mutex::new(std::collections::VecDeque::new()),
            nonce: uuid::Uuid::new_v4().simple().to_string(),
        }))
    }

    pub fn watch(&self) -> Result<Box<dyn Send>> {
        let inner = &self.0;
        let state = inner.state.lock().unwrap();
        if state.journal.removed {
            return Err(Error::Unavailable(
                "This extension was removed; install it in Settings → Extensions".into(),
            ));
        }
        // Nothing to pin before a first install: a view held then would hold that install back
        // forever. The app retries until a version is current.
        if state.phase == ExtensionPackagePhase::Starting || state.journal.current.is_none() {
            return Err(Error::Unavailable(
                "The extension is changing versions; wait for it to finish loading".into(),
            ));
        }
        inner.views.fetch_add(1, Ordering::SeqCst);
        drop(state);
        Ok(Box::new(View {
            inner: inner.clone(),
            office: false,
        }))
    }

    pub fn configure(&self, auto_update: bool) -> Result<()> {
        self.0.store.kv_set(
            &self.0.key("auto_update"),
            if auto_update { "true" } else { "false" },
        )?;
        self.0.publish();
        Ok(())
    }

    pub fn manage(&self, command: ExtensionCommand) -> Result<()> {
        let inner = self.0.clone();
        if matches!(command, ExtensionCommand::Remove) {
            let guard = inner.operations.clone().try_write_owned().map_err(|_| Error::Conflict("The extension is busy; finish or cancel its current operation before removing it".into()))?;
            let mut state = inner.state.lock().unwrap();
            inner.commit(&mut state, |journal| {
                journal.removed = true;
                journal.epoch += 1;
                journal.pending = None;
            })?;
            state.phase = ExtensionPackagePhase::Missing;
            drop(state);
            inner.activity.lock().unwrap().clear();
            // Its background work ends now, not at the next lifecycle tick, which a running
            // download may hold; no switch can start an engine while the guard is held.
            let process = inner.process.lock().unwrap().take();
            if let Some(process) = process {
                process.stop();
                // Its last host calls finish before any call or switch may begin.
                tokio::spawn(async move {
                    let _guard = guard;
                    process.drain_callbacks().await;
                });
            }
            inner.publish();
            return Ok(());
        }
        if inner.running_job.swap(true, Ordering::SeqCst) {
            return Err(Error::Conflict(
                "An extension download is already running; wait for it to finish".into(),
            ));
        }
        let epoch = inner.state.lock().unwrap().journal.epoch;
        tokio::spawn(async move {
            let _job = Job(inner.clone());
            let _serial = inner.lifecycle.lock().await;
            if let Err(error) = inner.command(command, epoch).await {
                inner.fail(error);
            }
        });
        Ok(())
    }

    pub async fn call(
        &self,
        method: String,
        mut params: Value,
        workspace: Option<String>,
    ) -> Result<Value> {
        let inner = self.0.clone();
        let daemon = inner.daemon()?;
        let slug = packages::slug(inner.catalogue.id);
        if !method.starts_with(&format!("{slug}."))
            && !(inner.catalogue.id == ExtensionId::Office && method == "workspace.move_house")
        {
            return Err(Error::BadRequest(
                "This operation belongs to another extension; call its own namespace".into(),
            ));
        }
        let workspace = workspace.or_else(|| {
            params
                .get("workspace")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
        let admitted = inner.catalogue.id == ExtensionId::Database
            && !matches!(
                method.as_str(),
                "database.list" | "database.save" | "database.test" | "database.cancel"
            );
        let admission = if admitted {
            let workspace = workspace.ok_or_else(|| {
                Error::BadRequest(
                    "Database operations require a workspace; choose one first".into(),
                )
            })?;
            let workspace = daemon.workspaces.get(&workspace)?;
            params
                .as_object_mut()
                .ok_or_else(|| Error::BadRequest("Extension parameters must be an object".into()))?
                .insert("workspace".into(), json!(workspace.id));
            Some(daemon.execution.admit(&workspace.id)?)
        } else {
            None
        };
        let lease = inner.operations.clone().read_owned().await;
        if inner.state.lock().unwrap().journal.removed
            || (!super::enabled(&daemon, inner.catalogue.id)? && method != "office.state")
        {
            return Err(Error::Unavailable("This extension is disabled or removed; enable or install it in Settings → Extensions".into()));
        }
        if matches!(
            method.as_str(),
            "office.browser_link" | "office.open_in_browser"
        ) {
            let url = if method == "office.open_in_browser" {
                daemon.extension_web.open_office_in_browser(&daemon).await?
            } else {
                daemon.extension_web.office_link(&daemon).await?
            };
            return crate::rpc::to_value(chauffeur_protocol::office::OfficeBrowserLink { url });
        }
        let process = inner.process.lock().unwrap().clone().filter(|process| process.is_alive()).ok_or_else(|| Error::Unavailable("This extension is not running; install it or restore its previous version in Settings → Extensions".into()))?;
        // The task, rather than the client future, owns admission until the native result or exit.
        tokio::spawn(async move {
            let (_lease, _admission) = (lease, admission);
            process.call(method, params).await
        })
        .await
        .map_err(|error| Error::Internal(error.into()))?
    }

    /// The installed revision a UI URL's nonce reads, current or previous; never a removed one.
    pub fn asset(&self, revision: &str, nonce: &str) -> Option<super::assets::VerifiedRevision> {
        let inner = &self.0;
        if nonce != inner.nonce {
            return None;
        }
        let state = inner.state.lock().unwrap();
        if state.journal.removed {
            return None;
        }
        for installed in state
            .journal
            .current
            .iter()
            .chain(state.journal.previous.iter())
        {
            let (_, target) = installed.metadata(&inner.catalogue).ok()?;
            if target.sha256 == revision {
                return Some(super::assets::VerifiedRevision {
                    root: inner.packages().join(&target.sha256),
                    files: installed.files.clone(),
                    script: target.script,
                    stylesheet: target.stylesheet,
                    assets: target.assets,
                });
            }
        }
        None
    }
}

impl Inner {
    fn key(&self, suffix: &str) -> String {
        format!("extension_{}_{suffix}", packages::slug(self.catalogue.id))
    }
    fn packages(&self) -> PathBuf {
        self.home.join("packages")
    }
    fn daemon(&self) -> Result<Arc<Daemon>> {
        self.daemon
            .upgrade()
            .ok_or_else(|| Error::Unavailable("Core stopped; restart Chauffeur".into()))
    }
    fn auto_update(&self) -> bool {
        self.store
            .kv_get(&self.key("auto_update"))
            .ok()
            .flatten()
            .as_deref()
            != Some("false")
    }
    fn save(&self, journal: &Journal) -> Result<()> {
        self.store
            .kv_set(&self.key("package"), &serde_json::to_string(journal)?)
    }
    /// Saves `change` before memory reflects it: a failed write leaves both as they were.
    fn commit(&self, state: &mut State, change: impl FnOnce(&mut Journal)) -> Result<()> {
        let mut journal = state.journal.clone();
        change(&mut journal);
        self.save(&journal)?;
        state.journal = journal;
        Ok(())
    }
    fn publish(&self) {
        if let Ok(daemon) = self.daemon()
            && let Ok(extensions) = super::list(&daemon)
        {
            daemon
                .events
                .registry(RegistryEvent::ExtensionsChanged { extensions });
        }
    }
    fn phase(&self, phase: ExtensionPackagePhase) {
        let mut state = self.state.lock().unwrap();
        if state.phase == phase {
            return;
        }
        state.phase = phase;
        drop(state);
        self.publish();
    }
    /// Shows `error`. `Failed` means no working engine: a check, download or update that
    /// failed beside a running one (restored, or never stopped) leaves it `Installed`, or
    /// `Waiting` with a version staged.
    fn fail(&self, error: Error) {
        let running = self
            .process
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|process| process.is_alive());
        let mut state = self.state.lock().unwrap();
        state.phase = match (state.journal.removed, running) {
            (true, _) => ExtensionPackagePhase::Missing,
            (false, false) => ExtensionPackagePhase::Failed,
            (false, true) if state.journal.pending.is_some() => ExtensionPackagePhase::Waiting,
            (false, true) => ExtensionPackagePhase::Installed,
        };
        state.error = Some(error.to_string());
        drop(state);
        self.publish();
    }
    fn notify(&self, method: &str, params: Value) {
        if let Some(process) = self.process.lock().unwrap().as_ref()
            && let Err(error) = process.notify(method, params)
        {
            tracing::warn!(%error, "extension notification failed");
        }
    }

    async fn stop(&self) {
        let process = self.process.lock().unwrap().take();
        if let Some(process) = process {
            process.stop();
            process.drain_callbacks().await;
        }
    }

    /// Stops the engine after it handled what it was sent, so activity handed to it just before a
    /// version switch or Core's exit is not lost; a kill after a few seconds.
    async fn finish(&self) {
        let process = self.process.lock().unwrap().take();
        if let Some(process) = process {
            process.shutdown(Duration::from_secs(3)).await;
        }
    }

    async fn start(&self, installed: &Installed) -> Result<Arc<Process>> {
        let daemon = self.daemon()?;
        let root = installed.check_files(&self.catalogue, &self.packages())?;
        let (release, target) = installed.metadata(&self.catalogue)?;
        // Credentials and sign-in stay closed until `activate`: the journal names this version
        // then, and no failed switch could have to undo them.
        let broker = Broker::open_preflight(self.daemon.clone(), self.catalogue.id, &self.home)?;
        let process = Process::start(
            &root.join(target.executable),
            Init {
                api: HOST_API_VERSION,
                extension: packages::slug(self.catalogue.id).into(),
                version: release.version,
            },
            &daemon.shell_env.vars(),
            broker,
        )
        .await?;
        process.ping().await?;
        Ok(process)
    }

    fn activate(&self, process: Arc<Process>) -> Result<()> {
        // Held across the swap so that activity arriving meanwhile queues behind what is kept.
        let mut activity = self.activity.lock().unwrap();
        // Kept before it is told anything: one that dies right away is then restarted by
        // `supervise` rather than forgotten.
        *self.process.lock().unwrap() = Some(process.clone());
        process.activate_host();
        process.notify("extension.activate", json!({}))?;
        process.notify(
            "extension.viewers",
            json!({"count": self.viewers.load(Ordering::SeqCst)}),
        )?;
        while let Some(params) = activity.front() {
            process.notify("core.activity", params.clone())?;
            activity.pop_front();
        }
        Ok(())
    }

    /// Hands Core activity to the engine, or keeps it, in order, until one is activated.
    fn activity(&self, params: Value) {
        // A removed extension hears nothing, and a reinstall starts afresh.
        if self.state.lock().unwrap().journal.removed {
            return;
        }
        let mut activity = self.activity.lock().unwrap();
        let process = self
            .process
            .lock()
            .unwrap()
            .clone()
            .filter(|process| process.is_alive());
        if activity.is_empty()
            && let Some(process) = process
            && process.notify("core.activity", params.clone()).is_ok()
        {
            return;
        }
        if activity.len() == MAX_ACTIVITY {
            activity.pop_front();
        }
        activity.push_back(params);
    }

    /// Puts back the state a switch interrupted or failed with, and forgets its candidate.
    fn recover(&self) -> Result<()> {
        self.restore_state()?;
        let mut state = self.state.lock().unwrap();
        let rejected = state
            .journal
            .pending
            .as_ref()
            .and_then(|package| package.metadata(&self.catalogue).ok())
            .map(|(_, target)| target.sha256);
        self.commit(&mut state, |journal| {
            journal.rejected = rejected;
            journal.pending = None;
            journal.switching = false;
        })
    }

    /// Restarts a current engine that exited on its own, at most three times in ten minutes;
    /// past that it stays stopped with the reason, so a crashing engine never spins.
    async fn supervise(&self) -> Result<()> {
        let crashed = self
            .process
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|process| !process.is_alive());
        if !crashed {
            return Ok(());
        }
        let Ok(_exclusive) = self.operations.try_write() else {
            return Ok(());
        };
        self.stop().await;
        let current = {
            let state = self.state.lock().unwrap();
            (!state.journal.removed)
                .then(|| state.journal.current.clone())
                .flatten()
        };
        let Some(current) = current else {
            return Ok(());
        };
        let recent = {
            let mut crashes = self.crashes.lock().unwrap();
            crashes.retain(|crash| crash.elapsed() < Duration::from_secs(600));
            crashes.push(Instant::now());
            crashes.len()
        };
        if recent > 3 {
            tracing::warn!(
                extension = packages::slug(self.catalogue.id),
                "an extension engine keeps stopping: leaving it stopped"
            );
            return Err(Error::Unavailable(format!(
                "{} stopped unexpectedly several times; restore its previous version or reinstall it in Settings → Extensions",
                super::name(self.catalogue.id)
            )));
        }
        tracing::warn!(
            extension = packages::slug(self.catalogue.id),
            "an extension engine exited unexpectedly: restarting it"
        );
        self.activate(self.start(&current).await?)?;
        self.phase(ExtensionPackagePhase::Installed);
        Ok(())
    }

    async fn bootstrap(&self) -> Result<()> {
        std::fs::create_dir_all(&self.home)?;
        crate::paths::private_dir(&self.home)?;
        if self.state.lock().unwrap().journal.switching {
            self.recover()?;
        }
        let journal = self.state.lock().unwrap().journal.clone();
        if journal.removed {
            return Ok(());
        }
        if let Some(installed) = &journal.current {
            self.activate(self.start(installed).await?)?;
            self.phase(ExtensionPackagePhase::Installed);
            return Ok(());
        }
        let signed = SignedRelease::read(&self.bootstrap).map_err(|error| Error::Unavailable(format!("The bundled {} package is unavailable: {error}; install it from Settings → Extensions", packages::slug(self.catalogue.id))))?;
        let target = packages::target(&signed.verify(&self.catalogue)?)?;
        let installed = packages::expand(
            signed,
            &self.catalogue,
            &self.bootstrap.join(target.archive),
            &self.packages(),
        )?;
        {
            let mut state = self.state.lock().unwrap();
            self.commit(&mut state, |journal| {
                journal.pending = Some(installed);
                journal.manual = true;
            })?;
        }
        self.apply().await
    }

    async fn check(&self) -> Result<()> {
        let daemon = self.daemon()?;
        std::fs::create_dir_all(&self.home)?;
        let env = daemon.shell_env.vars();
        let current = self
            .state
            .lock()
            .unwrap()
            .journal
            .current
            .clone()
            .and_then(|installed| installed.release.verify(&self.catalogue).ok())
            .and_then(|release| semver::Version::parse(&release.version).ok());
        let mut incompatible = None;
        for tag in download::tags(&self.catalogue, &env).await? {
            let signed = download::manifest(&self.catalogue, &env, &tag, &self.home).await?;
            let release = signed.verify(&self.catalogue)?;
            let version = semver::Version::parse(&release.version)
                .map_err(|error| Error::Internal(error.into()))?;
            if current.as_ref().is_some_and(|current| &version <= current) {
                break;
            }
            if let Err(error) = packages::compatible(&release, env!("CARGO_PKG_VERSION")) {
                incompatible = Some(error.to_string());
                continue;
            }
            // A release without this platform's package is skipped like an incompatible one,
            // so an older compatible release is still offered.
            if let Err(error) = packages::target(&release) {
                incompatible = Some(error.to_string());
                continue;
            }
            let mut state = self.state.lock().unwrap();
            state.available = Some(signed);
            state.error = incompatible;
            drop(state);
            self.publish();
            return Ok(());
        }
        let mut state = self.state.lock().unwrap();
        state.available = None;
        state.error = incompatible;
        drop(state);
        self.publish();
        Ok(())
    }

    async fn command(&self, command: ExtensionCommand, epoch: u64) -> Result<()> {
        if self.state.lock().unwrap().journal.epoch != epoch {
            return Ok(());
        }
        match command {
            ExtensionCommand::Check => return self.check().await,
            ExtensionCommand::Restore => {
                let mut state = self.state.lock().unwrap();
                if state.journal.epoch != epoch {
                    return Ok(());
                }
                let previous = state.journal.previous.clone().ok_or_else(|| {
                    Error::BadRequest(
                        "There is no previous version; reinstall the extension".into(),
                    )
                })?;
                // Automatic updates never bring back the version left; a newer release or an
                // explicit update may.
                let left = state
                    .journal
                    .current
                    .as_ref()
                    .and_then(|current| current.metadata(&self.catalogue).ok())
                    .map(|(_, target)| target.sha256);
                self.commit(&mut state, |journal| {
                    journal.removed = false;
                    journal.pending = Some(previous);
                    journal.manual = true;
                    if left.is_some() {
                        journal.rejected = left;
                    }
                })?;
            }
            ExtensionCommand::Install | ExtensionCommand::Update => {
                let checked = match self.check().await {
                    Err(error) if command == ExtensionCommand::Update => return Err(error),
                    result => result,
                };
                if self.state.lock().unwrap().journal.epoch != epoch {
                    return Ok(());
                }
                let signed = if checked.is_ok() {
                    self.state.lock().unwrap().available.clone()
                } else {
                    None
                };
                if let Some(signed) = signed {
                    self.download(signed, true, epoch).await?;
                } else {
                    {
                        let mut state = self.state.lock().unwrap();
                        if state.journal.epoch != epoch {
                            return Ok(());
                        }
                        self.commit(&mut state, |journal| {
                            journal.removed = false;
                            journal.pending = journal.current.clone();
                            journal.manual = true;
                        })?;
                    }
                    let missing = self.state.lock().unwrap().journal.current.is_none();
                    if missing {
                        if !self.bootstrap.join(packages::MANIFEST).is_file() {
                            checked?;
                        }
                        return self.bootstrap().await;
                    }
                }
            }
            ExtensionCommand::Remove => return Ok(()),
        }
        self.apply().await
    }

    async fn download(&self, signed: SignedRelease, manual: bool, epoch: u64) -> Result<()> {
        let target = packages::target(&signed.verify(&self.catalogue)?)?;
        if !manual && self.skips(&target.sha256) {
            return Ok(());
        }
        self.phase(ExtensionPackagePhase::Downloading);
        let temporary = tempfile::tempdir_in(&self.home)?;
        let archive = temporary.path().join(&target.archive);
        download::asset(
            &self.catalogue,
            &self.daemon()?.shell_env.vars(),
            &format!("v{}", signed.verify(&self.catalogue)?.version),
            &target.archive,
            &archive,
            u64::from(target.bytes),
        )
        .await?;
        let catalogue = self.catalogue.clone();
        let packages = self.packages();
        let installed = tokio::task::spawn_blocking(move || {
            packages::expand(signed, &catalogue, &archive, &packages)
        })
        .await
        .map_err(|error| Error::Internal(error.into()))??;
        let mut state = self.state.lock().unwrap();
        if state.journal.epoch != epoch {
            return Ok(());
        }
        if !manual
            && (state.journal.removed || (state.journal.manual && state.journal.pending.is_some()))
        {
            return Ok(());
        }
        self.commit(&mut state, |journal| {
            if manual {
                journal.removed = false;
            }
            journal.pending = Some(installed);
            journal.manual = manual;
        })?;
        drop(state);
        self.phase(ExtensionPackagePhase::Waiting);
        Ok(())
    }

    /// Whether an automatic download leaves `sha256` alone: a version rejected or rolled back
    /// from, or anything while a version the user chose waits to be applied.
    fn skips(&self, sha256: &str) -> bool {
        let state = self.state.lock().unwrap();
        state.journal.rejected.as_deref() == Some(sha256)
            || state.journal.manual && state.journal.pending.is_some()
    }

    async fn apply(&self) -> Result<()> {
        let journal = self.state.lock().unwrap().journal.clone();
        if journal.switching {
            // A failed switch that could not put its state back: retry that, and never take a
            // new snapshot over the only good one nor start its candidate again.
            let Ok(_exclusive) = self.operations.try_write() else {
                return Ok(());
            };
            self.finish().await;
            self.recover()?;
            if let Some(current) = journal.current.filter(|_| !journal.removed) {
                self.activate(self.start(&current).await?)?;
                self.phase(ExtensionPackagePhase::Installed);
            }
            return Ok(());
        }
        if journal.removed {
            self.stop().await;
            self.phase(ExtensionPackagePhase::Missing);
            return Ok(());
        }
        let Some(candidate) = journal.pending else {
            return Ok(());
        };
        if !journal.manual && !self.auto_update() {
            return Ok(());
        }
        let Ok(_exclusive) = self.operations.try_write() else {
            self.phase(ExtensionPackagePhase::Waiting);
            return Ok(());
        };
        {
            let mut state = self.state.lock().unwrap();
            // An Office page open before the first install (a browser subscribed at once) has
            // no engine to keep: it gets the first one, told its viewer count on activation.
            let viewing =
                self.viewers.load(Ordering::SeqCst) > 0 && state.journal.current.is_some();
            if self.views.load(Ordering::SeqCst) > 0 || viewing {
                drop(state);
                self.phase(ExtensionPackagePhase::Waiting);
                return Ok(());
            }
            state.phase = ExtensionPackagePhase::Starting;
        }
        self.publish();
        self.finish().await;
        self.snapshot_state()?;
        {
            let mut state = self.state.lock().unwrap();
            self.commit(&mut state, |journal| journal.switching = true)?;
        }
        match self.start(&candidate).await {
            Ok(process) => {
                let mut state = self.state.lock().unwrap();
                let version = |installed: &Installed| {
                    installed
                        .release
                        .verify(&self.catalogue)
                        .ok()
                        .map(|release| release.version)
                };
                // The same version again (reinstalled, or published anew) keeps the version
                // before it as the one to restore.
                let same = state
                    .journal
                    .current
                    .as_ref()
                    .is_some_and(|current| version(current) == version(&candidate));
                let candidate_sha = candidate
                    .metadata(&self.catalogue)
                    .ok()
                    .map(|(_, target)| target.sha256);
                let committed = self.commit(&mut state, |journal| {
                    if !same {
                        journal.previous = journal.current.take();
                    }
                    // A rejection stays unless it was this version's, now working: a restore
                    // keeps the version it left from coming back automatically.
                    if journal.rejected == candidate_sha {
                        journal.rejected = None;
                    }
                    journal.current = Some(candidate);
                    journal.pending = None;
                    journal.manual = false;
                    journal.switching = false;
                });
                if let Err(error) = committed {
                    drop(state);
                    // Still switching on disk: the next tick or launch puts the state back.
                    process.stop();
                    return Err(error);
                }
                state.error = None;
                drop(state);
                self.activate(process)?;
                self.phase(ExtensionPackagePhase::Installed);
                Ok(())
            }
            Err(error) => {
                if let Err(restore) = self.recover() {
                    // `switching` stays set: the next tick retries before anything else.
                    tracing::warn!(%restore, "could not restore an extension's state after a failed switch");
                    return Err(error);
                }
                if let Some(previous) = journal.current {
                    self.activate(self.start(&previous).await?)?;
                }
                Err(error)
            }
        }
    }

    fn snapshot_state(&self) -> Result<()> {
        let directory = self.home.join("rollback");
        std::fs::create_dir_all(&directory)?;
        crate::paths::private_dir(&directory)?;
        let database = self.home.join("state.sqlite");
        let snapshot = directory.join("state.sqlite");
        if snapshot.exists() {
            std::fs::remove_file(&snapshot)?;
        }
        if database.exists() {
            rusqlite::Connection::open(database)?.backup(
                rusqlite::MAIN_DB,
                &snapshot,
                rusqlite::backup::NO_PROGRESS,
            )?;
            crate::paths::private_file(&snapshot)?;
        }
        let values: Vec<(String, String)> = self.store.with(|connection| {
            connection.prepare("SELECT key,value FROM kv WHERE substr(key,1,7)='office_' AND key!='office_email'")?.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect()
        })?;
        let path = directory.join("office.json");
        std::fs::write(&path, serde_json::to_vec(&values)?)?;
        crate::paths::private_file(&path)?;
        std::fs::File::open(path)?.sync_all()?;
        Ok(())
    }

    fn restore_state(&self) -> Result<()> {
        let directory = self.home.join("rollback");
        let snapshot = directory.join("state.sqlite");
        let database = self.home.join("state.sqlite");
        if snapshot.exists() {
            let mut connection = rusqlite::Connection::open(&database)?;
            connection.restore(
                rusqlite::MAIN_DB,
                &snapshot,
                None::<fn(rusqlite::backup::Progress)>,
            )?;
        } else if database.exists() {
            // The failed first install had no prior extension database to keep.
            for path in [
                database,
                self.home.join("state.sqlite-wal"),
                self.home.join("state.sqlite-shm"),
            ] {
                if path.exists() {
                    std::fs::remove_file(path)?;
                }
            }
        }
        if self.catalogue.id == ExtensionId::Office {
            let values: Vec<(String, String)> =
                serde_json::from_slice(&std::fs::read(directory.join("office.json"))?)?;
            self.store.with(|connection| {
                let transaction = connection.unchecked_transaction()?;
                transaction.execute(
                    "DELETE FROM kv WHERE substr(key,1,7)='office_' AND key!='office_email'",
                    [],
                )?;
                for (key, value) in values {
                    transaction.execute(
                        "INSERT INTO kv(key,value) VALUES(?1,?2)",
                        rusqlite::params![key, value],
                    )?;
                }
                transaction.commit()
            })?;
        }
        Ok(())
    }
}

struct Job(Arc<Inner>);
impl Drop for Job {
    fn drop(&mut self) {
        self.0.running_job.store(false, Ordering::SeqCst);
    }
}
struct View {
    inner: Arc<Inner>,
    office: bool,
}
impl Drop for View {
    fn drop(&mut self) {
        if self.office {
            self.inner.viewers.fetch_sub(1, Ordering::SeqCst);
            self.inner.notify(
                "extension.viewers",
                json!({"count": self.inner.viewers.load(Ordering::SeqCst)}),
            );
        } else {
            self.inner.views.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

impl Runtime for Managed {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn version(&self) -> Option<String> {
        self.0
            .state
            .lock()
            .unwrap()
            .journal
            .current
            .as_ref()?
            .release
            .verify(&self.0.catalogue)
            .ok()
            .map(|release| release.version)
    }
    fn busy(&self) -> bool {
        self.0.operations.try_write().is_err()
    }
    fn activation_lease(&self) -> Result<Option<Box<dyn Send>>> {
        let guard = self.0.operations.clone().try_write_owned().map_err(|_| {
            Error::Conflict(
                "This extension is busy; finish its current operation before changing activation"
                    .into(),
            )
        })?;
        Ok(Some(Box::new(guard)))
    }
    fn dispatch<'a>(
        &'a self,
        _: &'a Arc<Daemon>,
        call: ClientCall,
    ) -> BoxFuture<'a, Result<Value>> {
        Box::pin(async move {
            let call = serde_json::to_value(call)?;
            self.call(
                call["method"]
                    .as_str()
                    .expect("a client call names its method")
                    .into(),
                call["params"].clone(),
                None,
            )
            .await
        })
    }
    fn activation_changed(&self) {
        self.0.notify("extension.activation_changed", json!({}));
    }
    fn identity_changed(&self) {
        self.0.notify("identity.changed", json!({}));
    }
    fn viewer(&self, _: &Arc<Daemon>) -> Option<Box<dyn Send>> {
        if self.0.catalogue.id != ExtensionId::Office {
            return None;
        }
        self.0.viewers.fetch_add(1, Ordering::SeqCst);
        self.0.notify(
            "extension.viewers",
            json!({"count": self.0.viewers.load(Ordering::SeqCst)}),
        );
        Some(Box::new(View {
            inner: self.0.clone(),
            office: true,
        }))
    }
    fn package_info(&self) -> Option<ExtensionPackageInfo> {
        let inner = &self.0;
        let state = inner.state.lock().unwrap();
        let current = state.journal.current.as_ref();
        let ui = current.and_then(|installed| {
            let (_, target) = installed.metadata(&inner.catalogue).ok()?;
            let daemon = inner.daemon().ok()?;
            let info: Value =
                serde_json::from_slice(&std::fs::read(&daemon.paths.daemon_info).ok()?).ok()?;
            let port = info.get("port")?.as_u64()?;
            Some(ExtensionUi {
                revision: target.sha256.clone(),
                base_url: format!(
                    "http://127.0.0.1:{port}/extension-assets/{}/{}/{}/",
                    packages::slug(inner.catalogue.id),
                    target.sha256,
                    inner.nonce
                ),
                script: target.script,
                stylesheet: target.stylesheet,
                assets: target.assets,
            })
        });
        Some(ExtensionPackageInfo {
            desired_installed: !state.journal.removed,
            installed: current.is_some() && !state.journal.removed,
            auto_update: inner.auto_update(),
            available: state
                .available
                .as_ref()
                .and_then(|signed| signed.verify(&inner.catalogue).ok())
                .map(|release| release.version),
            phase: state.phase,
            error: state.error.clone(),
            ui: if state.journal.removed { None } else { ui },
            can_restore: state.journal.previous.is_some(),
        })
    }
    fn resume(&self) {
        let inner = self.0.clone();
        if inner.started.swap(true, Ordering::SeqCst) {
            return;
        }
        if inner.catalogue.id == ExtensionId::Office
            && let Ok(daemon) = inner.daemon()
        {
            let weak = Arc::downgrade(&inner);
            daemon.events.hook(move |activity| {
                use crate::events::Activity;
                use chauffeur_protocol::extension_host::ExtensionActivity as Owned;
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                let activity = match activity {
                    Activity::ChatCreated => Owned::ChatCreated,
                    Activity::TurnEnded => Owned::TurnEnded,
                    Activity::StepsDone(steps) => Owned::StepsDone { steps: *steps },
                    Activity::PrsRead { workspace, prs } => Owned::PrsRead {
                        workspace: (*workspace).into(),
                        prs: prs.to_vec(),
                    },
                };
                inner.activity(serde_json::to_value(activity).expect("activity serializes"));
            });
            let weak = Arc::downgrade(&inner);
            let mut events = daemon.events.subscribe();
            let shutdown = daemon.shutdown.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! { _ = shutdown.cancelled() => break, event = events.recv() => match event { Ok(event) => { if let Some(inner) = weak.upgrade() { if !matches!(event.event, chauffeur_protocol::events::Event::Office { .. } | chauffeur_protocol::events::Event::Extension { .. }) { inner.notify("core.event", serde_json::to_value(&*event).expect("events serialize")); } } else { break; } }, Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue, Err(_) => break } }
                }
            });
        }
        tokio::spawn(async move {
            let Ok(daemon) = inner.daemon() else {
                return;
            };
            let shutdown = daemon.shutdown.clone();
            drop(daemon);
            {
                let _serial = inner.lifecycle.lock().await;
                if let Err(error) = inner.bootstrap().await {
                    inner.fail(error);
                }
            }
            let mut checked = Instant::now() - Duration::from_secs(6 * 3600);
            loop {
                tokio::select! { _ = shutdown.cancelled() => { inner.finish().await; break; }, _ = tokio::time::sleep(Duration::from_secs(5)) => {} }
                let Ok(_serial) = inner.lifecycle.try_lock() else {
                    continue;
                };
                if let Err(error) = inner.supervise().await {
                    inner.fail(error);
                }
                if let Err(error) = inner.apply().await {
                    inner.fail(error);
                }
                if checked.elapsed() >= Duration::from_secs(6 * 3600) {
                    checked = Instant::now();
                    if let Err(error) = inner.check().await {
                        inner.fail(error);
                        continue;
                    }
                    let signed = {
                        let state = inner.state.lock().unwrap();
                        if !state.journal.removed && inner.auto_update() {
                            state.available.clone()
                        } else {
                            None
                        }
                    };
                    let epoch = inner.state.lock().unwrap().journal.epoch;
                    if let Some(signed) = signed
                        && let Err(error) = inner.download(signed, false, epoch).await
                    {
                        inner.fail(error);
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn fixture() -> (tempfile::TempDir, Arc<Daemon>) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(
            dir.path().join("home"),
            dir.path().join("workspaces"),
            dir.path().join("repos"),
        );
        paths.ensure().unwrap();
        std::fs::write(
            &paths.settings,
            "github_org = \"\"\ngithub_sources = false\nclone_roots = []\nlinear_enabled = false\n",
        )
        .unwrap();
        let bootstrap = dir.path().join("bootstrap");
        let registration = Registration {
            id: ExtensionId::Office,
            name: "Office",
            description: "fixture",
            version: "fixture",
            capabilities: &[],
            build: Arc::new(move |daemon, paths, store| {
                Arc::new(Managed::new(
                    daemon,
                    paths,
                    store,
                    Catalogue {
                        id: ExtensionId::Office,
                        repository: "fixture",
                        public_key: SigningKey::from_bytes(&[7; 32]).verifying_key().to_bytes(),
                    },
                    bootstrap.clone(),
                ))
            }),
        };
        (
            dir,
            Daemon::open_with_extensions(paths, vec![registration]).unwrap(),
        )
    }

    fn package(inner: &Inner, version: &str, fail: bool) -> Installed {
        package_requiring(inner, version, fail, ">=0.30.0", "")
    }

    /// `startup` runs in the engine before it says it is ready.
    fn package_requiring(
        inner: &Inner,
        version: &str,
        fail: bool,
        requires: &str,
        startup: &str,
    ) -> Installed {
        let script = r#"#!/usr/bin/python3
import json,sys,time
def send(value): print(json.dumps(value),flush=True)
init=json.loads(sys.stdin.readline())["init"]
if FAIL:
    send({"type":"host_call","id":999,"method":"kv.set","params":{"key":"office_skin","value":"bad-migration"}})
    json.loads(sys.stdin.readline())
    send({"type":"host_call","id":998,"method":"storage.bootstrap","params":{"schema":"DROP TABLE IF EXISTS durable_fixture"}})
    json.loads(sys.stdin.readline())
    sys.exit(3)
STARTUP
send({"type":"ready","api":1,"extension":"office","version":VERSION})
heard=0
for line in sys.stdin:
    frame=json.loads(line)
    if frame["type"]=="shutdown": sys.exit(0)
    if frame["type"]=="ping": send({"type":"pong","id":frame["id"]})
    if frame["type"]=="notify" and frame["method"]=="extension.viewers":
        send({"type":"host_call","id":99,"method":"kv.set","params":{"key":"office_fixture_viewers","value":str(frame["params"]["count"])}})
    if frame["type"]=="notify" and frame["method"]=="core.activity":
        heard+=1
        send({"type":"host_call","id":100+heard,"method":"kv.set","params":{"key":"office_fixture_activity_%d"%heard,"value":json.dumps(frame["params"])}})
    if frame["type"]=="call":
        if frame["method"]=="office.crash": sys.exit(4)
        if frame["method"]=="office.slow":
            send({"type":"host_call","id":12,"method":"kv.set","params":{"key":"office_fixture_started","value":"yes"}})
            json.loads(sys.stdin.readline())
            time.sleep(0.5)
        send({"type":"result","id":frame["id"],"outcome":{"Ok":VERSION}})
"#.replace("FAIL", if fail { "True" } else { "False" }).replace("STARTUP", startup).replace("VERSION", &serde_json::to_string(version).unwrap());
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("package.tar.gz");
        let gzip = flate2::write::GzEncoder::new(
            std::fs::File::create(&archive).unwrap(),
            flate2::Compression::default(),
        );
        let mut tar = tar::Builder::new(gzip);
        for (name, bytes) in [
            ("engine", script.as_bytes()),
            ("ui/index.js", b"/*fixture*/".as_slice()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o700);
            header.set_cksum();
            tar.append_data(&mut header, name, bytes).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
        let release = ExtensionRelease {
            manifest_version: 1,
            id: ExtensionId::Office,
            version: version.into(),
            requires_core: requires.into(),
            host_api: 1,
            ui_api: 1,
            state_format: 1,
            targets: vec![ExtensionReleaseTarget {
                platform: packages::platform().unwrap().into(),
                archive: "package.tar.gz".into(),
                sha256: packages::digest(&archive).unwrap(),
                bytes: std::fs::metadata(&archive)
                    .unwrap()
                    .len()
                    .try_into()
                    .unwrap(),
                executable: "engine".into(),
                script: "ui/index.js".into(),
                stylesheet: None,
                assets: "ui".into(),
            }],
        };
        let json = serde_json::to_string(&release).unwrap();
        let signature = SigningKey::from_bytes(&[7; 32])
            .sign(json.as_bytes())
            .to_bytes()
            .to_vec();
        std::fs::create_dir_all(&inner.home).unwrap();
        packages::expand(
            SignedRelease { json, signature },
            &inner.catalogue,
            &archive,
            &inner.packages(),
        )
        .unwrap()
    }

    fn stage(inner: &Inner, package: Installed, manual: bool) {
        let mut state = inner.state.lock().unwrap();
        state.journal.pending = Some(package);
        state.journal.manual = manual;
        inner.save(&state.journal).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_mounted_view_defers_an_update_and_keeps_its_existing_engine() {
        let (_dir, daemon) = fixture();
        let runtime = daemon
            .extensions
            .get::<Managed>(ExtensionId::Office)
            .unwrap();
        stage(&runtime.0, package(&runtime.0, "1.0.0", false), true);
        runtime.0.apply().await.unwrap();
        let view = runtime.watch().unwrap();
        stage(&runtime.0, package(&runtime.0, "1.1.0", false), false);
        runtime.0.apply().await.unwrap();
        assert_eq!(
            runtime.package_info().unwrap().phase,
            ExtensionPackagePhase::Waiting
        );
        assert_eq!(
            runtime
                .call("office.version".into(), json!({}), None)
                .await
                .unwrap(),
            json!("1.0.0")
        );
        drop(view);
        runtime.0.apply().await.unwrap();
        assert_eq!(runtime.version().as_deref(), Some("1.1.0"));
        assert!(runtime.package_info().unwrap().can_restore);
        runtime.0.stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_failed_candidate_restores_preferences_and_the_previous_working_process() {
        let (_dir, daemon) = fixture();
        let runtime = daemon
            .extensions
            .get::<Managed>(ExtensionId::Office)
            .unwrap();
        daemon.store.kv_set("office_skin", "old-skin").unwrap();
        stage(&runtime.0, package(&runtime.0, "1.0.0", false), true);
        runtime.0.apply().await.unwrap();
        let database = rusqlite::Connection::open(runtime.0.home.join("state.sqlite")).unwrap();
        database.execute_batch("CREATE TABLE durable_fixture(value TEXT); INSERT INTO durable_fixture VALUES('keep');").unwrap();
        drop(database);
        stage(&runtime.0, package(&runtime.0, "1.1.0", true), true);
        assert!(runtime.0.apply().await.is_err());
        assert_eq!(
            daemon.store.kv_get("office_skin").unwrap().as_deref(),
            Some("old-skin")
        );
        assert_eq!(
            runtime
                .call("office.version".into(), json!({}), None)
                .await
                .unwrap(),
            json!("1.0.0")
        );
        assert!(runtime.0.state.lock().unwrap().journal.rejected.is_some());
        let database = rusqlite::Connection::open(runtime.0.home.join("state.sqlite")).unwrap();
        assert_eq!(
            database
                .query_row("SELECT value FROM durable_fixture", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "keep"
        );
        runtime.0.stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn disconnecting_a_client_does_not_release_its_native_operation_lease() {
        let (_dir, daemon) = fixture();
        let runtime = daemon
            .extensions
            .get::<Managed>(ExtensionId::Office)
            .unwrap();
        stage(&runtime.0, package(&runtime.0, "1.0.0", false), true);
        runtime.0.apply().await.unwrap();
        let task_daemon = daemon.clone();
        let call = tokio::spawn(async move {
            task_daemon
                .extensions
                .get::<Managed>(ExtensionId::Office)
                .unwrap()
                .call("office.slow".into(), json!({}), None)
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while daemon
                .store
                .kv_get("office_fixture_started")
                .unwrap()
                .is_none()
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        call.abort();
        let _ = call.await;
        assert!(runtime.busy());
        assert!(matches!(
            runtime.manage(ExtensionCommand::Remove),
            Err(Error::Conflict(_))
        ));
        tokio::time::timeout(Duration::from_secs(5), async {
            while runtime.busy() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        runtime.0.stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn manual_policy_and_removal_survive_reopening_without_reinstalling_bootstrap() {
        let (_dir, daemon) = fixture();
        let runtime = daemon
            .extensions
            .get::<Managed>(ExtensionId::Office)
            .unwrap();
        assert!(runtime.package_info().unwrap().auto_update);
        runtime.configure(false).unwrap();
        stage(&runtime.0, package(&runtime.0, "1.0.0", false), false);
        runtime.0.apply().await.unwrap();
        assert!(runtime.version().is_none());
        runtime.manage(ExtensionCommand::Remove).unwrap();
        let reopened = Managed::new(
            Arc::downgrade(&daemon),
            &daemon.paths,
            daemon.store.clone(),
            runtime.0.catalogue.clone(),
            runtime.0.bootstrap.parent().unwrap().to_path_buf(),
        );
        assert!(!reopened.package_info().unwrap().auto_update);
        assert!(!reopened.package_info().unwrap().desired_installed);
        reopened.0.bootstrap().await.unwrap();
        assert!(reopened.0.process.lock().unwrap().is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_interrupted_switch_recovers_its_snapshot_before_starting_the_old_engine() {
        let (_dir, daemon) = fixture();
        let runtime = daemon
            .extensions
            .get::<Managed>(ExtensionId::Office)
            .unwrap();
        daemon.store.kv_set("office_skin", "before").unwrap();
        stage(&runtime.0, package(&runtime.0, "1.0.0", false), true);
        runtime.0.apply().await.unwrap();
        runtime.0.stop().await;
        runtime.0.snapshot_state().unwrap();
        stage(&runtime.0, package(&runtime.0, "1.1.0", false), true);
        {
            let mut state = runtime.0.state.lock().unwrap();
            state.journal.switching = true;
            runtime.0.save(&state.journal).unwrap();
        }
        daemon
            .store
            .kv_set("office_skin", "partial-migration")
            .unwrap();
        let reopened = Managed::new(
            Arc::downgrade(&daemon),
            &daemon.paths,
            daemon.store.clone(),
            runtime.0.catalogue.clone(),
            runtime.0.bootstrap.parent().unwrap().to_path_buf(),
        );
        reopened.0.bootstrap().await.unwrap();
        assert_eq!(reopened.version().as_deref(), Some("1.0.0"));
        assert_eq!(
            daemon.store.kv_get("office_skin").unwrap().as_deref(),
            Some("before")
        );
        assert!(reopened.0.state.lock().unwrap().journal.pending.is_none());
        reopened.0.stop().await;
    }

    #[tokio::test]
    async fn removing_while_an_install_is_queued_cannot_resurrect_the_extension() {
        let (_dir, daemon) = fixture();
        let runtime = daemon
            .extensions
            .get::<Managed>(ExtensionId::Office)
            .unwrap();
        let epoch = runtime.0.state.lock().unwrap().journal.epoch;
        runtime.manage(ExtensionCommand::Remove).unwrap();
        runtime
            .0
            .command(ExtensionCommand::Install, epoch)
            .await
            .unwrap();
        assert!(!runtime.package_info().unwrap().desired_installed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_cached_extension_can_be_reinstalled_offline_without_resetting_its_policy() {
        let (_dir, daemon) = fixture();
        let runtime = daemon
            .extensions
            .get::<Managed>(ExtensionId::Office)
            .unwrap();
        stage(&runtime.0, package(&runtime.0, "1.0.0", false), true);
        runtime.0.apply().await.unwrap();
        runtime.configure(false).unwrap();
        runtime.manage(ExtensionCommand::Remove).unwrap();
        daemon
            .shell_env
            .set_vars(std::collections::BTreeMap::from([(
                "PATH".into(),
                "/no-github-cli".into(),
            )]));
        let epoch = runtime.0.state.lock().unwrap().journal.epoch;
        runtime
            .0
            .command(ExtensionCommand::Install, epoch)
            .await
            .unwrap();
        assert!(runtime.package_info().unwrap().installed);
        assert!(!runtime.package_info().unwrap().auto_update);
        assert_eq!(runtime.version().as_deref(), Some("1.0.0"));
        runtime.0.stop().await;
    }

    fn installed<'a>(daemon: &'a Arc<Daemon>, version: &str) -> &'a Managed {
        let runtime = daemon
            .extensions
            .get::<Managed>(ExtensionId::Office)
            .unwrap();
        stage(&runtime.0, package(&runtime.0, version, false), true);
        runtime
    }

    async fn version(runtime: &Managed) -> Result<Value> {
        runtime.call("office.version".into(), json!({}), None).await
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_failed_switch_that_could_not_restore_never_snapshots_over_its_state_again() {
        let (_dir, daemon) = fixture();
        daemon.store.kv_set("office_skin", "before").unwrap();
        let runtime = installed(&daemon, "1.0.0");
        runtime.0.apply().await.unwrap();
        runtime.0.stop().await;
        runtime.0.snapshot_state().unwrap();
        stage(&runtime.0, package(&runtime.0, "1.1.0", false), true);
        {
            let mut state = runtime.0.state.lock().unwrap();
            runtime
                .0
                .commit(&mut state, |journal| journal.switching = true)
                .unwrap();
        }
        // What the candidate left before its restore failed.
        daemon.store.kv_set("office_skin", "partial").unwrap();
        runtime.0.apply().await.unwrap();
        assert_eq!(
            daemon.store.kv_get("office_skin").unwrap().as_deref(),
            Some("before")
        );
        assert_eq!(version(runtime).await.unwrap(), json!("1.0.0"));
        let journal = runtime.0.state.lock().unwrap().journal.clone();
        assert!(!journal.switching && journal.pending.is_none() && journal.rejected.is_some());
        runtime.0.stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_journal_write_that_fails_changes_nothing_in_memory() {
        let (_dir, daemon) = fixture();
        let runtime = installed(&daemon, "1.0.0");
        runtime.0.apply().await.unwrap();
        daemon
            .store
            .with(|connection| {
                connection.execute_batch(
                    "CREATE TRIGGER full_disk BEFORE INSERT ON kv WHEN NEW.key='extension_office_package' BEGIN SELECT RAISE(ABORT, 'disk full'); END;",
                )
            })
            .unwrap();
        assert!(runtime.manage(ExtensionCommand::Remove).is_err());
        assert!(runtime.package_info().unwrap().desired_installed);
        assert_eq!(version(runtime).await.unwrap(), json!("1.0.0"));
        runtime.0.stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn removing_an_extension_stops_its_engine_at_once() {
        let (_dir, daemon) = fixture();
        let runtime = installed(&daemon, "1.0.0");
        runtime.0.apply().await.unwrap();
        let engine = runtime.0.process.lock().unwrap().clone().unwrap();
        // A download holding the lifecycle must not keep a removed engine's background work.
        let _download = runtime.0.lifecycle.lock().await;
        runtime.manage(ExtensionCommand::Remove).unwrap();
        assert!(!engine.is_alive());
        assert!(runtime.0.process.lock().unwrap().is_none());
        assert_eq!(
            runtime.package_info().unwrap().phase,
            ExtensionPackagePhase::Missing
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_crashed_engine_restarts_and_a_crash_loop_stops_with_its_reason() {
        let (_dir, daemon) = fixture();
        let runtime = installed(&daemon, "1.0.0");
        runtime.0.apply().await.unwrap();
        for _ in 0..3 {
            assert!(
                runtime
                    .call("office.crash".into(), json!({}), None)
                    .await
                    .is_err()
            );
            runtime.0.supervise().await.unwrap();
            assert_eq!(version(runtime).await.unwrap(), json!("1.0.0"));
        }
        assert!(
            runtime
                .call("office.crash".into(), json!({}), None)
                .await
                .is_err()
        );
        let error = runtime.0.supervise().await.unwrap_err();
        assert!(
            error.to_string().contains("stopped unexpectedly"),
            "{error}"
        );
        assert!(runtime.0.process.lock().unwrap().is_none());
    }

    async fn heard(daemon: &Daemon, n: u32) -> Option<Value> {
        let key = format!("office_fixture_activity_{n}");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(value) = daemon.store.kv_get(&key).unwrap() {
                    return serde_json::from_str(&value).unwrap();
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .ok()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn activity_during_a_failed_switch_reaches_the_restored_engine_once_in_order() {
        let (_dir, daemon) = fixture();
        let runtime = installed(&daemon, "1.0.0");
        runtime.0.apply().await.unwrap();
        // The engine is stopped for the switch when the activity comes.
        runtime.0.stop().await;
        runtime.0.activity(json!({"type": "chat_created"}));
        runtime.0.activity(json!({"type": "turn_ended"}));
        stage(&runtime.0, package(&runtime.0, "1.1.0", true), true);
        assert!(runtime.0.apply().await.is_err());
        assert_eq!(
            heard(&daemon, 1).await,
            Some(json!({"type": "chat_created"}))
        );
        assert_eq!(heard(&daemon, 2).await, Some(json!({"type": "turn_ended"})));
        // Delivered once: what follows is new activity, straight to the running engine.
        runtime.0.activity(json!({"type": "steps_done"}));
        assert_eq!(heard(&daemon, 3).await, Some(json!({"type": "steps_done"})));
        assert!(runtime.0.activity.lock().unwrap().is_empty());
        runtime.0.stop().await;
    }

    /// An engine as released with host API 1 up to Office 0.30.0: busy as it starts, it quits
    /// when more than 128 requests wait on it.
    const QUEUE_OF_128: &str = r#"
import os
send({"type":"ready","api":1,"extension":"office","version":VERSION})
buffered=b""
def read(block):
    global buffered
    os.set_blocking(0, block)
    try:
        while True:
            chunk=os.read(0,1<<20)
            if not chunk: sys.exit(0)
            buffered+=chunk
            if block: return
    except BlockingIOError:
        pass
    finally:
        os.set_blocking(0, True)
heard=0
while True:
    while b"\n" not in buffered: read(True)
    line,buffered=buffered.split(b"\n",1)
    frame=json.loads(line)
    if frame["type"]=="shutdown": sys.exit(0)
    if frame["type"]=="ping": send({"type":"pong","id":frame["id"]})
    if frame.get("method")=="extension.activate":
        time.sleep(0.5)
        read(False)
        if buffered.count(b"\n")>128: sys.exit(0)
    if frame.get("method")=="core.activity":
        heard+=1
        send({"type":"host_call","id":100+heard,"method":"kv.set","params":{"key":"office_fixture_activity_%d"%heard,"value":json.dumps(frame["params"])}})
    if frame["type"]=="call": send({"type":"result","id":frame["id"],"outcome":{"Ok":VERSION}})
"#;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_released_engine_survives_the_activity_kept_while_none_ran() {
        let (_dir, daemon) = fixture();
        let runtime = installed(&daemon, "1.0.0");
        runtime.0.apply().await.unwrap();
        runtime.0.stop().await;
        for n in 0..200 {
            runtime.0.activity(json!({ "n": n }));
        }
        stage(
            &runtime.0,
            package_requiring(&runtime.0, "1.1.0", false, ">=0.29.0", QUEUE_OF_128),
            true,
        );
        runtime.0.apply().await.unwrap();
        // The newest activity is kept, in order.
        assert_eq!(
            heard(&daemon, 1).await,
            Some(json!({ "n": 200 - MAX_ACTIVITY }))
        );
        assert_eq!(
            heard(&daemon, MAX_ACTIVITY as u32).await,
            Some(json!({ "n": 199 }))
        );
        assert_eq!(version(runtime).await.unwrap(), json!("1.1.0"));
        assert!(runtime.0.activity.lock().unwrap().is_empty());
        runtime.0.stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn reinstalling_a_damaged_revision_replaces_it_with_the_verified_copy() {
        let (_dir, daemon) = fixture();
        let runtime = daemon
            .extensions
            .get::<Managed>(ExtensionId::Office)
            .unwrap();
        let first = package(&runtime.0, "1.0.0", false);
        let root = first
            .root(&runtime.0.catalogue, &runtime.0.packages())
            .unwrap();
        std::fs::write(root.join("engine"), "#!/bin/sh\nexit 1\n").unwrap();
        assert!(
            first
                .check_files(&runtime.0.catalogue, &runtime.0.packages())
                .is_err()
        );
        let again = package(&runtime.0, "1.0.0", false);
        again
            .check_files(&runtime.0.catalogue, &runtime.0.packages())
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_same_version_published_again_keeps_the_version_before_it_to_restore() {
        let (_dir, daemon) = fixture();
        let runtime = installed(&daemon, "1.0.0");
        runtime.0.apply().await.unwrap();
        stage(&runtime.0, package(&runtime.0, "1.1.0", false), true);
        runtime.0.apply().await.unwrap();
        stage(
            &runtime.0,
            package_requiring(&runtime.0, "1.1.0", false, ">=0.29.0", ""),
            true,
        );
        runtime.0.apply().await.unwrap();
        let previous = runtime.0.state.lock().unwrap().journal.previous.clone();
        assert_eq!(
            previous
                .unwrap()
                .release
                .verify(&runtime.0.catalogue)
                .unwrap()
                .version,
            "1.0.0"
        );
        runtime.0.stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_newer_release_without_this_platform_does_not_hide_an_older_compatible_one() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, daemon) = fixture();
        let runtime = daemon
            .extensions
            .get::<Managed>(ExtensionId::Office)
            .unwrap();
        let releases = dir.path().join("releases");
        let sign = |version: &str, platform: &str| {
            let json = serde_json::to_string(&ExtensionRelease {
                manifest_version: 1,
                id: ExtensionId::Office,
                version: version.into(),
                requires_core: ">=0.30.0".into(),
                host_api: 1,
                ui_api: 1,
                state_format: 1,
                targets: vec![ExtensionReleaseTarget {
                    platform: platform.into(),
                    archive: "package.tar.gz".into(),
                    sha256: "a".repeat(64),
                    bytes: 1,
                    executable: "engine".into(),
                    script: "ui/index.js".into(),
                    stylesheet: None,
                    assets: "ui".into(),
                }],
            })
            .unwrap();
            let folder = releases.join(format!("v{version}"));
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(
                folder.join(packages::SIGNATURE),
                SigningKey::from_bytes(&[7; 32])
                    .sign(json.as_bytes())
                    .to_bytes(),
            )
            .unwrap();
            std::fs::write(folder.join(packages::MANIFEST), json).unwrap();
        };
        sign("2.0.0", "riscv64-unknown-none");
        sign("1.1.0", packages::platform().unwrap());
        std::fs::write(
            releases.join("tags.json"),
            r#"[{"tagName":"v2.0.0"},{"tagName":"v1.1.0"}]"#,
        )
        .unwrap();
        let gh = dir.path().join("bin/gh");
        std::fs::create_dir_all(gh.parent().unwrap()).unwrap();
        std::fs::write(
            &gh,
            "#!/bin/sh\ncase \"$2\" in\nlist) cat \"$RELEASES/tags.json\";;\ndownload) tag=\"$3\"; while [ $# -gt 0 ]; do [ \"$1\" = \"--pattern\" ] && pattern=\"$2\"; shift; done; cat \"$RELEASES/$tag/$pattern\";;\nesac\n",
        )
        .unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o700)).unwrap();
        daemon.shell_env.set_vars(std::collections::BTreeMap::from([
            (
                "PATH".into(),
                format!("{}:/usr/bin:/bin", gh.parent().unwrap().display()),
            ),
            ("RELEASES".into(), releases.display().to_string()),
        ]));
        runtime.0.check().await.unwrap();
        assert_eq!(
            runtime.package_info().unwrap().available.as_deref(),
            Some("1.1.0")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_failed_background_check_beside_a_working_engine_is_no_failed_install() {
        let (_dir, daemon) = fixture();
        let runtime = installed(&daemon, "1.0.0");
        runtime.0.apply().await.unwrap();
        daemon
            .shell_env
            .set_vars(std::collections::BTreeMap::from([(
                "PATH".into(),
                "/no-github-cli".into(),
            )]));
        let error = runtime.0.check().await.unwrap_err();
        runtime.0.fail(error);
        let info = runtime.package_info().unwrap();
        assert_eq!(info.phase, ExtensionPackagePhase::Installed);
        assert!(info.error.is_some());
        // Without a working engine, the same failure is one.
        runtime.0.stop().await;
        runtime
            .0
            .fail(Error::Unavailable("the engine did not start".into()));
        assert_eq!(
            runtime.package_info().unwrap().phase,
            ExtensionPackagePhase::Failed
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_view_opened_before_the_first_install_cannot_hold_it_back() {
        let (_dir, daemon) = fixture();
        let runtime = daemon
            .extensions
            .get::<Managed>(ExtensionId::Office)
            .unwrap();
        assert!(matches!(runtime.watch(), Err(Error::Unavailable(_))));
        stage(&runtime.0, package(&runtime.0, "1.0.0", false), true);
        runtime.0.apply().await.unwrap();
        assert_eq!(runtime.version().as_deref(), Some("1.0.0"));
        // Once a version is current, a view pins it even with an update staged.
        let view = runtime.watch().unwrap();
        stage(&runtime.0, package(&runtime.0, "1.1.0", false), false);
        runtime.0.apply().await.unwrap();
        assert_eq!(runtime.version().as_deref(), Some("1.0.0"));
        drop(view);
        runtime.0.stop().await;
    }

    fn sha(inner: &Inner, installed: &Installed) -> String {
        installed.metadata(&inner.catalogue).unwrap().1.sha256
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_queued_restore_survives_automatic_updates_which_never_bring_back_the_version_left() {
        let (_dir, daemon) = fixture();
        let runtime = installed(&daemon, "1.0.0");
        runtime.0.apply().await.unwrap();
        let bad = package(&runtime.0, "1.1.0", false);
        stage(&runtime.0, bad.clone(), true);
        runtime.0.apply().await.unwrap();
        let view = runtime.watch().unwrap();
        let epoch = runtime.0.state.lock().unwrap().journal.epoch;
        runtime
            .0
            .command(ExtensionCommand::Restore, epoch)
            .await
            .unwrap();
        // A newer release found while the restore waits for the view to close leaves it queued
        // (an automatic download stops before reaching GitHub).
        daemon
            .shell_env
            .set_vars(std::collections::BTreeMap::from([(
                "PATH".into(),
                "/no-github-cli".into(),
            )]));
        let newer = package(&runtime.0, "1.2.0", false);
        runtime
            .0
            .download(newer.release.clone(), false, epoch)
            .await
            .unwrap();
        let pending = runtime.0.state.lock().unwrap().journal.pending.clone();
        assert_eq!(
            sha(&runtime.0, &pending.unwrap()),
            sha(&runtime.0, &package(&runtime.0, "1.0.0", false))
        );
        drop(view);
        runtime.0.apply().await.unwrap();
        assert_eq!(runtime.version().as_deref(), Some("1.0.0"));
        // The version left stays rejected for automatic downloads only.
        assert!(runtime.0.skips(&sha(&runtime.0, &bad)));
        assert!(!runtime.0.skips(&sha(&runtime.0, &newer)));
        runtime.0.stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_starting_version_cannot_sign_out_before_it_is_the_installed_one() {
        let (_dir, daemon) = fixture();
        daemon
            .store
            .kv_set(crate::identity::EMAIL_KEY, "a@example.com")
            .unwrap();
        let runtime = installed(&daemon, "1.0.0");
        runtime.0.apply().await.unwrap();
        // A candidate whose startup signs out, and stops if it cannot.
        let candidate = package_requiring(
            &runtime.0,
            "1.1.0",
            false,
            ">=0.30.0",
            r#"send({"type":"host_call","id":997,"method":"identity.sign_out","params":{}})
if "Err" in json.loads(sys.stdin.readline())["outcome"]: sys.exit(5)"#,
        );
        stage(&runtime.0, candidate, true);
        assert!(runtime.0.apply().await.is_err());
        assert_eq!(
            daemon
                .store
                .kv_get(crate::identity::EMAIL_KEY)
                .unwrap()
                .as_deref(),
            Some("a@example.com")
        );
        assert_eq!(version(runtime).await.unwrap(), json!("1.0.0"));
        runtime.0.stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_office_page_open_before_the_first_install_gets_it_and_later_updates_wait_for_it() {
        let (_dir, daemon) = fixture();
        let runtime = daemon
            .extensions
            .get::<Managed>(ExtensionId::Office)
            .unwrap();
        let viewer = runtime.viewer(&daemon).unwrap();
        stage(&runtime.0, package(&runtime.0, "1.0.0", false), true);
        runtime.0.apply().await.unwrap();
        assert_eq!(runtime.version().as_deref(), Some("1.0.0"));
        tokio::time::timeout(Duration::from_secs(5), async {
            while daemon
                .store
                .kv_get("office_fixture_viewers")
                .unwrap()
                .as_deref()
                != Some("1")
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the first engine hears the page's viewer");
        stage(&runtime.0, package(&runtime.0, "1.1.0", false), false);
        runtime.0.apply().await.unwrap();
        assert_eq!(runtime.version().as_deref(), Some("1.0.0"));
        drop(viewer);
        runtime.0.apply().await.unwrap();
        assert_eq!(runtime.version().as_deref(), Some("1.1.0"));
        runtime.0.stop().await;
    }
}
