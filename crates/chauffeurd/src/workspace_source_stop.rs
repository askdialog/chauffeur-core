//! Coordinate source stops under the persistent admission fence: registered launchers, then the
//! workspace's remaining operating-system processes, then its services and databases, then a
//! check that no container or program still writes there. Only `stop_source` composes them into
//! the receipt whose digest the journal's `Stopped` state carries. Trusted callers authenticate
//! the exact controller/source operation first.
use crate::{
    Daemon, Error, Result,
    cloud_workspace_handoff::State,
    workspace_processes::{self as processes, ObservedProcess, Owner},
    workspace_service_stop::{DockerServices, ServiceStop},
};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc, time::Duration};

#[derive(Clone, Copy)]
pub struct StopTimeouts {
    pub admissions: Duration,
    pub script_grace: Duration,
    pub terminal_exit: Duration,
}
impl Default for StopTimeouts {
    fn default() -> Self {
        Self {
            admissions: Duration::from_secs(30),
            script_grace: Duration::from_secs(5),
            terminal_exit: Duration::from_secs(7),
        }
    }
}

/// A successful result confirms only the registered stop calls and launch-root observations.
/// It neither advances the controller journal nor authorizes snapshot, ownership or execution.
/// Dropping the caller does not cancel an already-started host task. Any failed/uncertain stop
/// leaves its durable marker pending and prevents cancellation from reopening local execution.
pub async fn stop_registered(
    daemon: Arc<Daemon>,
    stopping: State,
    timeouts: StopTimeouts,
) -> Result<()> {
    daemon.execution.install(&stopping)?;
    tokio::spawn(async move {
        tokio::time::timeout(timeouts.admissions, daemon.execution.wait_for_admissions(&stopping)).await
            .map_err(|_| Error::Unavailable("Source admissions are still in flight; keep the workspace fenced".into()))??;
        let attempt = daemon.execution.begin_stop_attempt(&stopping)?;
        let workspace = &stopping.binding.workspace_id;
        // Attempt every category even if another one fails. A dead old root may be settled
        // read-only first; missing registrations remain pending and cannot manufacture exit.
        let before = crate::workspace_launches::reconcile_roots(&daemon.store, workspace);
        let (agents, scripts, terminals) = tokio::join!(
            daemon.chats.stop_workspace_and_wait(workspace),
            daemon.procs.stop_workspace_and_wait(workspace, timeouts.script_grace),
            daemon.terminals.close_workspace_and_wait(workspace, timeouts.terminal_exit),
        );
        let mut uncertain = Vec::new();
        if agents.is_err() { uncertain.push("native sessions"); }
        if scripts.is_err() { uncertain.push("scripts"); }
        if terminals.is_err() { uncertain.push("terminals"); }
        if before.is_err() || !crate::workspace_launches::reconcile_roots(&daemon.store, workspace)?.is_empty() { uncertain.push("launch evidence"); }
        if !uncertain.is_empty() {
            return Err(Error::Unavailable(format!("Registered source stop is incomplete ({}); retain the workspace fence and retry reconciliation", uncertain.join(", "))));
        }
        attempt.finish()
    }).await.map_err(|error| Error::Internal(error.into()))?
}

#[derive(Clone, Copy)]
pub struct SweepTimeouts {
    /// Between SIGTERM and SIGKILL.
    pub grace: Duration,
    /// For the kernel to end a killed process.
    pub kill: Duration,
}
impl Default for SweepTimeouts {
    fn default() -> Self {
        Self {
            grace: Duration::from_secs(5),
            kill: Duration::from_secs(2),
        }
    }
}

/// The workspace's processes the sweep ended, after its registered stop.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessSweep {
    pub version: u8,
    pub source: State,
    pub roots: Vec<String>,
    /// Ended within the grace after SIGTERM.
    pub terminated: Vec<ObservedProcess>,
    /// Needed SIGKILL.
    pub killed: Vec<ObservedProcess>,
}

/// More rounds than this of newly started processes is a process that keeps forking.
const SWEEP_ROUNDS: usize = 16;

/// The folders a source's processes may work or write in: the workspace folder (worktrees and
/// notes), worktrees outside it, and the private folder of a restored workspace (its clones).
pub(crate) fn workspace_roots(daemon: &Daemon, workspace: &str) -> Result<Vec<PathBuf>> {
    let row = daemon.workspaces.row(workspace)?;
    let mut roots = vec![PathBuf::from(&row.path).canonicalize()?];
    for worktree in daemon.store.worktrees_of(workspace)? {
        if !worktree.removed {
            let path = PathBuf::from(&worktree.path);
            roots.push(path.canonicalize().unwrap_or(path));
        }
    }
    let native: Option<String> = daemon.store.with(|c| {
        use rusqlite::OptionalExtension;
        c.query_row(
            "SELECT native_root FROM workspace_restores WHERE workspace_id=?1",
            [workspace],
            |r| r.get(0),
        )
        .optional()
    })?;
    if let Some(native) = native {
        let path = PathBuf::from(native);
        roots.push(path.canonicalize().unwrap_or(path));
    }
    roots.sort();
    roots.dedup();
    Ok(roots)
}

/// Which processes a sweep scans: one workspace's, told apart by the daemon's owner.
#[derive(Clone)]
struct Scope {
    workspace: String,
    owner: Owner,
    roots: Vec<PathBuf>,
}

async fn scan(scope: &Scope) -> Result<processes::Inventory> {
    let scope = scope.clone();
    tokio::task::spawn_blocking(move || {
        processes::scan(&scope.workspace, &scope.owner, &scope.roots)
    })
    .await
    .map_err(|error| Error::Internal(error.into()))?
}

fn named(list: &[ObservedProcess]) -> String {
    list.iter()
        .take(8)
        .map(|p| format!("{} (pid {})", p.name, p.pid))
        .collect::<Vec<_>>()
        .join(", ")
}

/// After the registered stop settles, ends every process still the workspace's (see
/// `workspace_processes`): detached descendants of agents, scripts and terminals, and launches a
/// previous daemon lost. They are frozen until no new one appears, then sent SIGTERM, then
/// SIGKILL past the grace. A program the workspace did not start that works or writes in it is
/// never signalled: the sweep refuses, naming it, before any signal. Every return settles the
/// attempt, as no signal outlives the task; a failed sweep leaves the fence installed and can be
/// retried.
pub async fn sweep_processes(
    daemon: Arc<Daemon>,
    stopping: State,
    timeouts: SweepTimeouts,
) -> Result<ProcessSweep> {
    sweep(daemon, stopping, timeouts, Externals::Refuse).await
}

/// When other programs writing in the workspace refuse a sweep.
#[derive(Clone, Copy, PartialEq)]
enum Externals {
    /// Before any signal.
    Refuse,
    /// Not here: a container engine's file server writes in the workspace for its bind mounts
    /// until the services stop, so the caller checks again after them.
    AfterServices,
}

async fn sweep(
    daemon: Arc<Daemon>,
    stopping: State,
    timeouts: SweepTimeouts,
    externals: Externals,
) -> Result<ProcessSweep> {
    let scope = Scope {
        workspace: stopping.binding.workspace_id.clone(),
        owner: daemon_owner(&daemon)?,
        roots: workspace_roots(&daemon, &stopping.binding.workspace_id)?,
    };
    let attempt = daemon.execution.begin_preparation(&stopping)?;
    tokio::spawn(async move {
        let result = async {
            let first = scan(&scope).await?;
            if externals == Externals::Refuse
                && let Some(refusal) = first.external_refusal()
            {
                return Err(refusal);
            }
            let workspace = &scope.workspace;
            let fence = serde_json::to_string(&stopping)?;
            let plan = serde_json::to_string(&serde_json::json!({"version": 1, "source": stopping, "roots": scope.roots}))?;
            daemon.store.durable(|c| {
                let changed = c.execute(
                    &format!("INSERT INTO workspace_process_sweeps(attempt_id,workspace_id,plan) SELECT ?1,?2,?3 WHERE {CURRENT}"),
                    rusqlite::params![attempt.id(), workspace, plan, fence],
                )?;
                if changed != 1 { return Err(changed_stop()); }
                Ok(())
            })?;
            let (terminated, killed) = end_own(&scope, timeouts, externals).await?;
            let sweep = ProcessSweep {
                version: 1,
                source: stopping.clone(),
                roots: scope.roots.iter().map(|r| r.to_string_lossy().into_owned()).collect(),
                terminated,
                killed,
            };
            let receipt = serde_json::to_string(&sweep)?;
            daemon.store.durable(|c| {
                let changed = c.execute(
                    &format!("UPDATE workspace_process_sweeps SET receipt=?3 WHERE attempt_id=?1 AND workspace_id=?2 AND receipt IS NULL AND {CURRENT}"),
                    rusqlite::params![attempt.id(), workspace, receipt, fence],
                )?;
                if changed != 1 { return Err(changed_stop()); }
                Ok(())
            })?;
            Ok(sweep)
        }
        .await;
        attempt.finish()?;
        result
    })
    .await
    .map_err(|error| Error::Internal(error.into()))?
}

/// The attempt is still this fence's pending preparation.
const CURRENT: &str = "EXISTS(SELECT 1 FROM workspace_preparation_attempts p JOIN workspaces w ON w.id=p.workspace_id WHERE p.attempt_id=?1 AND p.workspace_id=?2 AND p.fence=?4 AND p.settled=0 AND w.execution_fence=?4)";

fn changed_stop() -> Error {
    Error::Conflict("The source stop changed; retry it".into())
}

fn daemon_owner(daemon: &Daemon) -> Result<Owner> {
    daemon.process_owner.lock().unwrap().clone().ok_or_else(|| {
        Error::Unavailable("Chauffeur cannot tell a workspace's processes from other programs' on this system; transfers need macOS or Linux".into())
    })
}

/// Freezes the workspace's processes to a fixed point, so none restarts another while the others
/// end, then terminates them. Returns those that ended on SIGTERM and those that needed SIGKILL.
async fn end_own(
    scope: &Scope,
    timeouts: SweepTimeouts,
    externals: Externals,
) -> Result<(Vec<ObservedProcess>, Vec<ObservedProcess>)> {
    let mut frozen: Vec<ObservedProcess> = Vec::new();
    let mut outcome = Err(Error::Unavailable("The workspace's processes kept starting new ones while being stopped; retry the source stop".into()));
    for _ in 0..SWEEP_ROUNDS {
        let fresh: Vec<ObservedProcess> = match scan(scope).await {
            Ok(inventory) => inventory
                .own
                .into_iter()
                .filter(|p| !frozen.contains(p))
                .collect(),
            Err(error) => {
                outcome = Err(error);
                break;
            }
        };
        if fresh.is_empty() {
            outcome = Ok(());
            break;
        }
        for process in fresh {
            if processes::signal(&process, libc::SIGSTOP).unwrap_or(false) {
                frozen.push(process);
            }
        }
    }
    // Whatever happened above, nothing stays frozen: every frozen process is asked to end.
    for process in &frozen {
        let _ = processes::signal(process, libc::SIGTERM);
        let _ = processes::signal(process, libc::SIGCONT);
    }
    let survivors = processes::wait_gone(&frozen, timeouts.grace).await;
    let terminated: Vec<ObservedProcess> = frozen
        .iter()
        .filter(|p| !survivors.contains(p))
        .cloned()
        .collect();
    // Survivors are killed even if they no longer touch the workspace, which would hide them
    // from a scan on macOS.
    let mut left = survivors;
    let mut killed: Vec<ObservedProcess> = Vec::new();
    for _ in 0..SWEEP_ROUNDS {
        if left.is_empty() {
            break;
        }
        for process in &left {
            processes::signal(process, libc::SIGKILL)?;
        }
        processes::wait_gone(&left, timeouts.kill).await;
        for process in &left {
            if !killed.contains(process) {
                killed.push(process.clone());
            }
        }
        left = scan(scope).await?.own;
    }
    outcome?;
    let last = scan(scope).await?;
    if !last.own.is_empty() {
        return Err(Error::Unavailable(format!(
            "Processes of the workspace still run after SIGKILL: {}; the workspace stays fenced, retry the source stop",
            named(&last.own)
        )));
    }
    if externals == Externals::Refuse
        && let Some(refusal) = last.external_refusal()
    {
        return Err(refusal);
    }
    Ok((terminated, killed))
}

/// Drain registered launchers before stopping the explicitly selected Docker services. This
/// finite observation is not overall source quiescence or a controller stop receipt. A lost
/// observer leaves host work running; uncertainty retains its plan and preparation barrier.
pub async fn stop_registered_and_services(
    daemon: Arc<Daemon>,
    stopping: State,
    timeouts: StopTimeouts,
    services: DockerServices,
) -> Result<ServiceStop> {
    if !services.matches(&stopping) {
        return Err(service_uncertain());
    }
    services.require_current_profile(&daemon.store)?;
    tokio::spawn(async move {
        stop_registered(daemon.clone(), stopping.clone(), timeouts).await?;
        // A detached process of the workspace could start a container again behind the check.
        sweep(
            daemon.clone(),
            stopping.clone(),
            SweepTimeouts::default(),
            Externals::AfterServices,
        )
        .await?;
        let stopped = stop_services(&daemon, &stopping, services).await?;
        // With the containers stopped, nothing but another program can still write there.
        let scope = Scope {
            workspace: stopping.binding.workspace_id.clone(),
            owner: daemon_owner(&daemon)?,
            roots: workspace_roots(&daemon, &stopping.binding.workspace_id)?,
        };
        if let Some(refusal) = scan(&scope).await?.external_refusal() {
            return Err(refusal);
        }
        Ok(stopped)
    })
    .await
    .map_err(|error| Error::Internal(error.into()))?
}

/// Stops the selected services under the preparation barrier, after the registered stop and the
/// sweep. Uncertainty retains the plan and leaves the attempt pending.
async fn stop_services(
    daemon: &Arc<Daemon>,
    stopping: &State,
    services: DockerServices,
) -> Result<ServiceStop> {
    // Profile publication holds ordinary admission through its durable write. Draining
    // pre-fence admissions makes this final head check stable until preparation settles.
    services.require_current_profile(&daemon.store)?;
    let attempt = daemon.execution.begin_preparation(stopping)?;
    let plan = services.evidence()?;
    if plan.is_empty() || plan.len() > 1024 * 1024 {
        return Err(service_uncertain());
    }
    let workspace = &stopping.binding.workspace_id;
    let fence = serde_json::to_string(stopping)?;
    daemon.store.durable(|c| {
        let changed = c.execute(
        "INSERT INTO workspace_service_stops(attempt_id,workspace_id,plan) SELECT ?1,?2,?3
         WHERE EXISTS(SELECT 1 FROM workspace_preparation_attempts p JOIN workspaces w ON w.id=p.workspace_id
         WHERE p.attempt_id=?1 AND p.workspace_id=?2 AND p.fence=?4 AND p.settled=0 AND w.execution_fence=?4)",
        rusqlite::params![attempt.id(), workspace, plan, fence],
        )?;
        if changed != 1 { return Err(service_uncertain()); }
        Ok(())
    })?;
    let stopped = services.stop().await?;
    daemon.execution.require_settled_stop(stopping)?;
    let receipt = serde_json::to_string(&stopped)?;
    if receipt.len() > 1024 * 1024 {
        return Err(service_uncertain());
    }
    daemon.store.durable(|c| {
        let changed = c.execute(
        "UPDATE workspace_service_stops SET receipt=?3 WHERE attempt_id=?1 AND workspace_id=?2 AND receipt IS NULL
         AND EXISTS(SELECT 1 FROM workspace_preparation_attempts p JOIN workspaces w ON w.id=p.workspace_id
         WHERE p.attempt_id=?1 AND p.workspace_id=?2 AND p.fence=?4 AND p.settled=0 AND w.execution_fence=?4)",
        rusqlite::params![attempt.id(), workspace, receipt, fence],
        )?;
        if changed != 1 { return Err(service_uncertain()); }
        Ok(())
    })?;
    attempt.finish()?;
    Ok(stopped)
}

fn service_uncertain() -> Error {
    Error::Unavailable("Selected workspace services remain uncertain; retain their stop plan and workspace fence before explicit reconciliation".into())
}

/// Everything a complete source stop observed. Its canonical JSON's SHA-256 is the journal's
/// `source_stop_digest`; source preparation captures only under a stored receipt of that digest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceStop {
    pub version: u8,
    pub source: State,
    pub profile_head: Option<crate::workspace_development_profile::ProfileHead>,
    pub sweep: ProcessSweep,
    pub services: Option<ServiceStop>,
    pub databases: Vec<crate::workspace_postgres_quiescence::QuietDatabase>,
    pub containers: crate::workspace_containers::ContainerCheck,
}

/// Stops the whole source under the exact `Stopping` fence: registered agents, scripts and
/// terminals; the workspace's remaining processes; its reconstructible services; each PostgreSQL
/// service through its exports and no-write proof (app containers first, so none still writes
/// to a database); then no running container may write in the workspace, and no process of it
/// or other program may run or write there. Returns the stored receipt and its digest. Any step
/// that refuses leaves the fence installed and what it stopped stopped; a retry resumes.
pub async fn stop_source(
    daemon: Arc<Daemon>,
    stopping: State,
    timeouts: StopTimeouts,
) -> Result<(SourceStop, String)> {
    use crate::{
        workspace_containers as containers, workspace_development_profile as profiles,
        workspace_postgres_quiescence as databases,
    };
    tokio::spawn(async move {
        let workspace = stopping.binding.workspace_id.clone();
        // The registered stop installs the fence first: nothing below may run unfenced.
        stop_registered(daemon.clone(), stopping.clone(), timeouts).await?;
        let profile = profiles::load(&daemon.store, &workspace)?;
        let sweep = sweep(daemon.clone(), stopping.clone(), SweepTimeouts::default(), Externals::AfterServices).await?;
        // The container check reads this snapshot of the login environment; services and
        // databases each pin their engine by socket and id.
        let env = crate::shellenv::ShellEnv::new();
        env.set_vars(daemon.shell_env.vars());
        let exported: std::collections::BTreeSet<&str> = profile
            .iter()
            .flat_map(|p| &p.profile.postgres_exports)
            .map(|export| export.service.as_str())
            .collect();
        let mut services = None;
        let mut quiet = Vec::new();
        if let Some(profile) = &profile {
            if profile.profile.services.iter().any(|s| !exported.contains(s.name.as_str())) {
                let selected = DockerServices::from_profile(&daemon.store, &env, stopping.clone(), profile.head.clone()).await?;
                services = Some(stop_services(&daemon, &stopping, selected).await?);
            }
            for name in &exported {
                let proof = match databases::proved_before(&daemon, &stopping, profile, name).await? {
                    Some(proof) => proof,
                    None => databases::export_and_stop(daemon.clone(), stopping.clone(), profile.head.clone(), (*name).to_owned()).await?,
                };
                quiet.push(proof);
            }
        }
        let roots = workspace_roots(&daemon, &workspace)?;
        let checked = containers::check(&env, profile.as_ref(), &roots).await?;
        let scope = Scope { workspace: workspace.clone(), owner: daemon_owner(&daemon)?, roots };
        let last = scan(&scope).await?;
        if !last.own.is_empty() {
            return Err(Error::Unavailable(format!(
                "Processes of the workspace started again during its stop: {}; retry the source stop",
                named(&last.own)
            )));
        }
        if let Some(refusal) = last.external_refusal() {
            return Err(refusal);
        }
        if profiles::load(&daemon.store, &workspace)? != profile {
            return Err(changed_stop());
        }
        daemon.execution.require_settled_stop(&stopping)?;
        let receipt = SourceStop {
            version: 1,
            source: stopping.clone(),
            profile_head: profile.map(|p| p.head),
            sweep,
            services,
            databases: quiet,
            containers: checked,
        };
        let bytes = serde_json::to_vec(&receipt)?;
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        use sha2::{Digest, Sha256};
        let digest = URL_SAFE_NO_PAD.encode(Sha256::digest(&bytes));
        let fence = serde_json::to_string(&stopping)?;
        let text = String::from_utf8(bytes).map_err(|error| Error::Internal(error.into()))?;
        daemon.store.durable(|c| {
            let changed = c.execute(
                "INSERT OR IGNORE INTO workspace_source_stops(digest,workspace_id,fence,receipt) SELECT ?1,?2,?3,?4
                 WHERE EXISTS(SELECT 1 FROM workspaces WHERE id=?2 AND execution_fence=?3)
                 AND NOT EXISTS(SELECT 1 FROM workspace_preparation_attempts WHERE workspace_id=?2 AND settled!=1)",
                rusqlite::params![digest, workspace, fence, text],
            )?;
            let stored: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM workspace_source_stops WHERE digest=?1 AND workspace_id=?2 AND fence=?3 AND receipt=?4)", rusqlite::params![digest, workspace, fence, text], |r| r.get(0))?;
            if changed > 1 || !stored { return Err(changed_stop()); }
            Ok(())
        })?;
        Ok((receipt, digest))
    })
    .await
    .map_err(|error| Error::Internal(error.into()))?
}

/// The latest complete-stop receipt stored under this fence, with its digest. A retried stop
/// supersedes earlier ones: its databases may have been exported again since.
pub(crate) fn latest_receipt(
    store: &crate::store::Store,
    stopping: &State,
) -> Result<Option<(String, SourceStop)>> {
    use rusqlite::OptionalExtension;
    let fence = serde_json::to_string(stopping)?;
    let row: Option<(String, String)> = store.with(|c| {
        c.query_row(
            "SELECT digest,receipt FROM workspace_source_stops WHERE workspace_id=?1 AND fence=?2 ORDER BY rowid DESC LIMIT 1",
            rusqlite::params![stopping.binding.workspace_id, fence],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
    })?;
    row.map(|(digest, raw)| Ok((digest, serde_json::from_str(&raw)?)))
        .transpose()
}

/// The `Stopped` acknowledgement must name the latest receipt stored under this fence.
pub(crate) fn require_receipt(daemon: &Daemon, stopping: &State, digest: &str) -> Result<()> {
    let latest = latest_receipt(&daemon.store, stopping)?;
    if latest.as_ref().map(|(stored, _)| stored.as_str()) != Some(digest) {
        return Err(Error::Conflict("No complete source stop matches this acknowledgement; stop the source with its receipt before preparing it".into()));
    }
    Ok(())
}
