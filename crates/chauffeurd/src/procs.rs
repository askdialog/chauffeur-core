//! Setup, run and archive scripts, each in its own process group so it can be stopped whole,
//! plus the sweep that kills whatever still runs inside a workspace folder at archive.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chauffeur_protocol::client::ProcessInfo;
use chauffeur_protocol::events::{OutputStream, ProcessKind, WorkspaceEvent};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;

use crate::events::EventBus;
use crate::shellenv::ShellEnv;

const TAIL_LINES: usize = 300;

/// Spawns `command`, waiting out "Text file busy" (ETXTBSY) for up to two seconds. On Linux a
/// binary cannot be run while any process holds a write descriptor to it, and a process forked
/// while another thread writes the file keeps a copy of that descriptor until its own `exec`: a
/// binary written a moment ago (an update, a test's stand-in) can fail to start for an instant.
pub async fn spawn_when_free<T>(
    mut spawn: impl FnMut() -> std::io::Result<T>,
) -> std::io::Result<T> {
    let mut attempts = 0;
    loop {
        match spawn() {
            Err(error)
                if error.kind() == std::io::ErrorKind::ExecutableFileBusy && attempts < 200 =>
            {
                attempts += 1;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            spawned => return spawned,
        }
    }
}

#[derive(Clone)]
pub struct ProcessManager {
    events: EventBus,
    execution: crate::workspace_execution::ExecutionGate,
    env: ShellEnv,
    procs: Arc<Mutex<HashMap<String, Proc>>>,
    setup_start: Arc<Mutex<()>>,
}

struct Proc {
    cwd: PathBuf,
    info: ProcessInfo,
    pgid: Option<i32>,
    child: Arc<Mutex<tokio::process::Child>>,
    tail: Arc<Mutex<VecDeque<String>>>,
    exited: watch::Receiver<Option<Option<i32>>>,
    wait_error: Arc<Mutex<Option<String>>>,
}

pub struct ScriptSpec {
    pub workspace_id: String,
    pub kind: ProcessKind,
    pub repo: Option<String>,
    pub name: String,
    pub command: String,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
}

impl ProcessManager {
    pub fn new(
        events: EventBus,
        env: ShellEnv,
        execution: crate::workspace_execution::ExecutionGate,
    ) -> ProcessManager {
        ProcessManager {
            events,
            execution,
            env,
            procs: Arc::default(),
            setup_start: Arc::default(),
        }
    }

    pub fn directory(&self, id: &str) -> Option<PathBuf> {
        self.procs
            .lock()
            .unwrap()
            .get(id)
            .map(|process| process.cwd.clone())
    }

    /// A setup already running in this exact worktree is returned, never started twice.
    pub fn spawn_setup(&self, spec: ScriptSpec) -> anyhow::Result<ProcessInfo> {
        anyhow::ensure!(spec.kind == ProcessKind::Setup, "Expected a setup script");
        let _serial = self.setup_start.lock().unwrap();
        if let Some(existing) = self.procs.lock().unwrap().values().find(|p| {
            p.info.kind == ProcessKind::Setup
                && p.info.running
                && p.cwd == spec.cwd
                && p.info.workspace_id == spec.workspace_id
        }) {
            return Ok(existing.info.clone());
        }
        self.spawn(spec)
    }

    /// Starts a script with `zsh -c` (bash where zsh is not installed) in its own session;
    /// output is streamed as events.
    pub fn spawn(&self, spec: ScriptSpec) -> anyhow::Result<ProcessInfo> {
        let _admission = self.execution.admit(&spec.workspace_id)?;
        let id = uuid::Uuid::now_v7().to_string();
        let mut command = Command::new(crate::shellenv::script_shell());
        command
            .arg("-c")
            .arg(&spec.command)
            .current_dir(&spec.cwd)
            .env_clear()
            .envs(self.env.vars())
            .envs(&spec.env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Its own session, not only its own group: a job the script detaches keeps that session,
        // which tells it apart from the user's programs once the script is gone (see
        // `workspace_processes`). The group id stays the script's pid.
        // SAFETY: setsid is async-signal-safe and touches nothing shared with the parent.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn()?;
        let pgid = child.id().map(|pid| pid as i32);
        let info = ProcessInfo {
            id: id.clone(),
            workspace_id: spec.workspace_id.clone(),
            kind: spec.kind,
            repo: spec.repo.clone(),
            name: spec.name.clone(),
            command: spec.command.clone(),
            running: true,
            exit_code: None,
            tail: Vec::new(),
        };
        self.events.workspace(
            &spec.workspace_id,
            WorkspaceEvent::ProcessStarted {
                process_id: id.clone(),
                kind: spec.kind,
                repo: spec.repo.clone(),
                name: spec.name.clone(),
                command: spec.command.clone(),
            },
        );
        let tail: Arc<Mutex<VecDeque<String>>> = Arc::default();
        let (exit_tx, exit_rx) = watch::channel(None);
        let wait_error = Arc::new(Mutex::new(None));
        for (stream, reader) in [
            (OutputStream::Stdout, child.stdout.take().map(boxed)),
            (OutputStream::Stderr, child.stderr.take().map(boxed)),
        ] {
            let Some(reader) = reader else { continue };
            let events = self.events.clone();
            let tail = tail.clone();
            let workspace_id = spec.workspace_id.clone();
            let process_id = id.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(reader).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    {
                        let mut tail = tail.lock().unwrap();
                        if tail.len() == TAIL_LINES {
                            tail.pop_front();
                        }
                        tail.push_back(line.clone());
                    }
                    events.workspace(
                        &workspace_id,
                        WorkspaceEvent::ProcessOutput {
                            process_id: process_id.clone(),
                            stream,
                            line,
                        },
                    );
                }
            });
        }
        let child = Arc::new(Mutex::new(child));
        {
            let child = child.clone();
            let events = self.events.clone();
            let wait_error = wait_error.clone();
            let procs = self.procs.clone();
            let workspace_id = spec.workspace_id.clone();
            let process_id = id.clone();
            tokio::spawn(async move {
                // The same lock covers wait/reap and signals: a reaped PID must never be
                // reused as the target of a historical stop, including the final escalation.
                let code = loop {
                    let outcome = child.lock().unwrap().try_wait();
                    match outcome {
                        Ok(Some(status)) => break status.code(),
                        Ok(None) => tokio::time::sleep(Duration::from_millis(25)).await,
                        Err(error) => {
                            *wait_error.lock().unwrap() = Some(error.to_string());
                            tracing::warn!(%error, process_id, "could not confirm script exit");
                            return;
                        }
                    }
                };
                // Let the readers drain the last lines.
                tokio::time::sleep(Duration::from_millis(50)).await;
                if let Some(proc) = procs.lock().unwrap().get_mut(&process_id) {
                    proc.info.running = false;
                    proc.info.exit_code = code;
                }
                events.workspace(
                    &workspace_id,
                    WorkspaceEvent::ProcessExited {
                        process_id,
                        exit_code: code,
                    },
                );
                let _ = exit_tx.send(Some(code));
            });
        }
        self.procs.lock().unwrap().insert(
            id,
            Proc {
                cwd: spec.cwd,
                info: info.clone(),
                pgid,
                child,
                tail,
                exited: exit_rx,
                wait_error,
            },
        );
        Ok(info)
    }

    /// Runs a script to completion (or until `timeout`), returning its exit code.
    pub async fn run_to_end(&self, spec: ScriptSpec, timeout: Duration) -> anyhow::Result<i32> {
        let info = self.spawn(spec)?;
        let mut exited = self.procs.lock().unwrap()[&info.id].exited.clone();
        match tokio::time::timeout(timeout, wait_exit(&mut exited)).await {
            Ok(Some(code)) => Ok(code.unwrap_or(-1)),
            Ok(None) => anyhow::bail!("lost track of {}", info.name),
            Err(_) => {
                self.stop(&info.id).await;
                anyhow::bail!("{} timed out after {} s", info.name, timeout.as_secs())
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<ProcessInfo> {
        let procs = self.procs.lock().unwrap();
        procs.get(id).map(with_tail)
    }

    pub fn list(&self, workspace_id: &str) -> Vec<ProcessInfo> {
        let procs = self.procs.lock().unwrap();
        let mut list: Vec<ProcessInfo> = procs
            .values()
            .filter(|proc| proc.info.workspace_id == workspace_id)
            .map(with_tail)
            .collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        list
    }

    /// The process groups of `workspace_id`'s running run scripts, with their names: what a
    /// listening port's group is matched against.
    pub fn running_run_groups(&self, workspace_id: &str) -> Vec<(i32, String)> {
        let procs = self.procs.lock().unwrap();
        procs
            .values()
            .filter(|proc| {
                proc.info.running
                    && proc.info.kind == ProcessKind::Run
                    && proc.info.workspace_id == workspace_id
            })
            .filter_map(|proc| Some((proc.pgid?, proc.info.name.clone())))
            .collect()
    }

    /// Running run scripts of a repo in any workspace, for non-concurrent repos.
    pub fn running_runs_of_repo(&self, repo: &str) -> Vec<ProcessInfo> {
        let procs = self.procs.lock().unwrap();
        procs
            .values()
            .filter(|proc| {
                proc.info.running
                    && proc.info.kind == ProcessKind::Run
                    && proc.info.repo.as_deref() == Some(repo)
            })
            .map(|proc| proc.info.clone())
            .collect()
    }

    /// SIGTERM the process group, SIGKILL after 5 s.
    pub async fn stop(&self, id: &str) -> Option<ProcessInfo> {
        let (pgid, child, mut exited) = {
            let procs = self.procs.lock().unwrap();
            let proc = procs.get(id)?;
            (proc.pgid, proc.child.clone(), proc.exited.clone())
        };
        if let Some(pgid) = pgid {
            let _ = signal_managed_group(&child, pgid, libc::SIGTERM);
            let done = tokio::time::timeout(Duration::from_secs(5), wait_exit(&mut exited)).await;
            if done.is_err() {
                let _ = signal_managed_group(&child, pgid, libc::SIGKILL);
                let _ = tokio::time::timeout(Duration::from_secs(2), wait_exit(&mut exited)).await;
            }
        }
        self.get(id)
    }

    /// Confirm the registered leader was reaped and its process group is absent. This is
    /// not proof about detached descendants, services or launches lost across daemon restart.
    /// A surviving group after its leader exits is left for explicit reconciliation: never
    /// signal a historical group whose original leader can no longer be identified.
    pub async fn stop_and_wait(&self, id: &str, grace: Duration) -> crate::Result<ProcessInfo> {
        let (pgid, child, mut exited, wait_error) = {
            let procs = self.procs.lock().unwrap();
            let proc = procs
                .get(id)
                .ok_or_else(|| crate::Error::not_found(format!("process {id}")))?;
            (
                proc.pgid,
                proc.child.clone(),
                proc.exited.clone(),
                proc.wait_error.clone(),
            )
        };
        let pgid = pgid.filter(|pid| *pid > 1).ok_or_else(|| {
            crate::Error::Unavailable(format!(
                "Process {id} has no verified group; reconcile it before transferring"
            ))
        })?;
        if exited.borrow().is_none() && wait_error.lock().unwrap().is_none() {
            signal_managed_group(&child, pgid, libc::SIGTERM)?;
            if tokio::time::timeout(grace, wait_exit(&mut exited))
                .await
                .is_err()
            {
                // Check again before escalation: a root already reaped must not be signalled.
                if exited.borrow().is_none() && wait_error.lock().unwrap().is_none() {
                    signal_managed_group(&child, pgid, libc::SIGKILL)?;
                }
                tokio::time::timeout(Duration::from_secs(2), wait_exit(&mut exited))
                    .await
                    .map_err(|_| {
                        crate::Error::Unavailable(format!(
                            "Process {id} did not confirm exit; keep the workspace fenced and retry"
                        ))
                    })?;
            }
        }
        if let Some(error) = wait_error.lock().unwrap().as_ref() {
            return Err(crate::Error::Unavailable(format!(
                "Process {id} exit could not be verified ({error}); reconcile it before transferring"
            )));
        }
        if exited.borrow().is_none() {
            return Err(crate::Error::Unavailable(format!(
                "Process {id} lost its exit watcher; reconcile it before transferring"
            )));
        }
        if group_exists(pgid)? {
            return Err(crate::Error::Conflict(format!(
                "Process {id} still has a surviving group; reconcile it before transferring"
            )));
        }
        let info = self
            .get(id)
            .ok_or_else(|| crate::Error::not_found(format!("process {id}")))?;
        if info.running {
            return Err(crate::Error::Unavailable(format!(
                "Process {id} has inconsistent exit metadata; reconcile it before transferring"
            )));
        }
        Ok(info)
    }

    /// Attempt every registered script, retaining observations and errors for reconciliation.
    pub async fn stop_workspace_and_wait(
        &self,
        workspace: &str,
        grace: Duration,
    ) -> crate::Result<()> {
        let results = futures_util::future::join_all(
            self.list(workspace)
                .iter()
                .map(|p| self.stop_and_wait(&p.id, grace)),
        )
        .await;
        for result in results {
            result?;
        }
        Ok(())
    }

    pub async fn stop_workspace(&self, workspace_id: &str) {
        let ids: Vec<String> = self
            .list(workspace_id)
            .into_iter()
            .filter(|p| p.running)
            .map(|p| p.id)
            .collect();
        futures_util::future::join_all(ids.iter().map(|id| self.stop(id))).await;
    }

    pub fn forget_workspace(&self, workspace_id: &str) {
        // An exited leader can leave a surviving group. Keep its observation while
        // fenced (or if admission cannot be verified), including between stop retries.
        let Ok(_admission) = self.execution.admit(workspace_id) else {
            return;
        };
        self.procs
            .lock()
            .unwrap()
            .retain(|_, proc| proc.info.workspace_id != workspace_id || proc.info.running);
    }
}

/// Waits for a process to exit; `None` if its watcher is gone. The value is copied out so no
/// borrow of the channel is held across awaits.
async fn wait_exit(exited: &mut watch::Receiver<Option<Option<i32>>>) -> Option<Option<i32>> {
    exited
        .wait_for(|code| code.is_some())
        .await
        .ok()
        .and_then(|code| *code)
}

fn boxed<R: tokio::io::AsyncRead + Send + Unpin + 'static>(
    reader: R,
) -> Box<dyn tokio::io::AsyncRead + Send + Unpin> {
    Box::new(reader)
}

fn with_tail(proc: &Proc) -> ProcessInfo {
    let mut info = proc.info.clone();
    info.tail = proc.tail.lock().unwrap().iter().cloned().collect();
    info
}

fn signal_managed_group(
    child: &Arc<Mutex<tokio::process::Child>>,
    pgid: i32,
    signal: i32,
) -> crate::Result<()> {
    let mut child = child.lock().unwrap();
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    if child.id() != Some(pgid as u32) {
        return Err(crate::Error::Unavailable(
            "Script leader identity was lost; reconcile it before transferring".into(),
        ));
    }
    // SAFETY: positive managed process-group identifier; no broadcast or own-group target.
    if unsafe { libc::killpg(pgid, signal) } == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    // macOS refuses (EPERM) to signal a group whose members are all exiting: a leader that ends
    // just after `try_wait` lands here. It is no refusal once the leader turns out to have exited.
    if error.raw_os_error() == Some(libc::EPERM) {
        for _ in 0..200 {
            if child.try_wait()?.is_some() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    Err(crate::Error::Unavailable(format!(
        "Could not signal process group {pgid} ({error}); keep the workspace fenced and reconcile it"
    )))
}

fn group_exists(pgid: i32) -> crate::Result<bool> {
    // SAFETY: signal zero only inspects existence; permission errors are not absence.
    if unsafe { libc::killpg(pgid, 0) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(false);
    }
    Err(crate::Error::Unavailable(format!(
        "Could not inspect process group {pgid} ({error}); reconcile it before transferring"
    )))
}

fn signal_group(pgid: i32, signal: i32) {
    // SAFETY: killpg only sends a signal; a stale pgid fails with ESRCH.
    unsafe {
        libc::killpg(pgid, signal);
    }
}

/// Processes whose working directory is inside `dir`. This catches what agents and terminals
/// started and left behind (dev servers re-parented to launchd or init included).
pub async fn processes_inside(dir: &Path) -> Vec<i32> {
    let mut pids: Vec<i32> = working_dirs()
        .await
        .into_iter()
        .filter(|(_, cwd)| cwd.starts_with(dir))
        .map(|(pid, _)| pid)
        .collect();
    pids.sort_unstable();
    pids.dedup();
    pids
}

/// Every process's working directory but the daemon's, found with `lsof`.
#[cfg(target_os = "macos")]
async fn working_dirs() -> Vec<(i32, PathBuf)> {
    let output = Command::new("lsof")
        .args(["-a", "-d", "cwd", "-F", "pn", "-w"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await;
    let Ok(output) = output else {
        return Vec::new();
    };
    let own = std::process::id() as i32;
    let mut dirs = Vec::new();
    let mut current: Option<i32> = None;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(pid) = line.strip_prefix('p') {
            current = pid.parse().ok();
        } else if let Some(path) = line.strip_prefix('n')
            && let Some(pid) = current
            && pid != own
        {
            dirs.push((pid, PathBuf::from(path)));
        }
    }
    dirs
}

/// Every process's working directory but the daemon's, read from `/proc/<pid>/cwd`; other
/// users' processes are unreadable, and skipped.
#[cfg(not(target_os = "macos"))]
async fn working_dirs() -> Vec<(i32, PathBuf)> {
    tokio::task::spawn_blocking(|| {
        let own = std::process::id() as i32;
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
            .filter(|&pid| pid != own)
            .filter_map(|pid| Some((pid, std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?)))
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// A running process, as `ps` and its working directory describe it.
#[derive(Clone, Debug)]
pub struct RunningProcess {
    pub pid: i32,
    pub ppid: i32,
    pub cwd: Option<PathBuf>,
    pub command: String,
}

/// Every process: one `ps` and one scan of working directories (about 0.3 s of CPU on macOS).
pub async fn listing() -> Vec<RunningProcess> {
    let dirs: HashMap<i32, PathBuf> = working_dirs().await.into_iter().collect();
    let output = Command::new("ps")
        .args(["-axo", "pid=,ppid=,command="])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await;
    let Ok(output) = output else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid: i32 = fields.next()?.parse().ok()?;
            let ppid: i32 = fields.next()?.parse().ok()?;
            let command = fields.collect::<Vec<_>>().join(" ");
            Some(RunningProcess {
                pid,
                ppid,
                cwd: dirs.get(&pid).cloned(),
                command,
            })
        })
        .collect()
}

/// The Nx daemons running in `dir`, to stop once its workspace is left alone: only when nothing
/// else runs there (a dev server, tests, an `nx` command in a terminal…), the daemons' own
/// workers and idle shells aside. Nx starts a daemon again on its next command.
pub fn idle_nx_daemons(procs: &[RunningProcess], dir: &Path) -> Vec<i32> {
    let inside: Vec<&RunningProcess> = procs
        .iter()
        .filter(|p| p.cwd.as_deref().is_some_and(|cwd| cwd.starts_with(dir)))
        .collect();
    let daemons: Vec<i32> = inside
        .iter()
        .filter(|p| is_nx_daemon(&p.command))
        .map(|p| p.pid)
        .collect();
    let parents: std::collections::HashSet<i32> = procs.iter().map(|p| p.ppid).collect();
    let busy = inside.iter().any(|p| {
        !daemons.contains(&p.pid)
            && !daemons.contains(&p.ppid)
            && !(is_shell(&p.command) && !parents.contains(&p.pid))
    });
    if busy { Vec::new() } else { daemons }
}

fn is_nx_daemon(command: &str) -> bool {
    command.contains("/nx/") && command.contains("/daemon/server/start.js")
}

/// An interactive shell, as a terminal starts it (`-zsh`, `/bin/zsh -l`).
fn is_shell(command: &str) -> bool {
    let mut words = command.split_whitespace();
    let program = words.next().unwrap_or_default();
    let name = program
        .rsplit('/')
        .next()
        .unwrap_or(program)
        .trim_start_matches('-');
    ["zsh", "bash", "sh", "fish"].contains(&name)
        && words.all(|word| ["-l", "-i", "--login", "-il", "-li"].contains(&word))
}

/// SIGTERM every process running inside `dir`, then SIGKILL the survivors after `grace`.
pub async fn kill_inside(dir: &Path, grace: Duration) -> usize {
    let pids = processes_inside(dir).await;
    if pids.is_empty() {
        return 0;
    }
    for pid in &pids {
        // SAFETY: sending a signal to a pid; a stale pid fails with ESRCH.
        unsafe {
            libc::kill(*pid, libc::SIGTERM);
        }
    }
    tokio::time::sleep(grace).await;
    for pid in processes_inside(dir).await {
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
    pids.len()
}

/// How long `docker compose ls` may take to answer (decided with Justin 2026-10-18).
pub const COMPOSE_LS_TIMEOUT: Duration = Duration::from_secs(10);

/// How long Docker may take: `ls` to list the projects, `down` to stop each one.
#[derive(Clone, Copy, Debug)]
pub struct ComposeTimeouts {
    pub ls: Duration,
    pub down: Duration,
}

/// A Docker call that did not end in time; its text is the warning shown to the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DockerTimeout {
    Ls(Duration),
    Down { project: String, after: Duration },
}

impl std::fmt::Display for DockerTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DockerTimeout::Ls(after) => write!(
                f,
                "Docker did not answer within {}: this workspace's compose projects may still be \
                 running; stop them with `docker compose ls` then `docker compose -p <name> down`",
                seconds(*after)
            ),
            DockerTimeout::Down { project, after } => write!(
                f,
                "`docker compose -p {project} down` did not end within {}: compose project \
                 {project} may still be running; stop it with `docker compose -p {project} down`",
                seconds(*after)
            ),
        }
    }
}

fn seconds(duration: Duration) -> String {
    if duration.subsec_millis() == 0 {
        format!("{} s", duration.as_secs())
    } else {
        format!("{:.1} s", duration.as_secs_f64())
    }
}

/// What [`compose_down_inside`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ComposeDown {
    /// Projects stopped.
    pub stopped: Vec<String>,
    /// Docker calls killed for taking too long.
    pub timed_out: Vec<DockerTimeout>,
}

/// `docker compose down` every compose project whose config files live inside `dir`. An absent
/// or stopped Docker is not an error, nor a hung one: each call is killed past its timeout and
/// reported in [`ComposeDown::timed_out`], so that archiving never waits on Docker.
pub async fn compose_down_inside(
    dir: &Path,
    env: &ShellEnv,
    timeouts: ComposeTimeouts,
) -> ComposeDown {
    let mut outcome = ComposeDown::default();
    let output = match docker(
        &["compose", "ls", "--all", "--format", "json"],
        env,
        timeouts.ls,
    )
    .await
    {
        Docker::Exited {
            success: true,
            stdout,
        } => stdout,
        Docker::Exited { .. } | Docker::Unavailable => return outcome,
        Docker::TimedOut => {
            outcome.timed_out.push(DockerTimeout::Ls(timeouts.ls));
            return outcome;
        }
    };
    #[derive(serde::Deserialize)]
    struct Project {
        #[serde(rename = "Name")]
        name: String,
        #[serde(rename = "ConfigFiles", default)]
        config_files: String,
    }
    let projects: Vec<Project> = serde_json::from_slice(&output).unwrap_or_default();
    for project in projects {
        let inside = project
            .config_files
            .split(',')
            .any(|file| Path::new(file.trim()).starts_with(dir));
        if !inside {
            continue;
        }
        let args = [
            "compose",
            "-p",
            &project.name,
            "down",
            "--volumes",
            "--remove-orphans",
        ];
        match docker(&args, env, timeouts.down).await {
            Docker::Exited { success: true, .. } => outcome.stopped.push(project.name),
            Docker::Exited { .. } | Docker::Unavailable => {}
            Docker::TimedOut => outcome.timed_out.push(DockerTimeout::Down {
                project: project.name,
                after: timeouts.down,
            }),
        }
    }
    outcome
}

/// How a `docker` call ended.
enum Docker {
    Exited {
        success: bool,
        stdout: Vec<u8>,
    },
    /// Not installed (or not startable).
    Unavailable,
    /// Killed, with its whole process group, past its timeout.
    TimedOut,
}

/// Runs `docker args` in its own process group for at most `timeout`. Past it the whole group is
/// killed, `docker` and the compose plugin it runs as a child (killing `docker` alone would
/// leave the plugin running), and `docker` is reaped: nothing is left behind.
async fn docker(args: &[&str], env: &ShellEnv, timeout: Duration) -> Docker {
    let child = Command::new("docker")
        .args(args)
        .env_clear()
        .envs(env.vars())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .kill_on_drop(true)
        .spawn();
    let Ok(mut child) = child else {
        return Docker::Unavailable;
    };
    let pgid = child.id().map(|pid| pid as i32);
    let mut stdout = child.stdout.take();
    let ended = tokio::time::timeout(timeout, async {
        let mut out = Vec::new();
        if let Some(stdout) = stdout.as_mut() {
            let _ = stdout.read_to_end(&mut out).await;
        }
        (child.wait().await, out)
    })
    .await;
    match ended {
        Ok((Ok(status), stdout)) => Docker::Exited {
            success: status.success(),
            stdout,
        },
        Ok((Err(_), _)) => Docker::Unavailable,
        Err(_) => {
            // Docker already failed to answer: nothing graceful to wait for. `docker` is not
            // reaped yet, so its pid still names this group.
            if let Some(pgid) = pgid {
                signal_group(pgid, libc::SIGKILL);
            }
            // Reaped here; should it be stuck in the kernel, tokio reaps it once dropped.
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            Docker::TimedOut
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NX: &str = "/usr/bin/node --conditions development /ws/.worktrees/abc-1/shop/node_modules/.pnpm/nx@23.2.1/node_modules/nx/dist/src/daemon/server/start.js";

    fn proc(pid: i32, ppid: i32, cwd: &str, command: &str) -> RunningProcess {
        RunningProcess {
            pid,
            ppid,
            cwd: Some(PathBuf::from(cwd)),
            command: command.into(),
        }
    }

    /// A fake `docker` running `body`, first on the `PATH` of the environment returned.
    fn fake_docker(dir: &Path, body: &str) -> ShellEnv {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let docker = bin.join("docker");
        std::fs::write(
            &docker,
            format!("#!/bin/sh\n[ \"$1\" = --warm-up ] && exit 0\n{body}\n"),
        )
        .unwrap();
        std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o755)).unwrap();
        // macOS checks a freshly written executable on its first exec, which can take seconds:
        // paid here, it stays out of the deadlines under test. On Linux it is also "Text file
        // busy" while a process another test forked meanwhile holds the file open for writing.
        let mut warm_up = None;
        for _ in 0..200 {
            match std::process::Command::new(&docker)
                .arg("--warm-up")
                .status()
            {
                Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                status => {
                    warm_up = Some(status.unwrap());
                    break;
                }
            }
        }
        assert!(warm_up.expect("the fake docker stays busy").success());
        let mut vars: BTreeMap<String, String> = std::env::vars().collect();
        let path = format!("{}:{}", bin.display(), vars["PATH"]);
        vars.insert("PATH".into(), path);
        let env = ShellEnv::new();
        env.set_vars(vars);
        env
    }

    const QUICK: ComposeTimeouts = ComposeTimeouts {
        ls: Duration::from_millis(500),
        down: Duration::from_millis(500),
    };

    fn alive(pid: i32) -> bool {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    fn read_pid(path: &Path) -> i32 {
        std::fs::read_to_string(path)
            .unwrap_or_else(|_| panic!("the fake docker never wrote {}", path.display()))
            .trim()
            .parse()
            .unwrap()
    }

    /// Waits up to 3 s for `pid` to be gone (reaped by its new parent, if orphaned).
    async fn gone(pid: i32) -> bool {
        for _ in 0..60 {
            if !alive(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    /// Lists a project with its compose file in `ws`, and another one elsewhere.
    fn listing_json(ws: &Path) -> String {
        format!(
            r#"[{{"Name":"inside","Status":"running(1)","ConfigFiles":"{}/repo/compose.yml"}},{{"Name":"outside","Status":"running(1)","ConfigFiles":"/elsewhere/compose.yml"}}]"#,
            ws.display()
        )
    }

    #[tokio::test]
    async fn a_hung_docker_is_killed_with_its_children_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // `docker` and a child standing for the compose plugin, both sleeping forever.
        let env = fake_docker(
            root,
            &format!(
                "echo $$ > {0}/docker.pid\nsleep 1000 &\necho $! > {0}/plugin.pid\nwait",
                root.display()
            ),
        );
        let started = std::time::Instant::now();
        let outcome = compose_down_inside(&root.join("ws"), &env, QUICK).await;
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(
            outcome,
            ComposeDown {
                stopped: vec![],
                timed_out: vec![DockerTimeout::Ls(QUICK.ls)],
            }
        );
        let docker = read_pid(&root.join("docker.pid"));
        let plugin = read_pid(&root.join("plugin.pid"));
        assert!(gone(docker).await, "docker is killed and reaped");
        assert!(gone(plugin).await, "its child is killed too");
        // SAFETY: signal 0 to a process group only checks that it has members.
        assert_eq!(unsafe { libc::killpg(docker, 0) }, -1, "the group is empty");
    }

    #[test]
    fn docker_s_timeouts_read_as_warnings() {
        assert_eq!(
            DockerTimeout::Ls(COMPOSE_LS_TIMEOUT).to_string(),
            "Docker did not answer within 10 s: this workspace's compose projects may still be \
             running; stop them with `docker compose ls` then `docker compose -p <name> down`"
        );
        let down = DockerTimeout::Down {
            project: "qa-shop".into(),
            after: Duration::from_secs(300),
        };
        assert!(
            down.to_string()
                .starts_with("`docker compose -p qa-shop down` did not end within 300 s"),
            "{down}"
        );
    }

    #[tokio::test]
    async fn a_binary_still_open_for_writing_starts_once_it_is_closed() {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("tool");
        let mut writer = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&binary)
            .unwrap();
        writer.write_all(b"#!/bin/sh\nexit 0\n").unwrap();
        // Another thread's fork would hold the descriptor like this one, then drop it.
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            drop(writer);
        });
        let mut command = Command::new(&binary);
        let mut child = spawn_when_free(|| command.spawn()).await.unwrap();
        assert!(child.wait().await.unwrap().success());
        release.join().unwrap();
    }

    #[tokio::test]
    async fn the_projects_inside_the_folder_are_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let ws = root.join("ws");
        let calls = root.join("calls.log");
        let env = fake_docker(
            root,
            &format!(
                "echo \"$*\" >> {}\ncase \"$*\" in\n  \"compose ls --all --format json\") echo '{}' ;;\nesac",
                calls.display(),
                listing_json(&ws)
            ),
        );
        // This checks filtering/arguments, not deadline behavior. Parallel process tests can
        // delay a healthy fixture beyond QUICK; hung-command tests retain their short budget.
        let outcome = compose_down_inside(
            &ws,
            &env,
            ComposeTimeouts {
                ls: Duration::from_secs(5),
                down: Duration::from_secs(5),
            },
        )
        .await;
        assert_eq!(
            outcome,
            ComposeDown {
                stopped: vec!["inside".into()],
                timed_out: vec![],
            }
        );
        assert_eq!(
            std::fs::read_to_string(&calls).unwrap(),
            "compose ls --all --format json\ncompose -p inside down --volumes --remove-orphans\n"
        );
    }

    #[tokio::test]
    async fn a_hung_down_is_killed_and_names_its_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let ws = root.join("ws");
        let env = fake_docker(
            root,
            &format!(
                "case \"$*\" in\n  \"compose ls --all --format json\") echo '{}' ;;\n  *down*) echo $$ > {}/down.pid; sleep 1000 & wait ;;\nesac",
                listing_json(&ws),
                root.display()
            ),
        );
        let started = std::time::Instant::now();
        let outcome = compose_down_inside(&ws, &env, QUICK).await;
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(
            outcome,
            ComposeDown {
                stopped: vec![],
                timed_out: vec![DockerTimeout::Down {
                    project: "inside".into(),
                    after: QUICK.down,
                }],
            }
        );
        assert!(
            outcome.timed_out[0]
                .to_string()
                .contains("compose project inside")
        );
        assert!(gone(read_pid(&root.join("down.pid"))).await);
    }

    #[tokio::test]
    async fn no_docker_is_no_project() {
        let dir = tempfile::tempdir().unwrap();
        let env = ShellEnv::new();
        env.set_vars(BTreeMap::from([(
            "PATH".to_string(),
            dir.path().display().to_string(),
        )]));
        assert_eq!(
            compose_down_inside(dir.path(), &env, QUICK).await,
            ComposeDown::default()
        );
    }

    #[test]
    fn a_left_alone_workspace_s_nx_daemons_are_stopped() {
        let procs = [
            proc(10, 1, "/ws/.worktrees/abc-1/shop", NX),
            // Its plugin worker, and an idle terminal shell.
            proc(11, 10, "/ws/.worktrees/abc-1/shop", "node plugin-worker.js"),
            proc(20, 5, "/ws/shop", "-zsh"),
            // Another workspace's dev server.
            proc(30, 1, "/other/shop", "node vite.js"),
        ];
        assert_eq!(idle_nx_daemons(&procs, Path::new("/ws")), [10]);
    }

    #[test]
    fn nx_daemons_stay_while_something_else_runs_in_the_workspace() {
        let dev_server = [
            proc(10, 1, "/ws/shop", NX),
            proc(30, 1, "/ws/shop", "node vite.js --mode dev"),
        ];
        assert!(idle_nx_daemons(&dev_server, Path::new("/ws")).is_empty());
        // A shell with a command running in it (`nx run-many` in a terminal).
        let terminal = [
            proc(10, 1, "/ws/shop", NX),
            proc(20, 5, "/ws/shop", "/bin/zsh -l"),
            proc(21, 20, "/ws/shop", "node nx run-many -t test"),
        ];
        assert!(idle_nx_daemons(&terminal, Path::new("/ws")).is_empty());
        // A shell running a script is not an idle shell.
        let script = [
            proc(10, 1, "/ws/shop", NX),
            proc(20, 5, "/ws/shop", "bash ./qa.sh"),
        ];
        assert!(idle_nx_daemons(&script, Path::new("/ws")).is_empty());
    }
}
