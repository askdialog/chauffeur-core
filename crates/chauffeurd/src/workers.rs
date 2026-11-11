//! GCP workers (docs/cloud-workers.md): Compute Engine VMs this daemon creates with the user's own
//! `gcloud`, installs Chauffeur on (the Linux bundle of this daemon's version) and reaches through
//! the worker daemon's socket forwarded over SSH, kept up while the worker is ready. A ready worker
//! is a transfer machine, by its name.
//!
//! Only instances this daemon labelled `chauffeur-worker` are stopped or deleted: the project's
//! other VMs (the relay, the Office) are never touched. A worker's backup account is limited to its own managed
//! folder. No firewall rule or project SSH metadata is changed: `gcloud` puts the user's key on
//! the instance itself. Manual backup recovery deletes a labelled VM with its disks kept.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, bail};
use chauffeur_protocol::model::AgentKind;
use chauffeur_protocol::worker::{
    WorkerCreateParams, WorkerInfo, WorkerMachineType, WorkerParams, WorkerState, WorkersResult,
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::{Daemon, Error, Result};

const LABEL: &str = "chauffeur-worker";
const INSTANCE_PREFIX: &str = "chauffeur-worker-";
const IMAGE_FAMILY: &str = "ubuntu-2404-lts-amd64";
const IMAGE_PROJECT: &str = "ubuntu-os-cloud";
const DISK_GB: u32 = 50;
/// Workers created without asking again (docs/cloud-workspaces.md: up to two automatically).
const LIMIT: u32 = 2;
const DEFAULT_MACHINE_TYPE: &str = "e2-standard-4";
/// On-demand prices in europe-west9, from gcloud-compute.com on this date: the VM an hour, a
/// balanced disk $0.116 per GB-month (730 hours), an ephemeral IPv4 address $0.005 an hour.
/// Network traffic and the agents' model use are not included.
const PRICES_AS_OF: &str = "2026-10-25";
/// What a stopped worker still costs: its disk.
const DISK_USD_PER_GB_MONTH: f64 = 0.116;
/// Workers live in GCP Paris (docs/cloud-workspaces.md), the region the prices are for.
const REGION: &str = "europe-west9";
const MACHINE_TYPES: &[(&str, u32, u32, f64)] = &[
    ("e2-standard-2", 2, 8, 0.0777),
    ("e2-standard-4", 4, 16, 0.1555),
    ("e2-standard-8", 8, 32, 0.3109),
];
const INSTALL: &str = include_str!("worker_install.sh");
/// IAM and Compute Engine learn an account just made within about two minutes: tried
/// again this often, this many times.
#[cfg(not(test))]
const NOT_KNOWN_YET_PAUSE: Duration = Duration::from_secs(5);
#[cfg(test)]
const NOT_KNOWN_YET_PAUSE: Duration = Duration::from_millis(50);
const NOT_KNOWN_YET_TRIES: u32 = 24;
/// Tests and the e2e scripts run a fake `gcloud` in place of the user's.
const GCLOUD_OVERRIDE: &str = "CHAUFFEUR_GCLOUD";
/// And a fake `ssh`, which forwards a worker's socket once its SSH entry is known.
const SSH_OVERRIDE: &str = "CHAUFFEUR_SSH";
/// What gcloud says when its session expired or was revoked, whatever the command.
const SIGNED_OUT: &str =
    "gcloud is signed out on this Mac: run `gcloud auth login` in a terminal, then retry";
/// A forward with an SSH entry that never reached the worker reads the entry again (its address
/// may have changed) at most this often.
const ENTRY_RECHECK: Duration = Duration::from_secs(5 * 60);
/// Where to include the workers' ssh entries from, for a daemon whose state is not `~/.chauffeur`
/// (the e2e scripts); without it such a daemon never touches `~/.ssh`.
const SSH_DIR_OVERRIDE: &str = "CHAUFFEUR_SSH_DIR";
/// Set once the `Include` was added, or found, for the workers: a user who takes it out later
/// keeps it out.
const INCLUDE_DONE: &str = "worker_ssh_include";

fn hourly_usd(vm: f64) -> f64 {
    let estimate = vm + f64::from(DISK_GB) * DISK_USD_PER_GB_MONTH / 730.0 + 0.005;
    (estimate * 10_000.0).round() / 10_000.0
}

/// The workers this daemon is creating or deleting, and the forwards it keeps up.
pub struct Workers {
    /// The `gcloud` the daemon runs: `CHAUFFEUR_GCLOUD`, else the login shell's; a test's fake.
    pub gcloud: std::sync::RwLock<PathBuf>,
    /// The `ssh` the forwards run: `CHAUFFEUR_SSH`, else the login shell's; a test's fake.
    pub ssh: std::sync::RwLock<PathBuf>,
    /// The `.ssh` folder named explicitly (`CHAUFFEUR_SSH_DIR`, or a test's).
    ssh_dir: std::sync::RwLock<Option<PathBuf>>,
    /// Held while the ssh files are written: workers refreshed at once at start would interleave.
    ssh_files: Mutex<()>,
    pub(crate) busy: Mutex<BTreeSet<String>>,
    pub(crate) lifecycle: Arc<tokio::sync::Mutex<()>>,
    forwards: Mutex<BTreeMap<String, Arc<Forward>>>,
    /// The logins `worker.login` follows, by worker and agent: a new one replaces the last.
    pub(crate) logins: Mutex<HashMap<(String, AgentKind), tokio::task::AbortHandle>>,
}

/// One worker's forwarded socket: an `ssh -N -L` with the worker's SSH entry, or run by `gcloud
/// compute ssh` while it has none, restarted when it ends, in a process group of its own so that
/// stopping it stops `ssh` too.
#[derive(Default)]
struct Forward {
    stop: CancellationToken,
    group: AtomicI32,
    connected: AtomicBool,
    /// Why its last run did not reach the worker's daemon; cleared once it does.
    issue: Mutex<Option<ForwardIssue>>,
}

/// Why a forward does not reach the worker's daemon, from what its `ssh` or `gcloud` printed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ForwardIssue {
    pub message: String,
    /// Trying again changes nothing until the user acts: gcloud signed out, the key refused, the
    /// host key changed.
    pub lasting: bool,
    /// When the run that left it ended: one from before the user acted says nothing of now.
    pub at: tokio::time::Instant,
}

impl Forward {
    fn kill(&self) {
        let group = self.group.swap(0, Ordering::SeqCst);
        if group > 0 {
            // SAFETY: signals a process group this daemon started.
            unsafe { libc::killpg(group, libc::SIGTERM) };
        }
    }
}

impl Default for Workers {
    fn default() -> Self {
        // A unit test that forgets its fake reaches no real project.
        let program = |name: &str, over: &str| {
            let default = if cfg!(test) {
                format!("/nonexistent/{name}")
            } else {
                name.to_owned()
            };
            std::sync::RwLock::new(
                std::env::var_os(over)
                    .filter(|path| !path.is_empty())
                    .map_or_else(|| PathBuf::from(default), PathBuf::from),
            )
        };
        Workers {
            gcloud: program("gcloud", GCLOUD_OVERRIDE),
            ssh: program("ssh", SSH_OVERRIDE),
            ssh_dir: std::sync::RwLock::new(
                std::env::var_os(SSH_DIR_OVERRIDE)
                    .filter(|dir| !dir.is_empty())
                    .map(PathBuf::from),
            ),
            ssh_files: Mutex::default(),
            busy: Mutex::default(),
            lifecycle: Arc::new(tokio::sync::Mutex::new(())),
            forwards: Mutex::default(),
            logins: Mutex::default(),
        }
    }
}

impl Workers {
    /// Stops every forward; the daemon is quitting, so its tasks may not run again.
    pub fn shutdown(&self) {
        for forward in self.forwards.lock().unwrap().values() {
            forward.stop.cancel();
            forward.kill();
        }
    }

    fn stop_forward(&self, name: &str) {
        if let Some(forward) = self.forwards.lock().unwrap().remove(name) {
            forward.stop.cancel();
            forward.kill();
        }
    }
}

/// What the store keeps of a worker.
#[derive(Serialize, Deserialize, Clone, Debug)]
struct Record {
    name: String,
    instance: String,
    project: String,
    zone: String,
    machine_type: String,
    state: WorkerState,
    log: Vec<String>,
    error: Option<String>,
    created_at: String,
    /// The worker daemon's socket on the VM, once installed.
    remote_socket: Option<String>,
    /// How this machine's editors reach it over SSH, once ready.
    #[serde(default)]
    ssh: Option<crate::worker_ssh::SshEntry>,
    /// Why its entry could not be read last time.
    #[serde(default)]
    ssh_error: Option<String>,
    /// The bucket its service account was given its prefix of, to take that access back.
    #[serde(default)]
    backup_bucket: Option<String>,
    /// The service account Chauffeur made for it, recorded once created: only that one is
    /// reused or deleted.
    #[serde(default)]
    backup_account: Option<String>,
    /// Its own prefix in the bucket, `<name>-<random>`: a worker made again under the same name
    /// gets another, and no access to the backups of the one it replaces.
    #[serde(default)]
    backup_prefix: Option<String>,
}

fn load(daemon: &Daemon, name: &str) -> Result<Option<Record>> {
    let raw: Option<String> = daemon.store.with(|c| {
        c.query_row("SELECT info FROM workers WHERE name=?1", [name], |r| {
            r.get(0)
        })
        .optional()
    })?;
    raw.map(|raw| serde_json::from_str(&raw).map_err(|e| Error::Internal(e.into())))
        .transpose()
}

pub(crate) fn deleting(daemon: &Daemon, name: &str) -> Result<bool> {
    Ok(load(daemon, name)?.is_some_and(|r| r.state == WorkerState::Deleting))
}

fn records(daemon: &Daemon) -> Result<Vec<Record>> {
    let rows: Vec<String> = daemon.store.with(|c| {
        c.prepare("SELECT info FROM workers ORDER BY created_at, name")?
            .query_map([], |r| r.get(0))?
            .collect()
    })?;
    Ok(rows
        .iter()
        .filter_map(|raw| serde_json::from_str(raw).ok())
        .collect())
}

fn save(daemon: &Daemon, record: &Record) -> Result<()> {
    let info = serde_json::to_string(record).map_err(|e| Error::Internal(e.into()))?;
    daemon.store.with(|c| {
        c.execute(
            "INSERT INTO workers (name, created_at, info) VALUES (?1, ?2, ?3)
             ON CONFLICT(name) DO UPDATE SET info=excluded.info",
            params![record.name, record.created_at, info],
        )
    })?;
    Ok(())
}

fn remove(daemon: &Daemon, name: &str) -> Result<()> {
    daemon
        .store
        .with(|c| c.execute("DELETE FROM workers WHERE name=?1", [name]))?;
    Ok(())
}

/// Changes a worker's record, when it still exists.
fn change(daemon: &Daemon, name: &str, edit: impl FnOnce(&mut Record)) {
    let outcome = load(daemon, name).and_then(|record| match record {
        Some(mut record) => {
            edit(&mut record);
            save(daemon, &record)
        }
        None => Ok(()),
    });
    if let Err(error) = outcome {
        tracing::warn!(%error, worker = name, "could not record the worker's progress");
    }
}

fn log(daemon: &Daemon, name: &str, line: impl Into<String>) {
    let line = line.into();
    change(daemon, name, |r| r.log.push(line));
}

pub(crate) fn socket_path(daemon: &Daemon, name: &str) -> PathBuf {
    daemon
        .paths
        .home
        .join("workers")
        .join(format!("{name}.sock"))
}

/// Why a ready worker's forward does not reach its daemon now; none while it does, or before its
/// first run ended.
pub(crate) fn forward_issue(daemon: &Daemon, name: &str) -> Option<ForwardIssue> {
    let forwards = daemon.workers.forwards.lock().unwrap();
    let forward = forwards.get(name)?;
    if forward.connected.load(Ordering::SeqCst) {
        return None;
    }
    forward.issue.lock().unwrap().clone()
}

fn info(daemon: &Daemon, record: &Record) -> WorkerInfo {
    let connected = daemon
        .workers
        .forwards
        .lock()
        .unwrap()
        .get(&record.name)
        .is_some_and(|f| f.connected.load(Ordering::SeqCst));
    let error = record.error.clone().or_else(|| {
        (record.state == WorkerState::Ready)
            .then(|| forward_issue(daemon, &record.name))
            .flatten()
            .map(|issue| issue.message)
    });
    let (ssh_host, ssh_issue) = match remote_host_of(daemon, record) {
        RemoteHost::Host(host) => (Some(host), None),
        RemoteHost::Unavailable(why) => (None, Some(why)),
    };
    WorkerInfo {
        name: record.name.clone(),
        instance: record.instance.clone(),
        project: record.project.clone(),
        zone: record.zone.clone(),
        machine_type: record.machine_type.clone(),
        hourly_usd: machine_type(&record.machine_type).map_or(0.0, |m| m.hourly_usd),
        state: if refuse_recovery_worker(daemon, &record.name).is_ok() {
            record.state
        } else {
            WorkerState::Failed
        },
        connected,
        socket: socket_path(daemon, &record.name).display().to_string(),
        ssh_host,
        ssh_issue,
        log: record.log.clone(),
        error,
        created_at: record.created_at.clone(),
    }
}

fn machine_type(name: &str) -> Option<WorkerMachineType> {
    MACHINE_TYPES
        .iter()
        .find(|(n, ..)| *n == name)
        .map(|&(name, vcpus, memory_gb, vm)| WorkerMachineType {
            name: name.into(),
            vcpus,
            memory_gb,
            hourly_usd: hourly_usd(vm),
        })
}

pub fn list(daemon: &Daemon) -> Result<WorkersResult> {
    let settings = daemon.settings.get();
    Ok(WorkersResult {
        workers: records(daemon)?
            .iter()
            .map(|record| info(daemon, record))
            .collect(),
        machine_types: MACHINE_TYPES
            .iter()
            .filter_map(|(name, ..)| machine_type(name))
            .collect(),
        default_machine_type: DEFAULT_MACHINE_TYPE.into(),
        project: settings.workers.project.clone(),
        zone: settings.workers.zone.clone(),
        disk_gb: DISK_GB,
        prices_as_of: PRICES_AS_OF.into(),
        limit: LIMIT,
    })
}

/// The names of the ready workers.
pub fn ready(daemon: &Daemon) -> Vec<String> {
    records(daemon)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| {
            r.state == WorkerState::Ready
                && refuse_recovery_worker(daemon, &r.name).is_ok()
                && crate::worker_lifecycle::allow_use(daemon, &r.name).is_ok()
        })
        .map(|r| r.name)
        .collect()
}

/// The socket a transfer reaches a machine on: one declared in `[transfer]`, else a ready worker.
pub fn machine_socket(daemon: &Daemon, name: &str) -> anyhow::Result<PathBuf> {
    let settings = daemon.settings.get();
    if let Some(machine) = settings.transfer.machines.iter().find(|m| m.name == name) {
        return Ok(crate::settings::expand_home(&machine.socket));
    }
    refuse_recovery_worker(daemon, name)?;
    crate::worker_lifecycle::allow_use(daemon, name)?;
    match load(daemon, name).map_err(|e| anyhow::anyhow!("{e}"))? {
        Some(record) if record.state == WorkerState::Ready => Ok(socket_path(daemon, name)),
        Some(_) => bail!("The worker {name} is not ready; see Settings → Transfer"),
        None => bail!("No machine named {name} in Settings → Transfer; add it there"),
    }
}

/// A GCP worker of this daemon goes by `name`, and no machine of `[transfer]` takes it first.
pub fn is_worker(daemon: &Daemon, name: &str) -> bool {
    !daemon
        .settings
        .get()
        .transfer
        .machines
        .iter()
        .any(|m| m.name == name)
        && load(daemon, name).ok().flatten().is_some()
}

/// Lowercase letters, digits and dashes, starting with a letter: the instance's name and label
/// value take it as is.
fn check_name(name: &str) -> Result<()> {
    let fine = name.len() <= 20
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if fine {
        Ok(())
    } else {
        Err(Error::BadRequest(format!(
            "\"{name}\" cannot name a worker: use up to 20 lowercase letters, digits and dashes, starting with a letter"
        )))
    }
}

/// Creates a worker in the background and returns it as creating. A worker whose creation failed
/// is created again where it stopped, with its first machine type; a stopped one is started and
/// this daemon's Chauffeur installed on it again, its agents' logins kept.
pub async fn create(daemon: &Arc<Daemon>, params: WorkerCreateParams) -> Result<WorkerInfo> {
    let name = params.name.trim().to_owned();
    check_name(&name)?;
    crate::worker_lifecycle::allow_use(daemon, &name)?;
    let wanted = params
        .machine_type
        .unwrap_or_else(|| DEFAULT_MACHINE_TYPE.into());
    if machine_type(&wanted).is_none() {
        let known: Vec<&str> = MACHINE_TYPES.iter().map(|(n, ..)| *n).collect();
        return Err(Error::BadRequest(format!(
            "Unknown machine type {wanted}: choose {}",
            known.join(", ")
        )));
    }
    let settings = daemon.settings.get();
    if settings.workers.project.trim().is_empty() {
        return Err(Error::BadRequest(
            "No Google Cloud project is configured: set `project` in [workers] of settings.toml before creating a worker".into(),
        ));
    }
    if settings
        .workers
        .zone
        .rsplit_once('-')
        .map(|(region, _)| region)
        != Some(REGION)
    {
        return Err(Error::BadRequest(format!(
            "Workers are created in {REGION} (Paris), whose prices the estimate uses: set `zone` in [workers] to {REGION}-a, -b or -c, not {}",
            settings.workers.zone
        )));
    }
    if settings
        .transfer
        .machines
        .iter()
        .any(|m| m.name.eq_ignore_ascii_case(&name))
    {
        return Err(Error::BadRequest(format!(
            "A machine named {name} is already declared in Settings → Transfer; name the worker otherwise"
        )));
    }
    refuse_recovery_worker(daemon, &name)?;
    let existing = load(daemon, &name)?;
    let new = existing.is_none();
    let mut record = match existing {
        Some(record) if !matches!(record.state, WorkerState::Failed | WorkerState::Stopped) => {
            return Err(Error::Conflict(format!(
                "A worker named {name} already exists"
            )));
        }
        Some(record) => record,
        None => Record {
            instance: format!("{INSTANCE_PREFIX}{name}"),
            name: name.clone(),
            project: settings.workers.project.clone(),
            zone: settings.workers.zone.clone(),
            machine_type: wanted,
            state: WorkerState::Creating,
            log: Vec::new(),
            error: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            remote_socket: None,
            ssh: None,
            ssh_error: None,
            backup_bucket: None,
            backup_account: None,
            backup_prefix: None,
        },
    };
    let account = run(
        gcloud(
            daemon,
            &[
                "auth",
                "list",
                "--filter=status:ACTIVE",
                "--format=value(account)",
            ],
        ),
        "gcloud auth list",
    )
    .await
    .map_err(|e| Error::BadRequest(format!("{e:#}")))?;
    if account.trim().is_empty() {
        return Err(Error::BadRequest(
            "gcloud is not logged in: run `gcloud auth login`, then retry".into(),
        ));
    }
    {
        // Counted and saved under the lock: two creations at once cannot both pass the limit.
        let mut busy = daemon.workers.busy.lock().unwrap();
        let count = records(daemon)?.len() as u32;
        if new && count >= LIMIT && !params.beyond_limit {
            return Err(Error::Conflict(format!(
                "{count} workers already exist; confirm to create one more"
            )));
        }
        if !busy.insert(name.clone()) {
            return Err(Error::Conflict(format!("{name} is already being changed")));
        }
        record.state = WorkerState::Creating;
        record.error = None;
        record.log.push(format!(
            "Creating {} ({}, {} in {}) as {}",
            record.name,
            record.machine_type,
            record.zone,
            record.project,
            account.trim()
        ));
        if let Err(error) = save(daemon, &record) {
            busy.remove(&name);
            return Err(error);
        }
    }
    let started = info(daemon, &record);
    let daemon = daemon.clone();
    tokio::spawn(async move {
        let outcome = provision(&daemon, &record).await;
        match outcome {
            Ok(()) => log(&daemon, &name, format!("{name} is ready")),
            Err(error) => {
                tracing::warn!(error = format!("{error:#}"), worker = %name, "could not create the worker");
                change(&daemon, &name, |r| {
                    r.state = WorkerState::Failed;
                    r.error = Some(format!("{error:#}"));
                });
            }
        }
        daemon.workers.busy.lock().unwrap().remove(&name);
    });
    Ok(started)
}

async fn provision(daemon: &Arc<Daemon>, record: &Record) -> anyhow::Result<()> {
    let name = &record.name;
    let bundle = bundle(daemon).await?;
    log(daemon, name, format!("Linux bundle: {}", bundle.display()));
    let account = prepare_backups(daemon, record).await?;
    match describe(daemon, record).await? {
        Some(instance) => {
            check_label(record, &instance.labels)?;
            log(
                daemon,
                name,
                format!("The VM {} already exists", record.instance),
            );
            if instance.status == "TERMINATED" {
                log(daemon, name, format!("Starting the VM {}", record.instance));
                run(
                    compute(daemon, record, &["instances", "start", &record.instance]),
                    "gcloud compute instances start",
                )
                .await?;
            }
        }
        None => {
            log(daemon, name, format!("Creating the VM {}", record.instance));
            let label = format!("{LABEL}={name}");
            let disk = format!("{DISK_GB}GB");
            // Compute Engine learns of an account just made a little after IAM does.
            let mut tries = 0;
            while let Err(error) = run(
                compute(
                    daemon,
                    record,
                    &[
                        "instances",
                        "create",
                        &record.instance,
                        "--machine-type",
                        &record.machine_type,
                        "--image-family",
                        IMAGE_FAMILY,
                        "--image-project",
                        IMAGE_PROJECT,
                        "--boot-disk-size",
                        &disk,
                        "--boot-disk-type",
                        "pd-balanced",
                        "--labels",
                        &label,
                        "--metadata",
                        "block-project-ssh-keys=true",
                        "--service-account",
                        &account,
                        "--scopes",
                        STORAGE_SCOPE,
                        "--shielded-secure-boot",
                        "--shielded-vtpm",
                        "--shielded-integrity-monitoring",
                    ],
                ),
                "gcloud compute instances create",
            )
            .await
            {
                let message = format!("{error:#}");
                if tries == NOT_KNOWN_YET_TRIES
                    || !message.contains("of type 'serviceAccount' was not found")
                {
                    return Err(error);
                }
                tries += 1;
                tokio::time::sleep(NOT_KNOWN_YET_PAUSE).await;
            }
        }
    }
    change(daemon, name, |r| r.state = WorkerState::Installing);
    let remote = install(daemon, record, &bundle).await?;
    // Read before the forward, which reaches the worker with it: a VM started again may have
    // another address. gcloud's ssh has just recorded its host key, which the entry checks.
    let entry = add_ssh_entry(daemon, record).await;
    log(daemon, name, "Forwarding its daemon's socket");
    let forward = start_forward(daemon, name, &remote, record);
    for _ in 0..60 {
        if forward.connected.load(Ordering::SeqCst) {
            change(daemon, name, |r| r.state = WorkerState::Ready);
            match entry {
                Ok(lines) => lines.into_iter().for_each(|line| log(daemon, name, line)),
                Err(error) => log(
                    daemon,
                    name,
                    format!("Not opened in this machine's editors yet: {error:#}"),
                ),
            }
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let why = forward
        .issue
        .lock()
        .unwrap()
        .as_ref()
        .map(|issue| format!(" ({})", issue.message))
        .unwrap_or_default();
    bail!(
        "the worker's daemon does not answer on its forwarded socket{why}; see {}",
        forward_log(daemon, name).display()
    )
}

/// Copies the Linux bundle to the worker and runs `worker_install.sh` there, which restarts its
/// daemon on the new binaries; returns the path of that daemon's socket on the worker.
async fn install(daemon: &Daemon, record: &Record, bundle: &Path) -> anyhow::Result<String> {
    let name = &record.name;
    log(daemon, name, "Waiting for SSH");
    wait_for_ssh(daemon, record).await?;
    log(daemon, name, "Copying Chauffeur");
    let target = format!("{}:chauffeur-linux-x64.tar.gz", record.instance);
    let source = bundle.display().to_string();
    run(
        compute(daemon, record, &["scp", &source, &target]),
        "gcloud compute scp",
    )
    .await?;
    log(daemon, name, "Installing Chauffeur, Claude Code and Codex");
    let output = ssh_script(daemon, record, INSTALL).await?;
    let home = output
        .lines()
        .find_map(|line| line.strip_prefix("home="))
        .context("the installation on the worker did not say where its home is")?;
    let remote = format!("{}/.chauffeur/chauffeurd.sock", home.trim());
    change(daemon, name, |r| r.remote_socket = Some(remote.clone()));
    Ok(remote)
}

/// Brings a ready worker of this daemon that runs another Chauffeur version to this one, when a
/// transfer finds it behind, never when the Mac updates. Installing restarts its daemon, which
/// interrupts the agents working there, in any of its workspaces. Its forward reconnects by
/// itself; the caller waits for the new daemon.
pub(crate) async fn update(daemon: &Arc<Daemon>, name: &str) -> anyhow::Result<()> {
    let record = load(daemon, name)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .with_context(|| format!("No worker named {name}"))?;
    if record.state != WorkerState::Ready {
        bail!("The worker {name} is not ready; see Settings → Transfers");
    }
    if !daemon.workers.busy.lock().unwrap().insert(name.to_owned()) {
        bail!("{name} is already being changed; retry once it is done");
    }
    let outcome = async {
        log(
            daemon,
            name,
            format!("Updating Chauffeur to {}", env!("CARGO_PKG_VERSION")),
        );
        let bundle = bundle(daemon).await?;
        install(daemon, &record, &bundle).await?;
        log(daemon, name, format!("{name} is up to date"));
        anyhow::Ok(())
    }
    .await;
    daemon.workers.busy.lock().unwrap().remove(name);
    outcome.with_context(|| format!("Could not update {name} to this Chauffeur version"))
}

/// The bundle to install: `[workers] bundle`, else this version's from the GitHub release (with the
/// user's `gh`), checked against its published SHA-256 and kept for the next worker.
async fn bundle(daemon: &Daemon) -> anyhow::Result<PathBuf> {
    let configured = daemon.settings.get().workers.bundle.clone();
    if !configured.trim().is_empty() {
        let path = crate::settings::expand_home(configured.trim());
        if !path.is_file() {
            bail!(
                "The Linux bundle {} set in [workers] does not exist; build it with scripts/package-linux.sh",
                path.display()
            );
        }
        return Ok(path);
    }
    let Some(repo) = crate::update::release_repo() else {
        bail!(
            "This Chauffeur was built without a release repository: set [workers] bundle to a Linux bundle built with scripts/package-linux.sh"
        );
    };
    let version = env!("CARGO_PKG_VERSION");
    let asset = crate::update::worker_bundle_name(version);
    let dir = daemon.paths.home.join("workers");
    let path = dir.join(&asset);
    if path.is_file() {
        return Ok(path);
    }
    let download = dir.join(format!("download-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&download)?;
    let checksum = format!("{asset}.sha256");
    let mut command = Command::new("gh");
    command
        .env_clear()
        .envs(daemon.shell_env.vars())
        .stdin(Stdio::null())
        .args([
            "release",
            "download",
            &crate::update::release_tag(version),
            "--repo",
        ])
        .arg(&repo)
        .args(["--pattern", &asset, "--pattern", &checksum, "--dir"])
        .arg(&download);
    let fetched = run(command, "gh release download").await;
    let verified = fetched.and_then(|_| {
        let bytes = std::fs::read(download.join(&asset))?;
        let published = std::fs::read_to_string(download.join(&checksum))?;
        let actual = hex::encode(Sha256::digest(&bytes));
        if published.split_whitespace().next() != Some(actual.as_str()) {
            bail!("{asset} does not match its published SHA-256");
        }
        std::fs::rename(download.join(&asset), &path)?;
        Ok(())
    });
    let _ = std::fs::remove_dir_all(&download);
    verified.with_context(|| {
        format!(
            "Chauffeur {version} has no Linux bundle on GitHub; build one with scripts/package-linux.sh and set `bundle` in [workers]"
        )
    })?;
    Ok(path)
}

/// What Chauffeur reads of an instance.
#[derive(Deserialize)]
struct Instance {
    #[serde(default)]
    labels: BTreeMap<String, String>,
    /// `RUNNING`, `TERMINATED` once stopped…
    #[serde(default)]
    status: String,
}

/// The instance, or none when it does not exist.
async fn describe(daemon: &Daemon, record: &Record) -> anyhow::Result<Option<Instance>> {
    let output = compute(
        daemon,
        record,
        &["instances", "describe", &record.instance, "--format=json"],
    )
    .output()
    .await
    .map_err(not_installed)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("was not found") {
            return Ok(None);
        }
        bail!(
            "gcloud compute instances describe failed: {}",
            gcloud_error(&stderr)
        );
    }
    let instance: Instance =
        serde_json::from_slice(&output.stdout).context("unreadable answer from gcloud")?;
    Ok(Some(instance))
}

/// Refuses an instance this daemon did not create as this worker.
fn check_label(record: &Record, labels: &BTreeMap<String, String>) -> anyhow::Result<()> {
    if labels.get(LABEL) != Some(&record.name) {
        bail!(
            "{} in {} does not carry the label {LABEL}={}: Chauffeur only touches the workers it created",
            record.instance,
            record.project,
            record.name
        );
    }
    Ok(())
}

/// Probes SSH while the VM boots, for three minutes in all.
async fn wait_for_ssh(daemon: &Daemon, record: &Record) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    let mut last = None;
    while tokio::time::Instant::now() < deadline {
        let probe = run(
            compute(
                daemon,
                record,
                &["ssh", &record.instance, "--command", "true"],
            ),
            "gcloud compute ssh",
        );
        match tokio::time::timeout_at(deadline, probe).await {
            Ok(Ok(_)) => return Ok(()),
            Ok(Err(error)) => last = Some(format!("{error:#}")),
            Err(_) => last = Some("no answer".into()),
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    bail!(
        "{} did not answer over SSH within 3 minutes ({})",
        record.instance,
        last.unwrap_or_default()
    )
}

/// Runs `script` with bash on the worker; returns what it printed.
async fn ssh_script(daemon: &Daemon, record: &Record, script: &str) -> anyhow::Result<String> {
    let mut command = compute(
        daemon,
        record,
        &["ssh", &record.instance, "--command", "bash -s"],
    );
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(not_installed)?;
    let mut stdin = child.stdin.take().context("no stdin for gcloud")?;
    stdin.write_all(script.as_bytes()).await?;
    drop(stdin);
    let output = child.wait_with_output().await?;
    if !output.status.success() {
        bail!(
            "the installation on the worker failed: {}",
            gcloud_error(&String::from_utf8_lossy(&output.stderr))
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn forward_log(daemon: &Daemon, name: &str) -> PathBuf {
    daemon
        .paths
        .home
        .join("workers")
        .join(format!("{name}.log"))
}

fn start_forward(daemon: &Arc<Daemon>, name: &str, remote: &str, record: &Record) -> Arc<Forward> {
    let forward = Arc::new(Forward::default());
    if let Some(old) = daemon
        .workers
        .forwards
        .lock()
        .unwrap()
        .insert(name.to_owned(), forward.clone())
    {
        old.stop.cancel();
        old.kill();
    }
    tokio::spawn(keep_forward(
        daemon.clone(),
        record.clone(),
        remote.to_owned(),
        forward.clone(),
    ));
    forward
}

/// Keeps `ssh -N -L <local socket>:<worker's socket>` running: restarted when it ends, and when the
/// worker's daemon stops answering through it, with a growing pause. It runs `ssh` with the
/// worker's SSH entry, so that gcloud's session expiring never cuts it; `gcloud compute ssh` only
/// while the worker has no entry yet. A run that never reached the worker's daemon leaves why.
async fn keep_forward(daemon: Arc<Daemon>, record: Record, remote: String, forward: Arc<Forward>) {
    let local = socket_path(&daemon, &record.name);
    let log_file = forward_log(&daemon, &record.name);
    if let Some(dir) = local.parent()
        && let Err(error) =
            std::fs::create_dir_all(dir).and_then(|()| crate::paths::private_dir(dir))
    {
        tracing::warn!(%error, worker = %record.name, "could not create the workers' folder");
    }
    let mut pause = Duration::from_secs(1);
    let mut entry_checked: Option<tokio::time::Instant> = None;
    while !forward.stop.is_cancelled() {
        let _ = std::fs::remove_file(&local);
        let binding = format!("{}:{remote}", local.display());
        let options = [
            "-N",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "StreamLocalBindUnlink=yes",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
            "-L",
            &binding,
        ];
        // Read again at each run: a refreshed entry carries the worker's new address.
        let current = load(&daemon, &record.name)
            .ok()
            .flatten()
            .unwrap_or_else(|| record.clone());
        let mut command = match &current.ssh {
            Some(entry) => ssh(&daemon, entry, &options),
            None => compute(
                &daemon,
                &current,
                &[&["ssh", current.instance.as_str(), "--"], &options[..]].concat(),
            ),
        };
        command.process_group(0).stdout(Stdio::null());
        let from = std::fs::metadata(&log_file).map_or(0, |m| m.len());
        if let Ok(log) = crate::paths::open_private_log(&log_file) {
            command.stderr(log);
        }
        let reached = match command.spawn() {
            Ok(mut child) => {
                forward
                    .group
                    .store(child.id().map_or(0, |id| id as i32), Ordering::SeqCst);
                let reached = watch(&forward, &mut child, &local).await;
                forward.kill();
                let _ = child.wait().await;
                reached
            }
            Err(error) => {
                tracing::warn!(%error, worker = %record.name, "could not start the worker's forward");
                *forward.issue.lock().unwrap() = Some(ForwardIssue {
                    message: format!("could not start ssh: {error}"),
                    lasting: true,
                    at: tokio::time::Instant::now(),
                });
                false
            }
        };
        forward.connected.store(false, Ordering::SeqCst);
        if !reached && !forward.stop.is_cancelled() {
            let mut issue = issue_of(&output_since(&log_file, from), &log_file);
            if current.ssh.is_some() && entry_checked.is_none_or(|at| at.elapsed() >= ENTRY_RECHECK)
            {
                entry_checked = Some(tokio::time::Instant::now());
                if let Err(error) = refresh_ssh_entry(&daemon, &current).await {
                    tracing::warn!(error = format!("{error:#}"), worker = %record.name, "could not refresh the worker's ssh entry");
                    issue.message = format!(
                        "{}; its address could not be checked: {error:#}",
                        issue.message
                    );
                }
            }
            *forward.issue.lock().unwrap() = Some(issue);
        }
        tokio::select! {
            _ = forward.stop.cancelled() => {}
            _ = tokio::time::sleep(pause) => {}
        }
        pause = (pause * 2).min(Duration::from_secs(30));
    }
    // A forward replaced by a retried creation shares its socket with the new one: only the
    // current forward, or one stopped with its worker, takes it away.
    let replaced = daemon
        .workers
        .forwards
        .lock()
        .unwrap()
        .get(&record.name)
        .is_some_and(|current| !Arc::ptr_eq(current, &forward));
    if !replaced {
        let _ = std::fs::remove_file(&local);
    }
}

/// What a run of the forward wrote to its log, at most its last 8 KB.
fn output_since(log: &Path, from: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let mut bytes = Vec::new();
    let read = std::fs::File::open(log).and_then(|mut file| {
        let end = file.metadata()?.len();
        file.seek(SeekFrom::Start(
            from.min(end).max(end.saturating_sub(8 * 1024)),
        ))?;
        file.read_to_end(&mut bytes)
    });
    match read {
        Ok(_) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => String::new(),
    }
}

/// Why a run of the forward ended without reaching the worker's daemon, from what it printed: ssh's
/// own message, before gcloud's advice and exit line when gcloud ran it.
fn issue_of(output: &str, log: &Path) -> ForwardIssue {
    let at = tokio::time::Instant::now();
    if signed_out(output) {
        return ForwardIssue {
            message: SIGNED_OUT.into(),
            lasting: true,
            at,
        };
    }
    let lasting = [
        "Permission denied",
        "Host key verification failed",
        "REMOTE HOST IDENTIFICATION HAS CHANGED",
    ]
    .iter()
    .any(|refusal| output.contains(refusal));
    let message = output
        .lines()
        .map(str::trim)
        .take_while(|l| !l.starts_with("Recommendation:") && !l.starts_with("ERROR: (gcloud"))
        .filter(|l| !l.is_empty())
        .last()
        .map(str::to_owned)
        .unwrap_or_else(|| match gcloud_error(output).as_str() {
            "no output" => format!("its ssh ended without saying why; see {}", log.display()),
            error => error.to_owned(),
        });
    ForwardIssue {
        message,
        lasting,
        at,
    }
}

/// Follows one run of the forward until it ends, is stopped, or the worker's daemon stopped
/// answering through it three times in a row. Whether the worker's daemon answered through it.
async fn watch(forward: &Forward, child: &mut tokio::process::Child, local: &Path) -> bool {
    let mut failures = 0;
    let mut reached = false;
    loop {
        let every = if forward.connected.load(Ordering::SeqCst) {
            Duration::from_secs(15)
        } else {
            Duration::from_secs(1)
        };
        tokio::select! {
            _ = forward.stop.cancelled() => return reached,
            status = child.wait() => {
                tracing::info!(?status, "a worker's forward ended");
                return reached;
            }
            _ = tokio::time::sleep(every) => {}
        }
        let answers = tokio::time::timeout(
            Duration::from_secs(10),
            crate::cloud_transfer_driver::Peer::connect(local),
        )
        .await
        .is_ok_and(|peer| peer.is_ok());
        if answers {
            failures = 0;
            if !reached {
                reached = true;
                *forward.issue.lock().unwrap() = None;
            }
        } else if forward.connected.load(Ordering::SeqCst) {
            failures += 1;
            if failures >= 3 {
                return reached;
            }
            continue;
        }
        forward.connected.store(answers, Ordering::SeqCst);
    }
}

/// The `.ssh` folder whose `config` includes the workers' entries: the one named explicitly, else
/// the user's own `~/.ssh` for the daemon whose state is `~/.chauffeur`; none for another daemon
/// (a throwaway one), which must never point the user's ssh at its own workers.
pub(crate) fn ssh_dir_for(
    state: &Path,
    user_home: Option<&Path>,
    explicit: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(dir) = explicit {
        return Some(dir.to_path_buf());
    }
    let home = user_home?;
    (state == home.join(".chauffeur")).then(|| home.join(".ssh"))
}

fn ssh_dir(daemon: &Daemon) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    ssh_dir_for(
        &daemon.paths.home,
        home.as_deref(),
        daemon.workers.ssh_dir.read().unwrap().as_deref(),
    )
}

/// The `Host` a worker's entry has: unique to this daemon's state folder, so that two daemons'
/// workers of one name never shadow each other in the user's ssh config.
fn ssh_alias(daemon: &Daemon, name: &str) -> String {
    let digest = hex::encode(Sha256::digest(
        daemon.paths.home.as_os_str().as_encoded_bytes(),
    ));
    format!("chauffeur-{}-{name}", &digest[..6])
}

/// Whether a worker opens in an editor of this machine now, and what to do when it does not.
pub enum RemoteHost {
    Host(String),
    Unavailable(String),
}

/// How the editors of this machine reach `machine`; none when it is not a worker.
pub fn remote_host(daemon: &Daemon, machine: &str) -> Option<RemoteHost> {
    Some(remote_host_of(
        daemon,
        &load(daemon, machine).ok().flatten()?,
    ))
}

fn remote_host_of(daemon: &Daemon, record: &Record) -> RemoteHost {
    let name = &record.name;
    if let Err(error) = refuse_recovery_worker(daemon, name)
        .and_then(|()| crate::worker_lifecycle::allow_use(daemon, name))
    {
        return RemoteHost::Unavailable(error.to_string());
    }
    if record.state != WorkerState::Ready {
        return RemoteHost::Unavailable(format!(
            "The worker {name} is not ready; see Settings → Transfers"
        ));
    }
    if record.ssh.is_none() {
        return RemoteHost::Unavailable(format!(
            "{name} has no SSH entry yet{}: run `gcloud auth login` if gcloud is logged out, then restart Chauffeur",
            record
                .ssh_error
                .as_deref()
                .map(|e| format!(" ({e})"))
                .unwrap_or_default()
        ));
    }
    let file = crate::worker_ssh::config_file(&daemon.paths.home);
    let line = crate::worker_ssh::include_line(&file).unwrap_or_else(|e| format!("{e:#}"));
    match ssh_dir(daemon) {
        Some(dir) if crate::worker_ssh::is_included(&dir, &file) => {
            RemoteHost::Host(ssh_alias(daemon, name))
        }
        Some(dir) => RemoteHost::Unavailable(format!(
            "Add `{line}` at the top of {}, then open it again",
            dir.join("config").display()
        )),
        None => RemoteHost::Unavailable(format!(
            "This daemon's state is not ~/.chauffeur: add `{line}` at the top of your ssh config to open {name}'s workspaces in an editor"
        )),
    }
}

/// The ssh host the editors of this machine open a ready worker with, once its entry is written
/// and included.
pub fn ssh_host(daemon: &Daemon, name: &str) -> Option<String> {
    match remote_host(daemon, name)? {
        RemoteHost::Host(host) => Some(host),
        RemoteHost::Unavailable(_) => None,
    }
}

/// Adds the `Include` of the entries' file to the ssh config, once for this daemon: after that, a
/// user who took it out keeps it out. Returns the log line saying it was added.
fn include_once(daemon: &Daemon) -> anyhow::Result<Option<String>> {
    if daemon.store.kv_get(INCLUDE_DONE)?.is_some() {
        return Ok(None);
    }
    let file = crate::worker_ssh::config_file(&daemon.paths.home);
    let Some(dir) = ssh_dir(daemon) else {
        bail!(
            "This daemon's state is not ~/.chauffeur, so ~/.ssh is left alone: add `{}` at the top of your ssh config to open its workers in an editor",
            crate::worker_ssh::include_line(&file)?
        );
    };
    let added = {
        let _files = daemon.workers.ssh_files.lock().unwrap();
        crate::worker_ssh::ensure_included(&dir, &file)?
    };
    daemon.store.kv_set(INCLUDE_DONE, "1")?;
    Ok(added.then(|| {
        format!(
            "Added `{}` at the top of {}, once: nothing else there is changed",
            crate::worker_ssh::include_line(&file).unwrap_or_default(),
            dir.join("config").display()
        )
    }))
}

/// Writes the worker's ssh entry, and the `Include` of the entries' file in the ssh config the
/// first time. Returns the log lines saying what it wrote.
async fn add_ssh_entry(daemon: &Daemon, record: &Record) -> anyhow::Result<Vec<String>> {
    refresh_ssh_entry(daemon, record).await?;
    let file = crate::worker_ssh::config_file(&daemon.paths.home);
    let mut lines = vec![format!(
        "SSH host {} for Cursor and VS Code (Remote-SSH), in {}",
        ssh_alias(daemon, &record.name),
        file.display()
    )];
    lines.extend(include_once(daemon)?);
    Ok(lines)
}

/// Reads how `gcloud compute ssh` reaches the worker now (its address may change) and rewrites the
/// entries' file. The ssh config is left as it is.
async fn refresh_ssh_entry(daemon: &Daemon, record: &Record) -> anyhow::Result<()> {
    let read = async {
        let output = run(
            compute(daemon, record, &["ssh", &record.instance, "--dry-run"]),
            "gcloud compute ssh --dry-run",
        )
        .await?;
        crate::worker_ssh::parse_dry_run(&output)
    };
    match read.await {
        Ok(entry) => change(daemon, &record.name, |r| {
            r.ssh = Some(entry);
            r.ssh_error = None;
        }),
        Err(error) => {
            change(daemon, &record.name, |r| {
                r.ssh_error = Some(format!("{error:#}"))
            });
            return Err(error);
        }
    }
    write_ssh_config(daemon)
}

/// The entries' file, from the workers that have one.
fn write_ssh_config(daemon: &Daemon) -> anyhow::Result<()> {
    let _files = daemon.workers.ssh_files.lock().unwrap();
    let entries: Vec<(String, crate::worker_ssh::SshEntry)> = records(daemon)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .into_iter()
        .filter_map(|r| Some((ssh_alias(daemon, &r.name), r.ssh?)))
        .collect();
    crate::worker_ssh::write(
        &crate::worker_ssh::config_file(&daemon.paths.home),
        &entries,
    )
}

/// Deletes a worker and its disk in the background. Refused while a workspace of this daemon
/// runs there or is being transferred to or from it.
pub fn delete(daemon: &Arc<Daemon>, params: WorkerParams) -> Result<WorkerInfo> {
    let name = params.name.trim().to_owned();
    refuse_recovery_worker(daemon, &name)?;
    crate::worker_lifecycle::allow_delete(daemon, &name)?;
    let mut record =
        load(daemon, &name)?.ok_or_else(|| Error::NotFound(format!("No worker named {name}")))?;
    for row in daemon.store.list_workspaces()? {
        if crate::cloud_transfer_driver::moved_to(daemon, &row.id).as_deref() == Some(&name) {
            return Err(Error::Conflict(format!(
                "{} runs on {name}: bring it back before deleting {name}",
                row.slug
            )));
        }
    }
    for transfer in crate::cloud_transfer_driver::running(daemon) {
        if transfer.source == name || transfer.destination == name {
            return Err(Error::Conflict(format!(
                "{} is being transferred with {name}; delete {name} once it is done",
                transfer.slug
            )));
        }
    }
    if !daemon.workers.busy.lock().unwrap().insert(name.clone()) {
        return Err(Error::Conflict(format!(
            "{name} is being created or deleted; wait for it"
        )));
    }
    daemon.workers.stop_forward(&name);
    record.state = WorkerState::Deleting;
    record.error = None;
    record
        .log
        .push(format!("Deleting the VM {}", record.instance));
    save(daemon, &record)?;
    let started = info(daemon, &record);
    let daemon = daemon.clone();
    tokio::spawn(async move {
        match destroy(&daemon, &record).await {
            Ok(()) => {
                if let Err(error) = remove(&daemon, &name) {
                    tracing::warn!(%error, worker = %name, "could not forget the deleted worker");
                } else {
                    crate::worker_lifecycle::forget(&daemon, &name);
                }
                let _ = std::fs::remove_file(socket_path(&daemon, &name));
                let _ = std::fs::remove_file(forward_log(&daemon, &name));
                if let Err(error) = write_ssh_config(&daemon) {
                    tracing::warn!(error = format!("{error:#}"), worker = %name, "could not remove the worker's ssh entry");
                }
            }
            Err(error) => {
                tracing::warn!(error = format!("{error:#}"), worker = %name, "could not delete the worker");
                change(&daemon, &name, |r| {
                    r.state = WorkerState::Failed;
                    r.error = Some(format!("Could not delete it: {error:#}"));
                });
            }
        }
        daemon.workers.busy.lock().unwrap().remove(&name);
    });
    Ok(started)
}

/// The worker was installed once: its disk holds Chauffeur and its agents' logins.
pub(crate) fn installed(daemon: &Daemon, name: &str) -> Result<bool> {
    Ok(load(daemon, name)?.is_some_and(|r| r.remote_socket.is_some()))
}

/// Stops a worker's VM, its disk kept: stopped, only the disk costs, and the agents' logins stay
/// for the next transfer to Cloud, which starts it again. The caller checked that nothing runs or
/// is reserved there; a failure leaves it failed, to stop or delete again.
pub(crate) async fn stop(daemon: &Daemon, name: &str) -> anyhow::Result<()> {
    let Some(mut record) = load(daemon, name).map_err(|e| anyhow::anyhow!("{e}"))? else {
        return Ok(());
    };
    if !daemon.workers.busy.lock().unwrap().insert(name.to_owned()) {
        return Ok(());
    }
    daemon.workers.stop_forward(name);
    record.state = WorkerState::Stopping;
    record.error = None;
    record.log.push(format!(
        "Stopping the VM {}, its disk kept",
        record.instance
    ));
    let outcome = async {
        save(daemon, &record).map_err(|e| anyhow::anyhow!("{e}"))?;
        if let Some(instance) = describe(daemon, &record).await? {
            check_label(&record, &instance.labels)?;
            run(
                compute(daemon, &record, &["instances", "stop", &record.instance]),
                "gcloud compute instances stop",
            )
            .await?;
        }
        anyhow::Ok(())
    }
    .await;
    match &outcome {
        Ok(()) => change(daemon, name, |r| {
            r.state = WorkerState::Stopped;
            r.log.push(format!(
                "Stopped: only its {DISK_GB} GB disk costs now, about ${:.2} a month",
                f64::from(DISK_GB) * DISK_USD_PER_GB_MONTH
            ));
        }),
        Err(error) => change(daemon, name, |r| {
            r.state = WorkerState::Failed;
            r.error = Some(format!("Could not stop it: {error:#}"));
        }),
    }
    daemon.workers.busy.lock().unwrap().remove(name);
    outcome
}

async fn destroy(daemon: &Daemon, record: &Record) -> anyhow::Result<()> {
    if let Some(instance) = describe(daemon, record).await? {
        check_label(record, &instance.labels)?;
        run(
            compute(
                daemon,
                record,
                &[
                    "instances",
                    "delete",
                    &record.instance,
                    "--delete-disks=all",
                ],
            ),
            "gcloud compute instances delete",
        )
        .await?;
    }
    remove_backup_access(daemon, record).await
}

/// The bucket workspaces running on workers are backed up to: `[workers] backup_bucket`, else
/// one per project (docs/cloud-workspace-backups.md).
pub fn backup_bucket(settings: &crate::settings::WorkerSettings) -> String {
    match settings.backup_bucket.trim() {
        "" => format!("{}-chauffeur-backups", settings.project),
        bucket => bucket.to_owned(),
    }
}

const STORAGE_SCOPE: &str = "https://www.googleapis.com/auth/devstorage.read_write";
/// Create, read, list and delete: no predefined role covers those without more (objectCreator
/// cannot read or delete, objectViewer cannot write, together they cannot delete), and a custom
/// role would be one more change to the project.
const BACKUP_ROLE: &str = "roles/storage.objectUser";
const SOFT_DELETE: &str = "7d";
/// What marks a bucket Chauffeur made: only that one has its soft delete set by Chauffeur.
const MADE_LABEL: &str = "chauffeur-backups=true";
/// What the service accounts Chauffeur makes for its workers are named, so that it never deletes
/// another one.
const ACCOUNT_NAME: &str = "Chauffeur worker";

/// The worker's own service account: `cw-backup-<name>` (service account ids are 30 characters at
/// most, worker names 20).
fn backup_account(record: &Record) -> String {
    format!(
        "cw-backup-{}@{}.iam.gserviceaccount.com",
        record.name, record.project
    )
}

/// Before the VM: the backup bucket (made in Paris when missing), the worker's service account,
/// and its access to its prefix of the bucket only. Returns the account, which the VM runs as.
async fn prepare_backups(daemon: &Daemon, record: &Record) -> anyhow::Result<String> {
    // The one a failed attempt was given access to, whatever the settings say since: its binding
    // is the one deleting the worker takes back.
    let bucket = record
        .backup_bucket
        .clone()
        .unwrap_or_else(|| backup_bucket(&daemon.settings.get().workers));
    let url = format!("gs://{bucket}");
    match describe_bucket(daemon, &url).await? {
        Some(found) => check_bucket(daemon, record, &url, &found).await?,
        None => {
            log(
                daemon,
                &record.name,
                format!("Creating the backup bucket {url}"),
            );
            let created = run(
                gcloud(
                    daemon,
                    &[
                        "storage",
                        "buckets",
                        "create",
                        &url,
                        "--project",
                        &record.project,
                        "--location",
                        REGION,
                        "--default-storage-class",
                        "STANDARD",
                        "--uniform-bucket-level-access",
                        "--public-access-prevention",
                        // A backup a process of the worker deletes or overwrites stays
                        // recoverable a week (docs/cloud-workspace-backups.md).
                        "--soft-delete-duration",
                        SOFT_DELETE,
                    ],
                ),
                "gcloud storage buckets create",
            )
            .await;
            if created.is_ok() {
                run(
                    gcloud(
                        daemon,
                        &[
                            "storage",
                            "buckets",
                            "update",
                            &url,
                            "--update-labels",
                            MADE_LABEL,
                        ],
                    ),
                    "gcloud storage buckets update",
                )
                .await?;
            }
            // Another worker being created at the same time may have made it first.
            match describe_bucket(daemon, &url).await? {
                Some(found) => check_bucket(daemon, record, &url, &found).await?,
                None => {
                    created?;
                }
            }
        }
    }
    let prefix = match &record.backup_prefix {
        Some(prefix) => prefix.clone(),
        None => {
            let prefix = format!("{}-{:08x}", record.name, rand::random::<u32>());
            change(daemon, &record.name, |r| {
                r.backup_prefix = Some(prefix.clone())
            });
            prefix
        }
    };
    let account = backup_account(record);
    let made = record.backup_account.as_deref() == Some(account.as_str());
    match account_state(daemon, record, &account).await? {
        Account::Named(name) if made && name.starts_with(ACCOUNT_NAME) => {}
        Account::Named(_) => bail!(
            "{account} exists and Chauffeur did not make it for this worker: Chauffeur only touches the accounts it created; name the worker otherwise"
        ),
        // An account deleted lately reads as refused "or it may not exist" (seen in the real
        // trial): creating one is what then says whether the user may.
        Account::Missing | Account::Unsure => {
            log(
                daemon,
                &record.name,
                format!("Creating its service account {account}"),
            );
            let id = format!("cw-backup-{}", record.name);
            let display = format!("{ACCOUNT_NAME} {}", record.name);
            run(
                gcloud(
                    daemon,
                    &[
                        "iam",
                        "service-accounts",
                        "create",
                        &id,
                        "--project",
                        &record.project,
                        "--display-name",
                        &display,
                        "--description",
                        "Writes the encrypted backups of the workspaces running on this Chauffeur worker, in its prefix of the backup bucket only",
                    ],
                ),
                "gcloud iam service-accounts create",
            )
            .await?;
            // Recorded once made, never before: only an account Chauffeur did make is deleted
            // with the worker.
            change(daemon, &record.name, |r| {
                r.backup_account = Some(account.clone())
            });
        }
    }
    log(
        daemon,
        &record.name,
        format!("Letting it write {url}/{prefix}/ only"),
    );
    // Recorded first: deleting the worker takes its access back even if this stops halfway.
    change(daemon, &record.name, |r| {
        r.backup_bucket = Some(bucket.clone())
    });
    // A managed folder, not a condition on object names: writing with gcloud lists the bucket,
    // which a condition on names never allows, while the folder's own policy covers listing it.
    let folder = format!("{url}/{prefix}/");
    let made = run(
        gcloud(daemon, &["storage", "managed-folders", "create", &folder]),
        "gcloud storage managed-folders create",
    )
    .await;
    if let Err(error) = made
        && !format!("{error:#}").contains("HTTPError 409")
        && !format!("{error:#}").contains("already exists")
    {
        return Err(error);
    }
    let member = format!("serviceAccount:{account}");
    // A service account just created takes a moment to be known to IAM; any other refusal is final.
    let mut tries = 0;
    loop {
        let bound = run(
            gcloud(
                daemon,
                &[
                    "storage",
                    "managed-folders",
                    "add-iam-policy-binding",
                    &folder,
                    "--member",
                    &member,
                    "--role",
                    BACKUP_ROLE,
                ],
            ),
            "gcloud storage managed-folders add-iam-policy-binding",
        )
        .await;
        match bound {
            Ok(_) => break,
            Err(error)
                if tries < NOT_KNOWN_YET_TRIES && unknown_yet(&format!("{error:#}"), &account) =>
            {
                tries += 1;
                tracing::debug!(
                    error = format!("{error:#}"),
                    "the worker's account is not known yet"
                );
                tokio::time::sleep(NOT_KNOWN_YET_PAUSE).await;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(account)
}

/// Storage's answer while IAM does not know `account`, just created, yet: one naming it as missing,
/// or a 412 on the policy as gcloud prints it. Never a bare number, which a prefix or a project
/// may hold, nor a missing folder or bucket, which does not name the account.
fn unknown_yet(error: &str, account: &str) -> bool {
    error.contains("HTTPError 412")
        || (error.contains(account)
            && ["does not exist", "not found", "NOT_FOUND"]
                .iter()
                .any(|sign| error.contains(sign)))
}

/// Where a worker's backups go: the bucket it was given access to, and its prefix there. A worker
/// made before backups has neither: it cannot write any.
pub fn backup_place(daemon: &Daemon, name: &str) -> anyhow::Result<(String, String)> {
    let record = load(daemon, name)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .with_context(|| format!("No worker named {name}"))?;
    match (record.backup_bucket, record.backup_prefix) {
        (Some(bucket), Some(prefix)) => Ok((bucket, prefix)),
        _ => bail!("{name} was created before backups: delete it and create it again"),
    }
}

/// A bucket as the storage API gives it (`--raw`: gcloud's own format has no project), or none
/// when it does not exist.
async fn describe_bucket(daemon: &Daemon, url: &str) -> anyhow::Result<Option<serde_json::Value>> {
    let output = gcloud(
        daemon,
        &[
            "storage",
            "buckets",
            "describe",
            url,
            "--raw",
            "--format=json",
        ],
    )
    .output()
    .await
    .map_err(not_installed)?;
    if output.status.success() {
        return Ok(Some(
            serde_json::from_slice(&output.stdout).context("unreadable answer from gcloud")?,
        ));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("not found") || stderr.contains("HTTPError 404") {
        return Ok(None);
    }
    bail!(
        "gcloud storage buckets describe failed: {}",
        gcloud_error(&stderr)
    );
}

/// In the worker's project, in Paris, with uniform access (which the workers' managed folders
/// need). A bucket Chauffeur made that keeps no deleted objects is given its week; another one is
/// used as it is, said in the worker's log.
async fn check_bucket(
    daemon: &Daemon,
    record: &Record,
    url: &str,
    found: &serde_json::Value,
) -> anyhow::Result<()> {
    let number = |value: &serde_json::Value| {
        value
            .as_str()
            .map(str::to_owned)
            .or_else(|| value.as_u64().map(|n| n.to_string()))
    };
    let owner = number(&found["projectNumber"]);
    let project = run(
        gcloud(
            daemon,
            &[
                "projects",
                "describe",
                &record.project,
                "--format=value(projectNumber)",
            ],
        ),
        "gcloud projects describe",
    )
    .await?;
    if owner.as_deref() != Some(project.trim()) {
        bail!(
            "{url} is not a bucket of {}: set [workers] backup_bucket to a name of your own there",
            record.project
        );
    }
    let location = found["location"].as_str().unwrap_or_default();
    if !location.eq_ignore_ascii_case(REGION) {
        bail!(
            "{url} is in {location}, not in Paris ({REGION}): set [workers] backup_bucket to a bucket there"
        );
    }
    if found["iamConfiguration"]["uniformBucketLevelAccess"]["enabled"].as_bool() != Some(true) {
        bail!(
            "{url} has no uniform bucket-level access, which limiting each worker to its prefix needs: turn it on, or set [workers] backup_bucket to another bucket"
        );
    }
    let retention = &found["softDeletePolicy"]["retentionDurationSeconds"];
    let seconds = retention
        .as_u64()
        .or_else(|| retention.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0);
    if seconds == 0 {
        if found["labels"]["chauffeur-backups"].as_str() == Some("true") {
            log(
                daemon,
                &record.name,
                format!("Keeping deleted backups of {url} a week"),
            );
            run(
                gcloud(
                    daemon,
                    &[
                        "storage",
                        "buckets",
                        "update",
                        url,
                        "--soft-delete-duration",
                        SOFT_DELETE,
                    ],
                ),
                "gcloud storage buckets update",
            )
            .await?;
        } else {
            log(
                daemon,
                &record.name,
                format!(
                    "{url} keeps no deleted objects: a backup deleted or overwritten on the worker is gone for good"
                ),
            );
        }
    }
    Ok(())
}

/// What IAM says of a service account.
enum Account {
    /// It exists, with this display name.
    Named(String),
    /// NOT_FOUND: it does not exist.
    Missing,
    /// Refused "or it may not exist": so reads an account deleted lately, and one the user may
    /// not read.
    Unsure,
}

async fn account_state(daemon: &Daemon, record: &Record, account: &str) -> anyhow::Result<Account> {
    let output = gcloud(
        daemon,
        &[
            "iam",
            "service-accounts",
            "describe",
            account,
            "--project",
            &record.project,
            "--format=json",
        ],
    )
    .output()
    .await
    .map_err(not_installed)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("NOT_FOUND") {
            return Ok(Account::Missing);
        }
        if stderr.contains("or it may not exist") {
            return Ok(Account::Unsure);
        }
        bail!(
            "gcloud iam service-accounts describe failed: {}",
            gcloud_error(&stderr)
        );
    }
    #[derive(Deserialize)]
    struct Found {
        #[serde(default, rename = "displayName")]
        display_name: String,
    }
    let found: Found =
        serde_json::from_slice(&output.stdout).context("unreadable answer from gcloud")?;
    Ok(Account::Named(found.display_name))
}

/// Once its VM is gone: its access to the bucket, then its service account. The backups it wrote
/// stay; deleting a workspace's backups is the return's job.
async fn remove_backup_access(daemon: &Daemon, record: &Record) -> anyhow::Result<()> {
    let account = backup_account(record);
    let folder = record
        .backup_bucket
        .as_ref()
        .zip(record.backup_prefix.as_ref())
        .map(|(bucket, prefix)| format!("gs://{bucket}/{prefix}/"));
    if let Some(folder) = &folder {
        let member = format!("serviceAccount:{account}");
        let removed = run(
            gcloud(
                daemon,
                &[
                    "storage",
                    "managed-folders",
                    "remove-iam-policy-binding",
                    folder,
                    "--member",
                    &member,
                    "--role",
                    BACKUP_ROLE,
                ],
            ),
            "gcloud storage managed-folders remove-iam-policy-binding",
        )
        .await;
        // Asked again after a failure, the binding or the folder may be gone already.
        if let Err(error) = removed
            && !format!("{error:#}").contains("not found")
        {
            return Err(error);
        }
    }
    remove_account(daemon, record, &account).await?;
    // The folder goes once empty; its backups stay until the workspace's are deleted.
    if let Some(folder) = &folder
        && let Err(error) = run(
            gcloud(daemon, &["storage", "managed-folders", "delete", folder]),
            "gcloud storage managed-folders delete",
        )
        .await
    {
        tracing::info!(
            error = format!("{error:#}"),
            folder,
            "kept a worker's backup folder"
        );
    }
    Ok(())
}

/// Its service account, if Chauffeur made it for this worker. Gone only once IAM says NOT_FOUND:
/// a refusal "or it may not exist" may hide an account the user may not delete, which recreating
/// the worker would then find in its way.
async fn remove_account(daemon: &Daemon, record: &Record, account: &str) -> anyhow::Result<()> {
    if record.backup_account.as_deref() != Some(account) {
        return Ok(());
    }
    match account_state(daemon, record, account).await? {
        Account::Named(name) if !name.starts_with(ACCOUNT_NAME) => bail!(
            "{account} is not a Chauffeur worker's: Chauffeur only deletes the accounts it created"
        ),
        Account::Missing => {}
        Account::Named(_) | Account::Unsure => {
            let deleted = run(
                gcloud(
                    daemon,
                    &[
                        "iam",
                        "service-accounts",
                        "delete",
                        account,
                        "--project",
                        &record.project,
                        "--quiet",
                    ],
                ),
                "gcloud iam service-accounts delete",
            )
            .await;
            if let Err(error) = deleted
                && !format!("{error:#}").contains("NOT_FOUND")
                && !matches!(
                    account_state(daemon, record, account).await?,
                    Account::Missing
                )
            {
                return Err(error);
            }
        }
    }
    // Deleted: asked again after a later step fails, the worker does not look for it.
    change(daemon, &record.name, |r| r.backup_account = None);
    Ok(())
}

/// At start: a ready worker gets its forward back; one this daemon was creating or deleting when it
/// stopped is left failed, to create or delete again. Returns the task refreshing the ready
/// workers' ssh entries, if any.
pub fn resume(daemon: &Arc<Daemon>) -> Option<tokio::task::JoinHandle<()>> {
    let all = match records(daemon) {
        Ok(all) => all,
        Err(error) => {
            tracing::warn!(%error, "could not read the workers");
            return None;
        }
    };
    let mut ready = Vec::new();
    for record in all {
        if refuse_recovery_worker(daemon, &record.name).is_err() {
            change(daemon, &record.name, |r| r.state = WorkerState::Failed);
            continue;
        }
        match (record.state, &record.remote_socket) {
            // A stop the daemon did not see through: ready again, the cleanup stops it again.
            (WorkerState::Ready | WorkerState::Stopping, Some(remote)) => {
                change(daemon, &record.name, |r| r.state = WorkerState::Ready);
                if crate::worker_lifecycle::allow_use(daemon, &record.name).is_err() {
                    continue;
                }
                start_forward(daemon, &record.name, remote, &record);
                ready.push(record.clone());
            }
            (WorkerState::Creating | WorkerState::Installing | WorkerState::Deleting, _) => {
                let doing = if record.state == WorkerState::Deleting {
                    "deleting"
                } else {
                    "creating"
                };
                change(daemon, &record.name, |r| {
                    r.state = WorkerState::Failed;
                    r.error = Some(format!(
                        "Chauffeur stopped while {doing} it: create it again or delete it"
                    ));
                });
            }
            _ => {}
        }
    }
    if ready.is_empty() {
        return None;
    }
    // Entries read again (an address may change); a worker created before entries existed gets
    // the include once too.
    let daemon = daemon.clone();
    Some(tokio::spawn(async move {
        for record in &ready {
            if let Err(error) = refresh_ssh_entry(&daemon, record).await {
                tracing::warn!(error = format!("{error:#}"), worker = %record.name, "could not refresh the worker's ssh entry");
            }
        }
        match include_once(&daemon) {
            Ok(Some(line)) => ready
                .iter()
                .for_each(|r| log(&daemon, &r.name, line.clone())),
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(
                    error = format!("{error:#}"),
                    "could not include the workers' ssh entries"
                )
            }
        }
    }))
}

pub(crate) fn gcloud(daemon: &Daemon, args: &[&str]) -> Command {
    let mut command = Command::new(&*daemon.workers.gcloud.read().unwrap());
    command
        .env_clear()
        .envs(daemon.shell_env.vars())
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .args(args);
    command
}

/// `gcloud compute <args>` on the worker's project and zone, never the user's default ones; flags
/// go before an `ssh` command's `--`.
fn compute(daemon: &Daemon, record: &Record, args: &[&str]) -> Command {
    let split = args.iter().position(|a| *a == "--").unwrap_or(args.len());
    let mut command = gcloud(daemon, &["compute"]);
    command
        .args(&args[..split])
        .args([
            "--project",
            &record.project,
            "--zone",
            &record.zone,
            "--quiet",
        ])
        .args(&args[split..]);
    command
}

/// `ssh` to the worker with its entry, never asking anything: a forward has no one to answer.
fn ssh(daemon: &Daemon, entry: &crate::worker_ssh::SshEntry, args: &[&str]) -> Command {
    let mut command = Command::new(&*daemon.workers.ssh.read().unwrap());
    command
        .env_clear()
        .envs(daemon.shell_env.vars())
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .args(crate::worker_ssh::options(entry))
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=20"])
        .args(args)
        .arg(format!("{}@{}", entry.user, entry.host_name));
    command
}

/// gcloud's session expired or was revoked: no command of it works until `gcloud auth login`.
fn signed_out(stderr: &str) -> bool {
    stderr.contains("gcloud auth login")
        && (stderr.contains("Reauthentication")
            || stderr.contains("refreshing your current auth tokens")
            || stderr.contains("You do not currently have an active account"))
}

fn not_installed(error: std::io::Error) -> anyhow::Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        anyhow::anyhow!(
            "gcloud is not installed: install the Google Cloud CLI, run `gcloud auth login`, then retry"
        )
    } else {
        anyhow::Error::new(error).context("could not run gcloud")
    }
}

/// gcloud's message: from its `ERROR:` line on, else its last line; what to do when it is signed
/// out.
fn gcloud_error(stderr: &str) -> String {
    if signed_out(stderr) {
        return SIGNED_OUT.into();
    }
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let from = lines
        .iter()
        .position(|l| l.starts_with("ERROR:"))
        .unwrap_or(lines.len().saturating_sub(1));
    let message = lines[from.min(lines.len())..].join(" ");
    if message.is_empty() {
        "no output".into()
    } else {
        message
    }
}

/// Runs a command; its error is what it printed last.
pub(crate) async fn run(mut command: Command, what: &str) -> anyhow::Result<String> {
    let output = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(not_installed)?;
    if !output.status.success() {
        bail!(
            "{what} failed: {}",
            gcloud_error(&String::from_utf8_lossy(&output.stderr))
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Held throughout a recovery, excluding provisioning, deletion and another recovery of the VM.
pub(crate) struct RecoveryWorker(Arc<Daemon>, String);

impl Drop for RecoveryWorker {
    fn drop(&mut self) {
        self.0.workers.busy.lock().unwrap().remove(&self.1);
    }
}

pub(crate) fn recovery_worker(daemon: &Arc<Daemon>, name: &str) -> Result<RecoveryWorker> {
    if !daemon.workers.busy.lock().unwrap().insert(name.to_owned()) {
        return Err(Error::Conflict(format!(
            "{name} is being changed; wait before recovering it"
        )));
    }
    Ok(RecoveryWorker(daemon.clone(), name.to_owned()))
}

fn recovery_marker(name: &str) -> String {
    format!("worker-recovery:{name}")
}

fn refuse_recovery_worker(daemon: &Daemon, name: &str) -> Result<()> {
    if daemon.store.kv_get(&recovery_marker(name))?.is_some() {
        return Err(Error::Conflict(format!(
            "{name} is retained for backup recovery: finish recovering its workspaces and inspect its kept disk before removing it manually"
        )));
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct RecoveryVm {
    instance_id: String,
    disk: String,
}

fn recovery_vm(instance: &serde_json::Value) -> anyhow::Result<RecoveryVm> {
    let instance_id = instance["id"]
        .as_str()
        .context("the worker has no instance id")?
        .to_owned();
    let disks = instance["disks"]
        .as_array()
        .context("the worker has no disks")?;
    let disk = disks
        .iter()
        .find(|d| d["boot"].as_bool() == Some(true))
        .and_then(|d| d["source"].as_str())
        .context("the worker has no boot disk")?
        .to_owned();
    Ok(RecoveryVm { instance_id, disk })
}

/// Deletes this labelled VM with every disk kept, then verifies through GCP that the VM is gone
/// and its boot disk is unattached. Its identity is recorded first so a retry never deletes a
/// replacement VM of the same name. Missing credentials, ambiguous absence and attached disks fail.
pub(crate) async fn fence_for_recovery(held: &RecoveryWorker) -> anyhow::Result<String> {
    let daemon = &held.0;
    let name = &held.1;
    let record = load(daemon, name)?.context("the worker no longer exists in Chauffeur")?;
    let marker = recovery_marker(name);
    let previous = daemon.store.kv_get(&marker)?;
    let described = compute(
        daemon,
        &record,
        &["instances", "describe", &record.instance, "--format=json"],
    )
    .output()
    .await?;
    let instance = if described.status.success() {
        let value: serde_json::Value = serde_json::from_slice(&described.stdout)?;
        let labels: BTreeMap<String, String> = serde_json::from_value(value["labels"].clone())?;
        check_label(&record, &labels)?;
        Some(value)
    } else {
        let error = String::from_utf8_lossy(&described.stderr);
        anyhow::ensure!(
            error.contains("was not found") || error.contains("HTTPError 404"),
            "could not verify {}: {}",
            record.instance,
            gcloud_error(&error)
        );
        None
    };
    let proof: RecoveryVm = match previous {
        Some(raw) => serde_json::from_str(&raw)?,
        None => {
            let proof = recovery_vm(instance.as_ref().context(
                "the old VM is already missing and its boot disk was never recorded; inspect GCP before recovering"
            )?)?;
            daemon
                .store
                .kv_set(&marker, &serde_json::to_string(&proof)?)?;
            proof
        }
    };
    daemon.workers.stop_forward(name);
    change(daemon, name, |r| {
        r.state = WorkerState::Failed;
        r.ssh = None;
        r.error = Some(format!(
            "Backup recovery: VM removed or being removed; kept disk {} still costs storage",
            proof.disk
        ));
    });
    write_ssh_config(daemon)?;
    if let Some(instance) = instance {
        anyhow::ensure!(
            instance["id"].as_str() == Some(&proof.instance_id),
            "{} now names another VM; refuse to delete it",
            record.instance
        );
        run(
            compute(
                daemon,
                &record,
                &["instances", "delete", &record.instance, "--keep-disks=all"],
            ),
            "gcloud compute instances delete (keep disks)",
        )
        .await?;
    }
    // A successful delete command alone is not proof of absence.
    let check = compute(
        daemon,
        &record,
        &["instances", "describe", &record.instance, "--format=json"],
    )
    .output()
    .await?;
    let error = String::from_utf8_lossy(&check.stderr);
    anyhow::ensure!(
        !check.status.success()
            && (error.contains("was not found") || error.contains("HTTPError 404")),
        "{} is not confirmed deleted: {}",
        record.instance,
        gcloud_error(&error)
    );
    let disk = proof
        .disk
        .rsplit('/')
        .next()
        .context("invalid retained disk")?;
    let disk = run(
        compute(
            daemon,
            &record,
            &["disks", "describe", disk, "--format=json"],
        ),
        "gcloud compute disks describe",
    )
    .await?;
    let disk: serde_json::Value = serde_json::from_str(&disk)?;
    anyhow::ensure!(
        disk["selfLink"].as_str() == Some(&proof.disk),
        "GCP described another disk"
    );
    anyhow::ensure!(
        disk.get("users")
            .is_none_or(|u| u.as_array().is_some_and(Vec::is_empty)),
        "the retained boot disk is attached to a VM; stop it before recovering"
    );
    Ok(proof.disk)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;

    /// A daemon whose `gcloud` is a fake that records its calls, keeps instances as files and
    /// answers SSH without a VM.
    fn fixture() -> (tempfile::TempDir, Arc<Daemon>, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::new(
            temp.path().join("home"),
            temp.path().join("workspaces"),
            temp.path().join("repos"),
        );
        paths.ensure().unwrap();
        let fake = temp.path().join("fake");
        std::fs::create_dir_all(fake.join("instances")).unwrap();
        std::fs::write(fake.join("bundle.tar.gz"), "bundle").unwrap();
        std::fs::write(
            &paths.settings,
            format!(
                "linear_enabled = false\ngithub_org = \"\"\ngithub_sources = false\n[transfer]\ncontroller = \"/tmp/c.sock\"\n[[transfer.machines]]\nname = \"vm\"\nsocket = \"/tmp/vm.sock\"\n[workers]\nproject = \"test-project\"\nzone = \"europe-west9-c\"\nbundle = \"{}\"\n",
                fake.join("bundle.tar.gz").display()
            ),
        )
        .unwrap();
        let gcloud = fake.join("gcloud");
        std::fs::write(
            &gcloud,
            format!(
                r#"#!/bin/bash
F={}
echo "$*" >> "$F/calls"
# Its session expired: every command but `auth list` refuses, as the real one does.
if [ -e "$F/signed-out" ] && [ "$1" != auth ]; then
  echo "ERROR: (gcloud.$1.$2) There was a problem refreshing your current auth tokens: Reauthentication failed. cannot prompt during non-interactive execution." >&2
  echo "Please run:" >&2
  echo "  \$ gcloud auth login" >&2
  exit 1
fi
case "$1 $2 $3" in
  "auth list "*) echo me@example.com ;;
  "compute instances describe")
    [ -f "$F/instances/$4" ] && cat "$F/instances/$4" && exit 0
    echo "ERROR: (gcloud.compute.instances.describe) Could not fetch resource:" >&2
    echo " - The resource 'projects/p/zones/z/instances/$4' was not found" >&2
    exit 1 ;;
  "compute instances create")
    # Compute does not know an account just made, the first time.
    if [ -e "$F/account-not-visible" ]; then
      rm "$F/account-not-visible"
      echo "ERROR: (gcloud.compute.instances.create) Could not fetch resource:" >&2
      echo " - The resource 'cw-backup-w1@test-project.iam.gserviceaccount.com' of type 'serviceAccount' was not found." >&2
      exit 1
    fi
    label=$(echo "$*" | sed -n 's/.*--labels chauffeur-worker=\([^ ]*\).*/\1/p')
    echo "{{\"labels\":{{\"chauffeur-worker\":\"$label\"}}}}" > "$F/instances/$4" ;;
  "compute instances delete") rm "$F/instances/$4" ;;
  "compute instances stop") sed -i.bak 's/^{{/{{"status":"TERMINATED",/' "$F/instances/$4" ;;
  "compute instances start") sed -i.bak 's/"status":"TERMINATED",//' "$F/instances/$4" ;;
  "storage buckets describe")
    [ -f "$F/buckets/${{4#gs://}}" ] && cat "$F/buckets/${{4#gs://}}" && exit 0
    echo "ERROR: (gcloud.storage.buckets.describe) $4 not found: 404." >&2
    exit 1 ;;
  "storage buckets create")
    mkdir -p "$F/buckets"
    echo '{{"location":"EUROPE-WEST9","iamConfiguration":{{"uniformBucketLevelAccess":{{"enabled":true}}}},"projectNumber":"123"}}' > "$F/buckets/${{4#gs://}}"
    # Another creation made it first.
    [ -e "$F/bucket-race" ] && {{ echo "ERROR: (gcloud.storage.buckets.create) HTTPError 409: already exists" >&2; exit 1; }}
    true ;;
  "storage managed-folders create") mkdir -p "$F/folders" && touch "$F/folders/$(echo "$4" | tr / _)" ;;
  "storage managed-folders add-iam-policy-binding")
    [ -e "$F/binding-denied" ] && {{ echo "ERROR: (gcloud.storage.managed-folders.add-iam-policy-binding) HTTPError 403: PERMISSION_DENIED on $4" >&2; exit 1; }}
    # IAM does not know the account just made, the first time.
    if [ -e "$F/member-unknown" ]; then
      rm "$F/member-unknown"
      echo "ERROR: (gcloud.storage.managed-folders.add-iam-policy-binding) HTTPError 400: Service account $6 does not exist." >&2
      exit 1
    fi
    true ;;
  "storage managed-folders delete") rm "$F/folders/$(echo "$4" | tr / _)" ;;
  "storage managed-folders remove-iam-policy-binding"|"storage buckets update") ;;
  "projects describe "*) echo 123 ;;
  "iam service-accounts describe")
    [ -f "$F/accounts/$4" ] && cat "$F/accounts/$4" && exit 0
    [ -e "$F/accounts-deleted-lately" ] && {{ echo "ERROR: (gcloud.iam.service-accounts.describe) PERMISSION_DENIED: Permission 'iam.serviceAccounts.get' denied on resource (or it may not exist)." >&2; exit 1; }}
    echo "ERROR: (gcloud.iam.service-accounts.describe) NOT_FOUND: Unknown service account" >&2
    exit 1 ;;
  "iam service-accounts create")
    [ -e "$F/account-create-denied" ] && {{ echo "ERROR: (gcloud.iam.service-accounts.create) PERMISSION_DENIED: Permission 'iam.serviceAccounts.create' denied on resource (or it may not exist)." >&2; exit 1; }}
    mkdir -p "$F/accounts"
    echo '{{"displayName":"Chauffeur worker"}}' > "$F/accounts/$4@test-project.iam.gserviceaccount.com" ;;
  "iam service-accounts delete")
    [ -e "$F/account-delete-denied" ] && {{ echo "ERROR: (gcloud.iam.service-accounts.delete) PERMISSION_DENIED: Permission 'iam.serviceAccounts.delete' denied on resource (or it may not exist)." >&2; exit 1; }}
    [ -f "$F/accounts/$4" ] || {{ echo "ERROR: (gcloud.iam.service-accounts.delete) NOT_FOUND: Unknown service account" >&2; exit 1; }}
    rm "$F/accounts/$4" ;;
  "compute scp "*) ;;
  "compute ssh "*)
    case "$*" in
      *"--command true"*) ;;
      *"--dry-run"*) echo "/usr/bin/ssh -t -i /keys/google_compute_engine -o HostKeyAlias=compute.$3 -o StrictHostKeyChecking=yes me@10.0.0.$((${{#3}} % 250))" ;;
      # A forward: its local socket, then an ssh that takes a moment to stop.
      *" -N "*)
        last="${{@: -1}}"
        touch "${{last%%:*}}"
        trap 'sleep 0.5; exit 0' TERM
        sleep 60 & wait ;;
      *) cat > /dev/null; echo "E: Unable to locate package" >&2; exit 100 ;;
    esac ;;
  *) exit 1 ;;
esac
"#,
                fake.display()
            ),
        )
        .unwrap();
        // The forward of a worker with an SSH entry: its local socket, unless the key is refused.
        let ssh = fake.join("ssh");
        std::fs::write(
            &ssh,
            format!(
                r#"#!/bin/bash
F={}
echo "$*" >> "$F/ssh-calls"
if [ -e "$F/key-refused" ]; then
  echo "${{@: -1}}: Permission denied (publickey)." >&2
  exit 255
fi
while [ $# -gt 0 ]; do
  [ "$1" = -L ] && touch "${{2%%:*}}"
  shift
done
trap 'exit 0' TERM
sleep 60 & wait
"#,
                fake.display()
            ),
        )
        .unwrap();
        for program in [&gcloud, &ssh] {
            std::fs::set_permissions(program, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .unwrap();
        }
        let daemon = Daemon::open(paths).unwrap();
        *daemon.workers.gcloud.write().unwrap() = gcloud;
        *daemon.workers.ssh.write().unwrap() = ssh;
        (temp, daemon, fake)
    }

    fn calls(fake: &Path) -> String {
        std::fs::read_to_string(fake.join("calls")).unwrap_or_default()
    }

    async fn settled(daemon: &Daemon, name: &str) -> Record {
        for _ in 0..100 {
            let record = load(daemon, name).unwrap().unwrap();
            if !daemon.workers.busy.lock().unwrap().contains(name) {
                return record;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("{name} did not settle");
    }

    fn params(name: &str) -> WorkerCreateParams {
        WorkerCreateParams {
            name: name.into(),
            machine_type: None,
            beyond_limit: false,
        }
    }

    #[tokio::test]
    async fn a_worker_is_created_labelled_in_its_project_as_its_own_backup_account_without_project_keys()
     {
        let (_temp, daemon, fake) = fixture();
        let started = create(&daemon, params("w1")).await.unwrap();
        assert_eq!(started.state, WorkerState::Creating);
        assert_eq!(started.instance, "chauffeur-worker-w1");
        assert_eq!(started.hourly_usd, 0.1684);
        // The fake's installation fails: the worker stops there, failed, its VM created.
        let record = settled(&daemon, "w1").await;
        assert_eq!(record.state, WorkerState::Failed);
        assert_eq!(
            record.error.as_deref(),
            Some("the installation on the worker failed: E: Unable to locate package")
        );
        let calls = calls(&fake);
        let create = calls
            .lines()
            .find(|l| l.starts_with("compute instances create"))
            .unwrap();
        for arg in [
            "chauffeur-worker-w1",
            "--machine-type e2-standard-4",
            "--labels chauffeur-worker=w1",
            "--metadata block-project-ssh-keys=true",
            "--service-account cw-backup-w1@test-project.iam.gserviceaccount.com",
            "--scopes https://www.googleapis.com/auth/devstorage.read_write",
            "--project test-project",
            "--zone europe-west9-c",
            "--quiet",
        ] {
            assert!(create.contains(arg), "{arg} missing from {create}");
        }
        assert!(calls.contains("compute scp "), "{calls}");
        assert!(calls.contains("bundle.tar.gz chauffeur-worker-w1:chauffeur-linux-x64.tar.gz"));
        // Before the VM: the bucket in Paris keeping deleted objects a week and marked Chauffeur's,
        // the worker's account, and its access to its own prefix only.
        let before_vm = &calls[..calls.find("compute instances create").unwrap()];
        let prefix = record.backup_prefix.clone().unwrap();
        assert!(prefix.starts_with("w1-") && prefix.len() == 11, "{prefix}");
        for call in [
            "storage buckets create gs://test-project-chauffeur-backups --project test-project --location europe-west9 --default-storage-class STANDARD --uniform-bucket-level-access --public-access-prevention --soft-delete-duration 7d".to_owned(),
            "storage buckets update gs://test-project-chauffeur-backups --update-labels chauffeur-backups=true".to_owned(),
            "iam service-accounts create cw-backup-w1 --project test-project --display-name Chauffeur worker w1".to_owned(),
            format!("storage managed-folders create gs://test-project-chauffeur-backups/{prefix}/"),
            format!("storage managed-folders add-iam-policy-binding gs://test-project-chauffeur-backups/{prefix}/ --member serviceAccount:cw-backup-w1@test-project.iam.gserviceaccount.com --role roles/storage.objectUser"),
        ] {
            assert!(before_vm.contains(&call), "{call} missing from {before_vm}");
        }
        // Read as the storage API gives it: gcloud's own format has no project.
        assert!(calls.contains(
            "storage buckets describe gs://test-project-chauffeur-backups --raw --format=json"
        ));
        assert_eq!(
            backup_place(&daemon, "w1").unwrap(),
            ("test-project-chauffeur-backups".to_owned(), prefix)
        );
        assert!(
            calls
                .lines()
                .filter(|l| l.starts_with("compute"))
                .all(|l| l.contains("--project test-project")),
            "{calls}"
        );
    }

    #[tokio::test]
    async fn creating_refuses_a_bad_name_an_unknown_type_a_declared_machine_and_a_third_worker() {
        let (_temp, daemon, fake) = fixture();
        for name in ["W1", "1w", "w_1", "w-", "a-name-longer-than-twenty"] {
            assert!(matches!(
                create(&daemon, params(name)).await,
                Err(Error::BadRequest(_))
            ));
        }
        let mut big = params("w1");
        big.machine_type = Some("n2-highmem-96".into());
        assert!(matches!(
            create(&daemon, big).await,
            Err(Error::BadRequest(_))
        ));
        assert!(matches!(
            create(&daemon, params("vm")).await,
            Err(Error::BadRequest(_))
        ));
        daemon
            .settings
            .update(|doc| doc["workers"]["zone"] = toml_edit::value("us-central1-a"))
            .unwrap();
        let Err(Error::BadRequest(message)) = create(&daemon, params("w1")).await else {
            panic!("a worker was created outside the region of the prices");
        };
        assert!(message.contains("europe-west9"), "{message}");
        daemon
            .settings
            .update(|doc| doc["workers"]["zone"] = toml_edit::value("europe-west9-b"))
            .unwrap();
        assert!(!calls(&fake).contains("instances create"));
        create(&daemon, params("w1")).await.unwrap();
        settled(&daemon, "w1").await;
        create(&daemon, params("w2")).await.unwrap();
        settled(&daemon, "w2").await;
        let Err(Error::Conflict(message)) = create(&daemon, params("w3")).await else {
            panic!("a third worker was created without confirmation");
        };
        assert!(message.contains("2 workers already exist"), "{message}");
        let mut confirmed = params("w3");
        confirmed.beyond_limit = true;
        create(&daemon, confirmed).await.unwrap();
        settled(&daemon, "w3").await;
    }

    #[tokio::test]
    async fn a_failed_worker_is_created_again_on_its_existing_vm() {
        let (_temp, daemon, fake) = fixture();
        create(&daemon, params("w1")).await.unwrap();
        settled(&daemon, "w1").await;
        create(&daemon, params("w1")).await.unwrap();
        let record = settled(&daemon, "w1").await;
        assert_eq!(calls(&fake).matches("instances create").count(), 1);
        assert!(
            record
                .log
                .iter()
                .any(|l| l == "The VM chauffeur-worker-w1 already exists")
        );
    }

    #[tokio::test]
    async fn deleting_removes_the_vm_and_forgets_the_worker() {
        let (_temp, daemon, fake) = fixture();
        create(&daemon, params("w1")).await.unwrap();
        settled(&daemon, "w1").await;
        let deleting = delete(&daemon, WorkerParams { name: "w1".into() }).unwrap();
        assert_eq!(deleting.state, WorkerState::Deleting);
        for _ in 0..100 {
            if load(&daemon, "w1").unwrap().is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(load(&daemon, "w1").unwrap().is_none());
        assert!(!fake.join("instances/chauffeur-worker-w1").exists());
        assert!(calls(&fake).contains(
            "compute instances delete chauffeur-worker-w1 --delete-disks=all --project test-project --zone europe-west9-c --quiet"
        ));
        // Its access goes with it, then its empty folder; the bucket stays.
        let calls = calls(&fake);
        let unbound = calls
            .find("storage managed-folders remove-iam-policy-binding gs://test-project-chauffeur-backups/w1-")
            .expect("its folder's binding removed");
        assert!(calls[unbound..].contains("--member serviceAccount:cw-backup-w1@test-project.iam.gserviceaccount.com --role roles/storage.objectUser"));
        assert!(
            calls[unbound..]
                .contains("storage managed-folders delete gs://test-project-chauffeur-backups/w1-")
        );
        assert!(
            std::fs::read_dir(fake.join("folders"))
                .unwrap()
                .next()
                .is_none(),
            "its folder is gone"
        );
        assert!(
            !fake
                .join("accounts/cw-backup-w1@test-project.iam.gserviceaccount.com")
                .exists()
        );
        assert!(fake.join("buckets/test-project-chauffeur-backups").exists());
    }

    #[tokio::test]
    async fn a_bucket_another_creation_made_first_is_used_and_one_keeping_deleted_objects_is_said()
    {
        let (_temp, daemon, fake) = fixture();
        std::fs::write(fake.join("bucket-race"), "").unwrap();
        create(&daemon, params("w1")).await.unwrap();
        let record = settled(&daemon, "w1").await;
        // It went on to the VM; the fake's installation is what fails.
        assert!(record.error.unwrap().contains("Unable to locate package"));
        assert!(calls(&fake).contains("compute instances create"));
        let prefix = record.backup_prefix.clone();
        // A bucket someone else made without soft delete is used as it is, and said.
        std::fs::write(
            fake.join("buckets/test-project-chauffeur-backups"),
            r#"{"location":"EUROPE-WEST9","iamConfiguration":{"uniformBucketLevelAccess":{"enabled":true}},"projectNumber":"123"}"#,
        )
        .unwrap();
        create(&daemon, params("w1")).await.unwrap();
        let record = settled(&daemon, "w1").await;
        assert!(
            record
                .log
                .iter()
                .any(|l| l.contains("keeps no deleted objects")),
            "{:?}",
            record.log
        );
        assert!(
            !calls(&fake).contains(
                "buckets update gs://test-project-chauffeur-backups --soft-delete-duration"
            ),
            "a bucket Chauffeur did not make is not changed"
        );
        assert_eq!(
            record.backup_prefix, prefix,
            "made again, it keeps its prefix"
        );
        // One Chauffeur made is given its week back.
        std::fs::write(
            fake.join("buckets/test-project-chauffeur-backups"),
            r#"{"location":"EUROPE-WEST9","iamConfiguration":{"uniformBucketLevelAccess":{"enabled":true}},"projectNumber":"123","labels":{"chauffeur-backups":"true"},"softDeletePolicy":{"retentionDurationSeconds":"0"}}"#,
        )
        .unwrap();
        create(&daemon, params("w1")).await.unwrap();
        settled(&daemon, "w1").await;
        assert!(calls(&fake).contains(
            "storage buckets update gs://test-project-chauffeur-backups --soft-delete-duration 7d"
        ));
    }

    #[tokio::test]
    async fn the_vm_waits_for_compute_to_know_the_account_just_made() {
        let (_temp, daemon, fake) = fixture();
        std::fs::write(fake.join("account-not-visible"), "").unwrap();
        create(&daemon, params("w1")).await.unwrap();
        let record = settled(&daemon, "w1").await;
        assert!(
            record.error.unwrap().contains("Unable to locate package"),
            "went on to the VM"
        );
        assert_eq!(calls(&fake).matches("compute instances create").count(), 2);
        assert!(fake.join("instances/chauffeur-worker-w1").exists());
    }

    #[tokio::test]
    async fn an_account_deleted_lately_is_made_again_and_deleting_twice_is_fine() {
        let (_temp, daemon, fake) = fixture();
        // IAM answers "denied, or it may not exist" for an account deleted lately.
        std::fs::write(fake.join("accounts-deleted-lately"), "").unwrap();
        create(&daemon, params("w1")).await.unwrap();
        let record = settled(&daemon, "w1").await;
        assert!(
            record.error.unwrap().contains("Unable to locate package"),
            "went on to the VM"
        );
        assert!(calls(&fake).contains("iam service-accounts create cw-backup-w1"));
        // Its account already gone, deleting the worker still ends.
        std::fs::remove_file(
            fake.join("accounts/cw-backup-w1@test-project.iam.gserviceaccount.com"),
        )
        .unwrap();
        delete(&daemon, WorkerParams { name: "w1".into() }).unwrap();
        for _ in 0..100 {
            if load(&daemon, "w1").unwrap().is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            load(&daemon, "w1").unwrap().is_none(),
            "{:?}",
            load(&daemon, "w1")
        );
    }

    #[tokio::test]
    async fn a_bucket_of_another_project_and_a_refused_binding_stop_at_once() {
        let (_temp, daemon, fake) = fixture();
        std::fs::create_dir_all(fake.join("buckets")).unwrap();
        std::fs::write(
            fake.join("buckets/test-project-chauffeur-backups"),
            r#"{"location":"EUROPE-WEST9","iamConfiguration":{"uniformBucketLevelAccess":{"enabled":true}},"projectNumber":"999"}"#,
        )
        .unwrap();
        create(&daemon, params("w1")).await.unwrap();
        let record = settled(&daemon, "w1").await;
        assert!(
            record
                .error
                .unwrap()
                .contains("is not a bucket of test-project")
        );
        assert!(!calls(&fake).contains("add-iam-policy-binding"));
        assert!(!calls(&fake).contains("managed-folders create"));
        std::fs::remove_file(fake.join("buckets/test-project-chauffeur-backups")).unwrap();
        std::fs::write(fake.join("binding-denied"), "").unwrap();
        let started = tokio::time::Instant::now();
        create(&daemon, params("w1")).await.unwrap();
        let record = settled(&daemon, "w1").await;
        assert!(record.error.unwrap().contains("PERMISSION_DENIED"));
        assert_eq!(
            calls(&fake).matches("add-iam-policy-binding").count(),
            1,
            "not retried"
        );
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    async fn deleted(daemon: &Arc<Daemon>, name: &str) -> Option<Record> {
        delete(daemon, WorkerParams { name: name.into() }).unwrap();
        for _ in 0..100 {
            match load(daemon, name).unwrap() {
                Some(record) if record.state == WorkerState::Failed => return Some(record),
                None => return None,
                Some(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
        panic!("{name} did not settle");
    }

    #[tokio::test]
    async fn an_account_is_recorded_only_once_made_and_gone_only_once_iam_says_not_found() {
        let (_temp, daemon, fake) = fixture();
        // Not allowed to make it: nothing recorded, so deleting the worker never deletes it.
        std::fs::write(fake.join("account-create-denied"), "").unwrap();
        create(&daemon, params("w1")).await.unwrap();
        let record = settled(&daemon, "w1").await;
        assert!(record.error.unwrap().contains("PERMISSION_DENIED"));
        assert_eq!(record.backup_account, None);
        assert_eq!(deleted(&daemon, "w1").await.map(|r| r.error), None);
        assert!(!calls(&fake).contains("service-accounts delete"));
        std::fs::remove_file(fake.join("account-create-denied")).unwrap();
        // Made, then not allowed to delete it ("denied, or it may not exist"): the worker is not
        // said deleted while its account stays.
        create(&daemon, params("w1")).await.unwrap();
        settled(&daemon, "w1").await;
        let account = fake.join("accounts/cw-backup-w1@test-project.iam.gserviceaccount.com");
        assert!(account.exists());
        std::fs::write(fake.join("account-delete-denied"), "").unwrap();
        let failed = deleted(&daemon, "w1").await.expect("kept, failed");
        assert!(failed.error.unwrap().contains("PERMISSION_DENIED"));
        assert!(account.exists());
        std::fs::remove_file(fake.join("account-delete-denied")).unwrap();
        assert!(deleted(&daemon, "w1").await.is_none());
        assert!(!account.exists());
    }

    #[tokio::test]
    async fn another_attempt_keeps_the_bucket_the_first_one_was_given_access_to() {
        let (_temp, daemon, fake) = fixture();
        std::fs::write(fake.join("binding-denied"), "").unwrap();
        create(&daemon, params("w1")).await.unwrap();
        let first = settled(&daemon, "w1").await;
        assert_eq!(
            first.backup_bucket.as_deref(),
            Some("test-project-chauffeur-backups")
        );
        let settings = std::fs::read_to_string(&daemon.paths.settings).unwrap();
        std::fs::write(
            &daemon.paths.settings,
            settings.replace("[workers]\n", "[workers]\nbackup_bucket = \"elsewhere\"\n"),
        )
        .unwrap();
        for _ in 0..60 {
            if daemon.settings.get().workers.backup_bucket == "elsewhere" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(daemon.settings.get().workers.backup_bucket, "elsewhere");
        std::fs::remove_file(fake.join("binding-denied")).unwrap();
        create(&daemon, params("w1")).await.unwrap();
        let again = settled(&daemon, "w1").await;
        assert_eq!(again.backup_bucket, first.backup_bucket);
        assert!(!calls(&fake).contains("gs://elsewhere"), "{}", calls(&fake));
    }

    #[tokio::test]
    async fn the_binding_waits_for_iam_to_know_the_account_and_nothing_else() {
        let (_temp, daemon, fake) = fixture();
        std::fs::write(fake.join("member-unknown"), "").unwrap();
        create(&daemon, params("w1")).await.unwrap();
        let record = settled(&daemon, "w1").await;
        assert!(record.error.unwrap().contains("Unable to locate package"));
        assert_eq!(calls(&fake).matches("add-iam-policy-binding").count(), 2);
        let account = "cw-backup-w1@p.iam.gserviceaccount.com";
        assert!(unknown_yet(
            "gcloud storage managed-folders add-iam-policy-binding failed: HTTPError 400: Service account cw-backup-w1@p.iam.gserviceaccount.com does not exist.",
            account
        ));
        assert!(unknown_yet("HTTPError 412: Precondition Failed", account));
        // A number in a prefix or a project is no HTTP code; a missing folder is final.
        assert!(!unknown_yet(
            "HTTPError 403: PERMISSION_DENIED on gs://b/w1-41203aa1/",
            account
        ));
        assert!(!unknown_yet(
            "HTTPError 404: The specified managed folder does not exist.",
            account
        ));
    }

    #[tokio::test]
    async fn a_worker_made_before_backups_has_no_place_to_back_up_to() {
        let (_temp, daemon, _fake) = fixture();
        let record = Record {
            name: "old".into(),
            instance: "chauffeur-worker-old".into(),
            project: "test-project".into(),
            zone: "europe-west9-c".into(),
            machine_type: DEFAULT_MACHINE_TYPE.into(),
            state: WorkerState::Ready,
            log: Vec::new(),
            error: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            remote_socket: None,
            backup_bucket: None,
            backup_account: None,
            backup_prefix: None,
            ssh: None,
            ssh_error: None,
        };
        save(&daemon, &record).unwrap();
        let refused = backup_place(&daemon, "old").unwrap_err().to_string();
        assert!(
            refused.contains("created before backups: delete it and create it again"),
            "{refused}"
        );
    }

    #[tokio::test]
    async fn a_bucket_outside_paris_or_an_account_chauffeur_did_not_make_stops_before_any_vm() {
        let (_temp, daemon, fake) = fixture();
        std::fs::create_dir_all(fake.join("buckets")).unwrap();
        let bucket = fake.join("buckets/test-project-chauffeur-backups");
        std::fs::write(
            &bucket,
            r#"{"location":"US","iamConfiguration":{"uniformBucketLevelAccess":{"enabled":true}},"projectNumber":"123"}"#,
        )
        .unwrap();
        create(&daemon, params("w1")).await.unwrap();
        let record = settled(&daemon, "w1").await;
        assert!(record.error.unwrap().contains("is in US, not in Paris"));
        std::fs::write(
            &bucket,
            r#"{"location":"EUROPE-WEST9","iamConfiguration":{"uniformBucketLevelAccess":{"enabled":true}},"projectNumber":"123"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(fake.join("accounts")).unwrap();
        std::fs::write(
            fake.join("accounts/cw-backup-w1@test-project.iam.gserviceaccount.com"),
            r#"{"displayName":"Someone's robot"}"#,
        )
        .unwrap();
        create(&daemon, params("w1")).await.unwrap();
        let record = settled(&daemon, "w1").await;
        assert!(
            record
                .error
                .unwrap()
                .contains("Chauffeur did not make it for this worker")
        );
        // Named like Chauffeur's, but not made by it for this worker: refused all the same.
        std::fs::write(
            fake.join("accounts/cw-backup-w1@test-project.iam.gserviceaccount.com"),
            r#"{"displayName":"Chauffeur worker w1"}"#,
        )
        .unwrap();
        create(&daemon, params("w1")).await.unwrap();
        let record = settled(&daemon, "w1").await;
        assert!(
            record
                .error
                .unwrap()
                .contains("Chauffeur did not make it for this worker")
        );
        let calls = calls(&fake);
        assert!(!calls.contains("instances create"), "{calls}");
        assert!(!calls.contains("buckets create"), "{calls}");
        assert!(!calls.contains("add-iam-policy-binding"), "{calls}");
        assert!(!calls.contains("managed-folders"), "{calls}");
    }

    #[tokio::test]
    async fn an_instance_without_the_worker_label_is_never_deleted() {
        let (_temp, daemon, fake) = fixture();
        create(&daemon, params("w1")).await.unwrap();
        settled(&daemon, "w1").await;
        std::fs::write(
            fake.join("instances/chauffeur-worker-w1"),
            r#"{"labels":{"purpose":"relay"}}"#,
        )
        .unwrap();
        delete(&daemon, WorkerParams { name: "w1".into() }).unwrap();
        let record = settled(&daemon, "w1").await;
        assert_eq!(record.state, WorkerState::Failed);
        assert!(
            record
                .error
                .unwrap()
                .contains("does not carry the label chauffeur-worker=w1")
        );
        assert!(!calls(&fake).contains("instances delete"));
        assert!(fake.join("instances/chauffeur-worker-w1").exists());
    }

    #[tokio::test]
    async fn a_worker_left_creating_by_a_stopped_daemon_reads_failed_at_the_next_start() {
        let (_temp, daemon, _fake) = fixture();
        let record = Record {
            name: "w1".into(),
            instance: "chauffeur-worker-w1".into(),
            project: "test-project".into(),
            zone: "europe-west9-c".into(),
            machine_type: DEFAULT_MACHINE_TYPE.into(),
            state: WorkerState::Installing,
            log: Vec::new(),
            error: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            remote_socket: None,
            ssh: None,
            ssh_error: None,
            backup_bucket: None,
            backup_account: None,
            backup_prefix: None,
        };
        save(&daemon, &record).unwrap();
        resume(&daemon);
        let record = load(&daemon, "w1").unwrap().unwrap();
        assert_eq!(record.state, WorkerState::Failed);
        assert!(record.error.unwrap().contains("stopped while creating"));
    }

    #[tokio::test]
    async fn a_ready_worker_is_a_transfer_machine_after_the_declared_ones() {
        let (_temp, daemon, _fake) = fixture();
        for (name, state) in [("w1", WorkerState::Ready), ("w2", WorkerState::Installing)] {
            let record = Record {
                name: name.into(),
                instance: format!("{INSTANCE_PREFIX}{name}"),
                project: "test-project".into(),
                zone: "europe-west9-c".into(),
                machine_type: DEFAULT_MACHINE_TYPE.into(),
                state,
                log: Vec::new(),
                error: None,
                created_at: chrono::Utc::now().to_rfc3339(),
                remote_socket: None,
                ssh: None,
                ssh_error: None,
                backup_bucket: None,
                backup_account: None,
                backup_prefix: None,
            };
            save(&daemon, &record).unwrap();
        }
        assert_eq!(
            crate::cloud_transfer_driver::machines(&daemon).machines,
            ["vm", "w1"]
        );
        assert_eq!(
            machine_socket(&daemon, "w1").unwrap(),
            socket_path(&daemon, "w1")
        );
        let refused = machine_socket(&daemon, "w2").unwrap_err().to_string();
        assert!(refused.contains("The worker w2 is not ready"), "{refused}");
    }

    #[tokio::test]
    async fn a_replaced_forward_keeps_its_socket_and_a_stopped_one_removes_it() {
        let (_temp, daemon, _fake) = fixture();
        let record = Record {
            name: "w1".into(),
            instance: "chauffeur-worker-w1".into(),
            project: "test-project".into(),
            zone: "europe-west9-c".into(),
            machine_type: DEFAULT_MACHINE_TYPE.into(),
            state: WorkerState::Ready,
            log: Vec::new(),
            error: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            remote_socket: Some("/remote.sock".into()),
            ssh: None,
            ssh_error: None,
            backup_bucket: None,
            backup_account: None,
            backup_prefix: None,
        };
        let local = socket_path(&daemon, "w1");
        let exists = |wanted: bool| {
            let local = local.clone();
            async move {
                for _ in 0..100 {
                    if local.exists() == wanted {
                        return true;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                false
            }
        };
        start_forward(&daemon, "w1", "/remote.sock", &record);
        assert!(exists(true).await, "the first forward made no socket");
        // A retried creation replaces it: the old one, slow to stop, must not take the new socket.
        start_forward(&daemon, "w1", "/remote.sock", &record);
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(
            local.exists(),
            "the replaced forward removed its successor's socket"
        );
        daemon.workers.stop_forward("w1");
        assert!(exists(false).await, "a stopped forward left its socket");
    }

    #[tokio::test]
    async fn a_ready_worker_gets_an_ssh_entry_the_include_once_and_loses_the_entry_when_deleted() {
        let (temp, daemon, fake) = fixture();
        let ssh_dir = temp.path().join("user/.ssh");
        *daemon.workers.ssh_dir.write().unwrap() = Some(ssh_dir.clone());
        let ready = |name: &str| {
            let record = Record {
                name: name.into(),
                instance: format!("{INSTANCE_PREFIX}{name}"),
                project: "test-project".into(),
                zone: "europe-west9-c".into(),
                machine_type: DEFAULT_MACHINE_TYPE.into(),
                state: WorkerState::Ready,
                log: Vec::new(),
                error: None,
                created_at: chrono::Utc::now().to_rfc3339(),
                remote_socket: None,
                ssh: None,
                ssh_error: None,
                backup_bucket: None,
                backup_account: None,
                backup_prefix: None,
            };
            save(&daemon, &record).unwrap();
            std::fs::write(
                fake.join("instances").join(&record.instance),
                format!(r#"{{"labels":{{"chauffeur-worker":"{name}"}}}}"#),
            )
            .unwrap();
            record
        };
        let first = add_ssh_entry(&daemon, &ready("w1")).await.unwrap();
        assert_eq!(first.len(), 2, "{first:?}");
        assert!(first[1].starts_with("Added `Include "), "{first:?}");
        let second = add_ssh_entry(&daemon, &ready("w2")).await.unwrap();
        assert_eq!(second.len(), 1, "the include was added twice: {second:?}");
        let file = crate::worker_ssh::config_file(&daemon.paths.home);
        let entries = std::fs::read_to_string(&file).unwrap();
        let (w1, w2) = (ssh_alias(&daemon, "w1"), ssh_alias(&daemon, "w2"));
        assert!(entries.contains(&format!("Host {w1}\n")), "{entries}");
        assert!(entries.contains(&format!("Host {w2}\n")), "{entries}");
        assert!(entries.contains("  IdentityFile /keys/google_compute_engine\n"));
        assert_eq!(ssh_host(&daemon, "w1"), Some(w1.clone()));
        assert_eq!(list(&daemon).unwrap().workers[0].ssh_host, Some(w1.clone()));
        delete(&daemon, WorkerParams { name: "w1".into() }).unwrap();
        for _ in 0..100 {
            if load(&daemon, "w1").unwrap().is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let entries = std::fs::read_to_string(&file).unwrap();
        assert!(!entries.contains(&w1), "{entries}");
        assert!(entries.contains(&format!("Host {w2}\n")), "{entries}");
        let config = std::fs::read_to_string(ssh_dir.join("config")).unwrap();
        assert_eq!(config, format!("Include \"{}\"\n", file.display()));
    }

    #[tokio::test]
    async fn recovery_keeps_the_disk_checks_absence_and_never_deletes_a_replacement_vm() {
        let (_temp, daemon, fake) = fixture();
        let record = ready_worker(&daemon, &fake, "w1");
        let disk = "https://www.googleapis.com/compute/v1/projects/test-project/zones/europe-west9-c/disks/boot";
        let instance = fake.join("instances").join(&record.instance);
        let vm = |id: &str| {
            serde_json::json!({"id": id, "labels": {"chauffeur-worker":"w1"}, "disks":[{"boot":true,"source":disk}]}).to_string()
        };
        std::fs::write(&instance, vm("42")).unwrap();
        let script = fake.join("gcloud");
        let body = std::fs::read_to_string(&script).unwrap().replace(
            "  \"compute instances delete\")",
            "  \"compute disks describe\") cat \"$F/disk\" ;;\n  \"compute instances delete\")",
        );
        std::fs::write(&script, body).unwrap();
        std::fs::write(
            fake.join("disk"),
            serde_json::json!({"selfLink":disk, "users":[]}).to_string(),
        )
        .unwrap();
        let held = recovery_worker(&daemon, "w1").unwrap();
        assert!(recovery_worker(&daemon, "w1").is_err());
        assert_eq!(fence_for_recovery(&held).await.unwrap(), disk);
        assert!(!instance.exists());
        assert!(calls(&fake).contains("--keep-disks=all"));
        assert!(!calls(&fake).contains("--delete-disks"));
        assert!(refuse_recovery_worker(&daemon, "w1").is_err());
        // A stale asynchronous SSH refresh must not make a retained worker usable again.
        change(&daemon, "w1", |r| r.state = WorkerState::Ready);
        assert!(!ready(&daemon).contains(&"w1".to_owned()));
        assert!(machine_socket(&daemon, "w1").is_err());
        assert_eq!(list(&daemon).unwrap().workers[0].state, WorkerState::Failed);
        assert!(matches!(
            remote_host(&daemon, "w1"),
            Some(RemoteHost::Unavailable(_))
        ));
        resume(&daemon);
        assert_eq!(
            load(&daemon, "w1").unwrap().unwrap().state,
            WorkerState::Failed
        );

        assert_eq!(
            fence_for_recovery(&held).await.unwrap(),
            disk,
            "a retry checks the kept disk again"
        );
        std::fs::write(&instance, vm("43")).unwrap();
        assert!(
            format!("{:#}", fence_for_recovery(&held).await.unwrap_err()).contains("another VM")
        );
        assert!(instance.exists(), "a replacement VM must never be deleted");
    }

    #[tokio::test]
    async fn an_attached_disk_or_an_unlabelled_vm_never_proves_the_old_worker_stopped() {
        let (_temp, daemon, fake) = fixture();
        let record = ready_worker(&daemon, &fake, "w1");
        let instance = fake.join("instances").join(&record.instance);
        std::fs::write(&instance, r#"{"id":"42","labels":{},"disks":[]}"#).unwrap();
        let held = recovery_worker(&daemon, "w1").unwrap();
        assert!(fence_for_recovery(&held).await.is_err());
        assert!(instance.exists());
        assert!(!calls(&fake).contains("instances delete"));
        let disk = "https://www.googleapis.com/compute/v1/projects/test-project/zones/europe-west9-c/disks/boot";
        daemon
            .store
            .kv_set(
                &recovery_marker("w1"),
                &serde_json::to_string(&RecoveryVm {
                    instance_id: "42".into(),
                    disk: disk.into(),
                })
                .unwrap(),
            )
            .unwrap();
        std::fs::remove_file(&instance).unwrap();
        let script = fake.join("gcloud");
        let body = std::fs::read_to_string(&script).unwrap().replace(
            "  \"compute instances delete\")",
            "  \"compute disks describe\") cat \"$F/disk\" ;;\n  \"compute instances delete\")",
        );
        std::fs::write(&script, body).unwrap();
        std::fs::write(
            fake.join("disk"),
            serde_json::json!({"selfLink":disk,"users":["other-instance"]}).to_string(),
        )
        .unwrap();
        assert!(format!("{:#}", fence_for_recovery(&held).await.unwrap_err()).contains("attached"));
        // Permission denied is never evidence of absence, even with a durable recovery marker.
        let body = std::fs::read_to_string(&script)
            .unwrap()
            .replace("was not found", "PERMISSION_DENIED");
        std::fs::write(&script, body).unwrap();
        assert!(
            format!("{:#}", fence_for_recovery(&held).await.unwrap_err())
                .contains("could not verify")
        );
    }

    /// A ready worker with its forward's remote socket, as one created before SSH entries existed.
    fn ready_worker(daemon: &Daemon, fake: &Path, name: &str) -> Record {
        let record = Record {
            name: name.into(),
            instance: format!("{INSTANCE_PREFIX}{name}"),
            project: "test-project".into(),
            zone: "europe-west9-c".into(),
            machine_type: DEFAULT_MACHINE_TYPE.into(),
            state: WorkerState::Ready,
            log: Vec::new(),
            error: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            remote_socket: Some("/remote.sock".into()),
            ssh: None,
            ssh_error: None,
            backup_bucket: None,
            backup_account: None,
            backup_prefix: None,
        };
        save(daemon, &record).unwrap();
        std::fs::write(
            fake.join("instances").join(&record.instance),
            format!(r#"{{"labels":{{"chauffeur-worker":"{name}"}}}}"#),
        )
        .unwrap();
        record
    }

    async fn until(what: &str, mut check: impl FnMut() -> bool) {
        for _ in 0..100 {
            if check() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("timed out waiting for {what}");
    }

    #[tokio::test]
    async fn a_worker_from_before_the_update_gets_the_include_once_and_a_removed_one_stays_removed()
    {
        let (temp, daemon, fake) = fixture();
        let ssh_dir = temp.path().join("user/.ssh");
        *daemon.workers.ssh_dir.write().unwrap() = Some(ssh_dir.clone());
        ready_worker(&daemon, &fake, "w1");
        resume(&daemon);
        let config = ssh_dir.join("config");
        until("the include", || {
            std::fs::read_to_string(&config).is_ok_and(|c| c.starts_with("Include "))
        })
        .await;
        daemon.workers.stop_forward("w1");
        // The user takes the line out: the next start leaves it out.
        std::fs::write(&config, "Host mine\n").unwrap();
        resume(&daemon).unwrap().await.unwrap();
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "Host mine\n");
        daemon.workers.stop_forward("w1");
    }

    #[tokio::test]
    async fn a_worker_is_not_offered_to_editors_while_the_include_is_missing() {
        let (temp, daemon, fake) = fixture();
        let ssh_dir = temp.path().join("user/.ssh");
        std::fs::create_dir_all(&ssh_dir).unwrap();
        std::fs::write(ssh_dir.join("config"), "Host mine\n").unwrap();
        std::fs::set_permissions(
            ssh_dir.join("config"),
            std::os::unix::fs::PermissionsExt::from_mode(0o444),
        )
        .unwrap();
        *daemon.workers.ssh_dir.write().unwrap() = Some(ssh_dir.clone());
        let record = ready_worker(&daemon, &fake, "w1");
        assert!(add_ssh_entry(&daemon, &record).await.is_err());
        assert_eq!(
            std::fs::read_to_string(ssh_dir.join("config")).unwrap(),
            "Host mine\n"
        );
        assert_eq!(ssh_host(&daemon, "w1"), None);
        let listed = &list(&daemon).unwrap().workers[0];
        assert_eq!(listed.ssh_host, None);
        // The app says why, with the line to add by hand.
        let line =
            crate::worker_ssh::include_line(&crate::worker_ssh::config_file(&daemon.paths.home))
                .unwrap();
        let why = listed.ssh_issue.as_deref().unwrap_or_default();
        assert!(why.contains(&line), "{why}");
    }

    #[tokio::test]
    async fn a_start_rewrites_an_entry_whose_address_changed() {
        let (temp, daemon, fake) = fixture();
        *daemon.workers.ssh_dir.write().unwrap() = Some(temp.path().join("user/.ssh"));
        let mut record = ready_worker(&daemon, &fake, "w1");
        record.ssh = Some(crate::worker_ssh::SshEntry {
            host_name: "10.9.9.9".into(),
            user: "me".into(),
            options: Vec::new(),
        });
        save(&daemon, &record).unwrap();
        write_ssh_config(&daemon).unwrap();
        resume(&daemon);
        let file = crate::worker_ssh::config_file(&daemon.paths.home);
        until("the new address", || {
            std::fs::read_to_string(&file)
                .is_ok_and(|c| !c.contains("10.9.9.9") && c.contains("HostName 10.0.0."))
        })
        .await;
        daemon.workers.stop_forward("w1");
    }

    #[test]
    fn a_daemon_whose_state_is_not_the_users_never_touches_the_users_ssh() {
        let home = Path::new("/Users/me");
        assert_eq!(
            ssh_dir_for(Path::new("/Users/me/.chauffeur"), Some(home), None),
            Some(PathBuf::from("/Users/me/.ssh"))
        );
        assert_eq!(
            ssh_dir_for(Path::new("/tmp/wt/mac/home"), Some(home), None),
            None
        );
        assert_eq!(
            ssh_dir_for(
                Path::new("/tmp/wt/mac/home"),
                Some(home),
                Some(Path::new("/tmp/wt/ssh"))
            ),
            Some(PathBuf::from("/tmp/wt/ssh"))
        );
    }

    #[tokio::test]
    async fn a_throwaway_daemon_writes_its_entries_but_no_include_and_says_which_line_to_add() {
        let (_temp, daemon, fake) = fixture();
        let record = ready_worker(&daemon, &fake, "w1");
        let refused = add_ssh_entry(&daemon, &record)
            .await
            .unwrap_err()
            .to_string();
        assert!(refused.contains("is not ~/.chauffeur"), "{refused}");
        assert!(refused.contains("Include \""), "{refused}");
        assert!(crate::worker_ssh::config_file(&daemon.paths.home).exists());
        let Some(RemoteHost::Unavailable(why)) = remote_host(&daemon, "w1") else {
            panic!("a throwaway daemon's worker was offered to editors");
        };
        assert!(why.contains("Include \""), "{why}");
    }

    #[tokio::test]
    async fn two_daemons_name_their_workers_apart_in_ssh() {
        let (_a, first, _) = fixture();
        let (_b, second, _) = fixture();
        let alias = ssh_alias(&first, "w1");
        assert!(
            alias.starts_with("chauffeur-") && alias.ends_with("-w1"),
            "{alias}"
        );
        assert_eq!(alias.len(), "chauffeur-".len() + 6 + "-w1".len());
        assert_ne!(alias, ssh_alias(&second, "w1"));
        assert_eq!(alias, ssh_alias(&first, "w1"));
    }

    #[test]
    fn workers_install_the_codex_the_sidecar_speaks() {
        let sidecar = include_str!("../../../sidecar/src/version.ts");
        let version = sidecar
            .lines()
            .find_map(|l| l.strip_prefix("export const CODEX_APP_SERVER_VERSION = \""))
            .and_then(|rest| rest.strip_suffix("\";"))
            .expect("the sidecar names its Codex version");
        assert!(
            INSTALL.contains(&format!("\nCODEX_VERSION={version}\n")),
            "worker_install.sh installs another Codex than {version}"
        );
    }

    #[test]
    fn a_gcloud_error_reads_from_its_error_line() {
        assert_eq!(
            gcloud_error(
                "Updating...\nERROR: (gcloud.compute.instances.create) Could not fetch resource:\n - Quota exceeded\n"
            ),
            "ERROR: (gcloud.compute.instances.create) Could not fetch resource: - Quota exceeded"
        );
        assert_eq!(gcloud_error("one\nlast line\n"), "last line");
        assert_eq!(gcloud_error(""), "no output");
    }
    #[tokio::test]
    async fn automatic_cleanup_retries_a_failed_deletion_after_restart_and_leaves_manual_workers() {
        let (_temp, daemon, fake) = fixture();
        for name in ["auto", "manual"] {
            create(&daemon, params(name)).await.unwrap();
            settled(&daemon, name).await;
        }
        daemon.store.kv_set("automatic-worker:auto", "1").unwrap();
        crate::worker_lifecycle::request_cleanup(&daemon, "auto").unwrap();
        std::fs::write(fake.join("account-delete-denied"), "1").unwrap();
        crate::worker_lifecycle::cleanup(&daemon).await.unwrap();
        let record = settled(&daemon, "auto").await;
        assert_eq!(record.state, WorkerState::Failed);
        assert!(record.error.unwrap().contains("PERMISSION_DENIED"));
        assert!(crate::worker_lifecycle::allow_use(&daemon, "auto").is_err());
        change(&daemon, "auto", |r| r.state = WorkerState::Deleting);
        assert!(resume(&daemon).is_none());
        assert_eq!(
            load(&daemon, "auto").unwrap().unwrap().state,
            WorkerState::Failed
        );
        std::fs::remove_file(fake.join("account-delete-denied")).unwrap();
        crate::worker_lifecycle::cleanup(&daemon).await.unwrap();
        for _ in 0..100 {
            if load(&daemon, "auto").unwrap().is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(load(&daemon, "auto").unwrap().is_none());
        assert!(load(&daemon, "manual").unwrap().is_some());
        assert!(fake.join("instances/chauffeur-worker-manual").exists());
        assert!(crate::worker_lifecycle::allow_use(&daemon, "auto").is_ok());
    }

    #[tokio::test]
    async fn an_unused_automatic_worker_is_stopped_with_its_disk_and_started_again_by_the_next_creation()
     {
        let (_temp, daemon, fake) = fixture();
        ready_worker(&daemon, &fake, "auto");
        daemon.store.kv_set("automatic-worker:auto", "1").unwrap();
        crate::worker_lifecycle::cleanup(&daemon).await.unwrap();
        let record = load(&daemon, "auto").unwrap().unwrap();
        assert_eq!(record.state, WorkerState::Stopped);
        assert_eq!(
            record.log.last().map(String::as_str),
            Some("Stopped: only its 50 GB disk costs now, about $5.80 a month")
        );
        assert!(calls(&fake).contains(
            "compute instances stop chauffeur-worker-auto --project test-project --zone europe-west9-c --quiet"
        ));
        assert!(!calls(&fake).contains("instances delete"));
        assert!(fake.join("instances/chauffeur-worker-auto").exists());
        assert!(ready(&daemon).is_empty());
        // A cancelled preparation leaves it stopped, usable at once.
        crate::worker_lifecycle::request_cleanup(&daemon, "auto").unwrap();
        crate::worker_lifecycle::cleanup(&daemon).await.unwrap();
        assert!(crate::worker_lifecycle::allow_use(&daemon, "auto").is_ok());
        assert_eq!(calls(&fake).matches("instances stop").count(), 1);
        create(&daemon, params("auto")).await.unwrap();
        let record = settled(&daemon, "auto").await;
        assert!(calls(&fake).contains("compute instances start chauffeur-worker-auto"));
        assert!(!calls(&fake).contains("instances create"));
        assert!(
            record
                .log
                .iter()
                .any(|l| l == "Starting the VM chauffeur-worker-auto")
        );
        // Chauffeur is installed again, whatever version the disk kept.
        let installs = calls(&fake);
        let started = installs.find("compute instances start").unwrap();
        assert!(installs[started..].contains("compute scp"));
    }

    #[tokio::test]
    async fn a_ready_worker_is_updated_in_place_and_stays_ready_whatever_the_installation_says() {
        let (_temp, daemon, fake) = fixture();
        ready_worker(&daemon, &fake, "w1");
        // The fake's installation fails, after the bundle was copied.
        let error = update(&daemon, "w1").await.unwrap_err();
        assert!(
            format!("{error:#}").contains("Could not update w1 to this Chauffeur version"),
            "{error:#}"
        );
        let calls = calls(&fake);
        assert!(calls.contains("bundle.tar.gz chauffeur-worker-w1:chauffeur-linux-x64.tar.gz"));
        assert!(!calls.contains("instances create") && !calls.contains("instances start"));
        let record = load(&daemon, "w1").unwrap().unwrap();
        assert_eq!(record.state, WorkerState::Ready);
        assert!(
            record
                .log
                .iter()
                .any(|l| l == &format!("Updating Chauffeur to {}", env!("CARGO_PKG_VERSION")))
        );
        assert!(!daemon.workers.busy.lock().unwrap().contains("w1"));
        // One being changed, or not ready, is left alone.
        daemon.workers.busy.lock().unwrap().insert("w1".into());
        assert!(update(&daemon, "w1").await.is_err());
        daemon.workers.busy.lock().unwrap().remove("w1");
        change(&daemon, "w1", |r| r.state = WorkerState::Stopped);
        let refused = update(&daemon, "w1").await.unwrap_err();
        assert!(
            format!("{refused:#}").contains("is not ready"),
            "{refused:#}"
        );
    }

    #[tokio::test]
    async fn a_stop_cut_short_by_a_restart_is_tried_again() {
        let (_temp, daemon, fake) = fixture();
        ready_worker(&daemon, &fake, "auto");
        daemon.store.kv_set("automatic-worker:auto", "1").unwrap();
        change(&daemon, "auto", |r| r.state = WorkerState::Stopping);
        resume(&daemon);
        assert_eq!(
            load(&daemon, "auto").unwrap().unwrap().state,
            WorkerState::Ready
        );
        crate::worker_lifecycle::cleanup(&daemon).await.unwrap();
        assert_eq!(
            load(&daemon, "auto").unwrap().unwrap().state,
            WorkerState::Stopped
        );
        assert!(!daemon.workers.forwards.lock().unwrap().contains_key("auto"));
    }

    #[tokio::test]
    async fn automatic_cleanup_never_deletes_a_vm_or_disk_retained_for_backup_recovery() {
        let (_temp, daemon, fake) = fixture();
        create(&daemon, params("auto")).await.unwrap();
        settled(&daemon, "auto").await;
        daemon.store.kv_set("automatic-worker:auto", "1").unwrap();
        crate::worker_lifecycle::request_cleanup(&daemon, "auto").unwrap();
        daemon
            .store
            .kv_set(&recovery_marker("auto"), "kept")
            .unwrap();
        crate::worker_lifecycle::cleanup(&daemon).await.unwrap();
        assert!(fake.join("instances/chauffeur-worker-auto").exists());
        assert!(!calls(&fake).contains("compute instances delete"));
    }

    fn entry() -> crate::worker_ssh::SshEntry {
        crate::worker_ssh::SshEntry {
            host_name: "10.0.0.7".into(),
            user: "me".into(),
            options: vec![
                ("IdentityFile".into(), "/keys/google_compute_engine".into()),
                ("HostKeyAlias".into(), "compute.7".into()),
                ("StrictHostKeyChecking".into(), "yes".into()),
            ],
        }
    }

    #[tokio::test]
    async fn a_worker_with_an_ssh_entry_stays_forwarded_while_gcloud_is_signed_out() {
        let (_temp, daemon, fake) = fixture();
        let mut record = ready_worker(&daemon, &fake, "w1");
        record.ssh = Some(entry());
        save(&daemon, &record).unwrap();
        std::fs::write(fake.join("signed-out"), "").unwrap();
        start_forward(&daemon, "w1", "/remote.sock", &record);
        let local = socket_path(&daemon, "w1");
        until("the forwarded socket", || local.exists()).await;
        let ssh = std::fs::read_to_string(fake.join("ssh-calls")).unwrap();
        let binding = format!("-L {}:/remote.sock", local.display());
        for arg in [
            "-o IdentityFile=/keys/google_compute_engine",
            "-o HostKeyAlias=compute.7",
            "-o StrictHostKeyChecking=yes",
            "-o BatchMode=yes",
            "-N",
            &binding,
            "me@10.0.0.7",
        ] {
            assert!(ssh.contains(arg), "{arg} missing from {ssh}");
        }
        assert!(!calls(&fake).contains("compute ssh"), "{}", calls(&fake));
        daemon.workers.stop_forward("w1");
    }

    #[tokio::test]
    async fn a_transfer_to_a_worker_gcloud_cannot_reach_says_to_sign_gcloud_in_at_once() {
        let (_temp, daemon, fake) = fixture();
        // No entry yet: its forward goes through gcloud, whose session expired.
        let record = ready_worker(&daemon, &fake, "w1");
        std::fs::write(fake.join("signed-out"), "").unwrap();
        start_forward(&daemon, "w1", "/remote.sock", &record);
        until("the forward's issue", || {
            forward_issue(&daemon, "w1").is_some()
        })
        .await;
        let issue = forward_issue(&daemon, "w1").unwrap();
        assert_eq!(issue.message, SIGNED_OUT);
        assert!(issue.lasting);
        // Settings → Transfers says why it is not connected.
        let listed = &list(&daemon).unwrap().workers[0];
        assert!(!listed.connected);
        assert_eq!(listed.error.as_deref(), Some(SIGNED_OUT));
        // Owned by this daemon, as an on-demand worker is.
        daemon.store.kv_set("automatic-worker:w1", "1").unwrap();
        let prepared = tokio::time::timeout(
            Duration::from_secs(10),
            crate::worker_lifecycle::destination(&daemon, "ws1", "t1", false, |_| {}),
        )
        .await
        .expect("the transfer waited instead of failing");
        let Err(refused) = prepared else {
            panic!("the transfer went on to an unreachable worker");
        };
        assert!(
            refused
                .to_string()
                .contains(&format!("Could not reach w1: {SIGNED_OUT}")),
            "{refused}"
        );
        daemon.workers.stop_forward("w1");
    }

    #[tokio::test]
    async fn a_refused_key_is_said_in_ssh_s_words_and_the_entry_read_again() {
        let (_temp, daemon, fake) = fixture();
        let mut record = ready_worker(&daemon, &fake, "w1");
        record.ssh = Some(entry());
        save(&daemon, &record).unwrap();
        std::fs::write(fake.join("key-refused"), "").unwrap();
        start_forward(&daemon, "w1", "/remote.sock", &record);
        until("the forward's issue", || {
            forward_issue(&daemon, "w1").is_some()
        })
        .await;
        let issue = forward_issue(&daemon, "w1").unwrap();
        assert_eq!(issue.message, "me@10.0.0.7: Permission denied (publickey).");
        assert!(issue.lasting);
        // Its address may have changed: gcloud is asked again.
        until("the entry read again", || {
            calls(&fake).contains("compute ssh chauffeur-worker-w1 --dry-run")
        })
        .await;
        daemon.workers.stop_forward("w1");
    }

    #[test]
    fn an_expired_gcloud_session_reads_as_what_to_do() {
        let stderr = "ERROR: (gcloud.compute.ssh) There was a problem refreshing your current auth tokens: Reauthentication failed. cannot prompt during non-interactive execution.\nPlease run:\n\n  $ gcloud auth login\n\nto obtain new credentials.\n";
        assert_eq!(gcloud_error(stderr), SIGNED_OUT);
        let other = "ERROR: (gcloud.compute.instances.describe) The resource was not found";
        assert_eq!(gcloud_error(other), other);
    }
}
