//! On-demand workers belong to the daemon, not to a transient UI request. Durable workspace
//! reservations protect partial handoffs across restarts; a completed return releases one. Once
//! none is left, the worker is stopped with its disk, which keeps its agents' logins, and the next
//! transfer to Cloud starts it again. The same admission lock excludes stopping and deletion while
//! a transfer chooses and reserves its destination.
use std::sync::Arc;
use std::time::Duration;

use chauffeur_protocol::cloud_transfer::{TransferEndpoint, TransferInfo, TransferPhase};
use chauffeur_protocol::worker::{WorkerCreateParams, WorkerParams, WorkerState};
use rusqlite::OptionalExtension;
use tokio::sync::OwnedMutexGuard;

use crate::{Daemon, Error, Result};

const OWNED: &str = "automatic-worker:";
const USE: &str = "automatic-worker-use:";
const DELETE: &str = "automatic-worker-delete:";

fn owned(daemon: &Daemon, worker: &str) -> Result<bool> {
    Ok(daemon.store.kv_get(&format!("{OWNED}{worker}"))?.is_some())
}

fn uses(daemon: &Daemon, worker: &str) -> Result<Vec<(String, String)>> {
    let prefix = format!("{USE}{worker}:");
    Ok(daemon
        .store
        .with(|c| {
            c.prepare("SELECT key,value FROM kv WHERE substr(key,1,length(?1))=?1")?
                .query_map([&prefix], |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })?
        .into_iter()
        .map(|(key, value)| (key[prefix.len()..].to_owned(), value))
        .collect())
}

fn use_key(worker: &str, workspace: &str) -> String {
    format!("{USE}{worker}:{workspace}")
}

fn reserve(daemon: &Daemon, worker: &str, workspace: &str, transfer: &str) -> Result<()> {
    let key = use_key(worker, workspace);
    // An earlier uncertain handoff keeps its reservation until a proven return/cancellation;
    // a later admission failing before it starts cannot erase that protection.
    if daemon.store.kv_get(&key)?.is_none() {
        daemon.store.kv_set(&key, transfer)?;
    }
    Ok(())
}

pub(crate) fn allow_use(daemon: &Daemon, worker: &str) -> Result<()> {
    if daemon.store.kv_get(&format!("{DELETE}{worker}"))?.is_some()
        || crate::workers::deleting(daemon, worker)?
    {
        return Err(Error::Conflict(format!(
            "{worker} is being stopped or deleted now that no workspace uses it; send to Cloud again in a moment"
        )));
    }
    Ok(())
}

pub(crate) fn allow_delete(daemon: &Daemon, worker: &str) -> Result<()> {
    if let Some((workspace, _)) = uses(daemon, worker)?.first() {
        return Err(Error::Conflict(format!(
            "{workspace} still reserves {worker}; finish or cancel its transfer before deleting the worker"
        )));
    }
    Ok(())
}

fn worker(daemon: &Daemon, endpoint: &TransferEndpoint) -> Result<Option<String>> {
    let socket = match endpoint {
        TransferEndpoint::Machine { name } if owned(daemon, name)? => {
            return Ok(Some(name.clone()));
        }
        TransferEndpoint::Machine { name } => daemon
            .settings
            .get()
            .transfer
            .machines
            .iter()
            .find(|m| m.name == *name)
            .map(|m| crate::settings::expand_home(&m.socket)),
        TransferEndpoint::Socket { path } => Some(crate::settings::expand_home(path)),
        _ => None,
    };
    let Some(socket) = socket else {
        return Ok(None);
    };
    for record in crate::workers::list(daemon)?.workers {
        if owned(daemon, &record.name)?
            && crate::workers::socket_path(daemon, &record.name) == socket
        {
            return Ok(Some(record.name));
        }
    }
    Ok(None)
}

/// Called under the admission lock before a transfer is exposed as running.
pub(crate) fn admit(
    daemon: &Daemon,
    endpoint: &TransferEndpoint,
    workspace: &str,
    transfer: &str,
) -> Result<()> {
    if let Some(name) = worker(daemon, endpoint)? {
        allow_use(daemon, &name)?;
        reserve(daemon, &name, workspace, transfer)?;
    }
    Ok(())
}

pub(crate) fn reserved(daemon: &Daemon, workspace: &str) -> Result<Option<String>> {
    let prefix = USE;
    let keys: Vec<String> = daemon.store.with(|c| {
        c.prepare("SELECT key FROM kv WHERE substr(key,1,length(?1))=?1")?
            .query_map([prefix], |r| r.get(0))?
            .collect()
    })?;
    Ok(keys.into_iter().find_map(|key| {
        key.strip_prefix(USE)?
            .strip_suffix(&format!(":{workspace}"))
            .map(str::to_owned)
    }))
}

/// A Cloud reservation whose original attempt never entered the native handoff can be cancelled
/// even when this workspace has older completed handoffs in its history.
pub(crate) fn preparation_only(daemon: &Daemon, name: &str, workspace: &str) -> Result<bool> {
    let Some(id) = daemon.store.kv_get(&use_key(name, workspace))? else {
        return Ok(false);
    };
    let raw: Option<String> = daemon.store.with(|c| {
        c.query_row(
            "SELECT info FROM workspace_transfers WHERE id=?1 AND workspace_id=?2",
            rusqlite::params![id, workspace],
            |r| r.get(0),
        )
        .optional()
    })?;
    let info = raw.and_then(|raw| serde_json::from_str::<TransferInfo>(&raw).ok());
    Ok(info.is_some_and(|info| matches!(info.to, Some(TransferEndpoint::Cloud { .. }))))
}

/// The caller records the chosen name while the pool is locked; the lease precedes creation,
/// so even a crash between GCP and the local record cannot lose the workspace's intent.
pub(crate) async fn destination(
    daemon: &Arc<Daemon>,
    workspace: &str,
    transfer: &str,
    beyond_limit: bool,
    mut chosen: impl FnMut(&str),
) -> Result<(String, OwnedMutexGuard<()>)> {
    let held = daemon.workers.lifecycle.clone().lock_owned().await;
    let all = crate::workers::list(daemon)?.workers;
    let mut name = reserved(daemon, workspace)?;
    if name.is_none() {
        for candidate in &all {
            if matches!(
                candidate.state,
                WorkerState::Ready
                    | WorkerState::Creating
                    | WorkerState::Installing
                    | WorkerState::Stopped
            ) && owned(daemon, &candidate.name)?
                && allow_use(daemon, &candidate.name).is_ok()
            {
                name = Some(candidate.name.clone());
                break;
            }
        }
    }
    let name = name
        .unwrap_or_else(|| format!("cloud-{}", &uuid::Uuid::new_v4().simple().to_string()[..10]));
    allow_use(daemon, &name)?;
    daemon.store.kv_set(&format!("{OWNED}{name}"), "1")?;
    reserve(daemon, &name, workspace, transfer)?;
    chosen(&name);
    if !all.iter().any(|w| {
        w.name == name
            && matches!(
                w.state,
                WorkerState::Ready | WorkerState::Creating | WorkerState::Installing
            )
    }) {
        crate::workers::create(
            daemon,
            WorkerCreateParams {
                name: name.clone(),
                machine_type: None,
                beyond_limit,
            },
        )
        .await?;
    }
    drop(held);
    let started = tokio::time::Instant::now();
    let deadline = started + Duration::from_secs(15 * 60);
    loop {
        let issue = crate::workers::forward_issue(daemon, &name);
        if tokio::time::Instant::now() >= deadline {
            let why = issue.map_or_else(
                || "its forwarded socket never answered".to_owned(),
                |issue| issue.message,
            );
            return Err(Error::Unavailable(format!(
                "{name} did not become reachable in 15 minutes: {why}; retry the transfer or cancel it"
            )));
        }
        let record = crate::workers::list(daemon)?
            .workers
            .into_iter()
            .find(|w| w.name == name)
            .ok_or_else(|| {
                Error::Conflict(format!(
                    "{name} disappeared during preparation; send to Cloud again"
                ))
            })?;
        if record.state == WorkerState::Ready && record.connected {
            let held = daemon.workers.lifecycle.clone().lock_owned().await;
            allow_use(daemon, &name)?;
            return Ok((name, held));
        }
        // Waiting changes nothing until the user acts: say so now, not after 15 minutes.
        if record.state == WorkerState::Ready
            && let Some(issue) = issue.filter(|issue| issue.lasting && issue.at >= started)
        {
            return Err(Error::Unavailable(format!(
                "Could not reach {name}: {}",
                issue.message
            )));
        }
        if record.state == WorkerState::Failed {
            return Err(Error::Unavailable(format!(
                "Could not prepare {name}: {}; retry the transfer or cancel its preparation",
                record.error.unwrap_or_default()
            )));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Releases only after a committed return or confirmed cancellation. A new admission that failed
/// before touching the journal can release its own lease, never one held by an earlier attempt.
pub(crate) fn finished(daemon: &Daemon, info: &TransferInfo) -> Result<()> {
    if info.phase == TransferPhase::Done
        && info.returned
        && info.to == Some(TransferEndpoint::Here)
        && info.discard_pending.is_none()
    {
        if let Some(end) = &info.from
            && let Some(name) = worker(daemon, end)?
        {
            daemon
                .store
                .kv_delete(&use_key(&name, &info.workspace_id))?;
        }
    } else if matches!(info.phase, TransferPhase::Cancelled | TransferPhase::Failed)
        && info.journal_owner.is_none()
    {
        if matches!(info.to, Some(TransferEndpoint::Cloud { .. })) {
            // A failed preparation is retryable. A cancelled one releases only this attempt's
            // lease, never the protection retained by an earlier uncertain handoff.
            if info.phase == TransferPhase::Cancelled
                && let Some(name) = reserved(daemon, &info.workspace_id)?
                && daemon
                    .store
                    .kv_get(&use_key(&name, &info.workspace_id))?
                    .as_deref()
                    == Some(&info.id)
            {
                daemon
                    .store
                    .kv_delete(&use_key(&name, &info.workspace_id))?;
                request_cleanup(daemon, &name)?;
            }
        } else if let Some(end) = &info.to
            && let Some(name) = worker(daemon, end)?
        {
            let key = use_key(&name, &info.workspace_id);
            if daemon.store.kv_get(&key)?.as_deref() == Some(&info.id)
                || (info.phase == TransferPhase::Cancelled
                    && preparation_only(daemon, &name, &info.workspace_id)?)
            {
                daemon.store.kv_delete(&key)?;
            }
        }
    } else if info.phase == TransferPhase::Cancelled
        && let Some(end) = &info.to
        && let Some(name) = worker(daemon, end)?
    {
        daemon
            .store
            .kv_delete(&use_key(&name, &info.workspace_id))?;
    }
    Ok(())
}

/// An explicit cancellation of a preparation can also clean a failed partial installation: the
/// cleanup stops a worker installed once and deletes one never installed.
pub(crate) fn request_cleanup(daemon: &Daemon, name: &str) -> Result<()> {
    if owned(daemon, name)? && uses(daemon, name)?.is_empty() {
        match crate::workers::list(daemon)?
            .workers
            .iter()
            .find(|w| w.name == name)
        {
            // Left stopped, ready for the next transfer to Cloud.
            Some(worker) if worker.state == WorkerState::Stopped => {}
            Some(_) => daemon.store.kv_set(&format!("{DELETE}{name}"), "1")?,
            None => forget(daemon, name),
        }
    }
    Ok(())
}

pub(crate) fn forget(daemon: &Daemon, name: &str) {
    for prefix in [OWNED, DELETE] {
        if let Err(error) = daemon.store.kv_delete(&format!("{prefix}{name}")) {
            tracing::warn!(%error, worker=name, "could not forget an automatic worker");
        }
    }
}

/// Safe to repeat after a stop or deletion failure or a restart. A worker installed once is
/// stopped, one never installed deleted. Recovery's retained disks are excluded by workers::delete;
/// a manual worker never acquires this ownership marker.
pub async fn cleanup(daemon: &Arc<Daemon>) -> Result<()> {
    let _held = daemon.workers.lifecycle.lock().await;
    if crate::cloud_transfer_driver::running(daemon)
        .iter()
        .any(|t| matches!(t.to, Some(TransferEndpoint::Cloud { .. })))
    {
        return Ok(());
    }
    for record in crate::workers::list(daemon)?.workers {
        // A crash after saving a completed return but before releasing its lease is harmless:
        // replay that durable outcome before deciding whether this worker is still in use.
        for (workspace, _) in uses(daemon, &record.name)? {
            if let Some(info) = crate::cloud_transfer_driver::status(daemon, &workspace)? {
                finished(daemon, &info)?;
            }
        }
        if !owned(daemon, &record.name)? || !uses(daemon, &record.name)?.is_empty() {
            continue;
        }
        if daemon.workers.busy.lock().unwrap().contains(&record.name) {
            continue;
        }
        let key = format!("{DELETE}{}", record.name);
        let previous = daemon.store.kv_get(&key)?.is_some();
        let installed = crate::workers::installed(daemon, &record.name)?;
        match record.state {
            WorkerState::Ready => {}
            // A failed preparation is retried by the next transfer, unless it was cancelled.
            WorkerState::Failed if previous => {}
            // A cancelled preparation of a stopped worker leaves it as it was.
            WorkerState::Stopped if previous => {
                daemon.store.kv_delete(&key)?;
                continue;
            }
            _ => continue,
        }
        if installed {
            // Held under the admission lock: a transfer to Cloud waits, then starts it again.
            if let Err(error) = crate::workers::stop(daemon, &record.name).await {
                tracing::warn!(
                    error = format!("{error:#}"),
                    worker = record.name,
                    "could not stop an unused automatic worker"
                );
            } else if previous {
                daemon.store.kv_delete(&key)?;
            }
            continue;
        }
        // The admission lock stays held through the existing deletion guards and durable mark.
        daemon.store.kv_set(&key, "1")?;
        if let Err(error) = crate::workers::delete(
            daemon,
            WorkerParams {
                name: record.name.clone(),
            },
        ) {
            if !previous {
                daemon.store.kv_delete(&key)?;
            }
            tracing::warn!(%error, worker=record.name, "could not delete an unused automatic worker");
        }
    }
    Ok(())
}

pub fn spawn(daemon: Arc<Daemon>) {
    tokio::spawn(async move {
        loop {
            if let Err(error) = cleanup(&daemon).await {
                tracing::warn!(%error, "could not check unused automatic workers");
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;

    fn fixture() -> (tempfile::TempDir, Arc<Daemon>) {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::new(
            temp.path().join("home"),
            temp.path().join("workspaces"),
            temp.path().join("repos"),
        );
        paths.ensure().unwrap();
        std::fs::write(
            &paths.settings,
            "linear_enabled = false\ngithub_org = \"\"\ngithub_sources = false\n[office]\nurl = \"\"\n",
        )
        .unwrap();
        let daemon = Daemon::open(paths).unwrap();
        (temp, daemon)
    }

    fn transfer(phase: TransferPhase) -> TransferInfo {
        TransferInfo {
            revision: None,
            id: "t1".into(),
            workspace_id: "w1".into(),
            slug: "moving".into(),
            source: "cloud-one".into(),
            destination: "mac".into(),
            phase,
            log: vec![],
            error: None,
            workspace_path: None,
            kept_copy: None,
            returned: true,
            started_at: "2026-10-27T00:00:00Z".into(),
            finished_at: None,
            from: Some(TransferEndpoint::Machine {
                name: "cloud-one".into(),
            }),
            to: Some(TransferEndpoint::Here),
            keep_local_copy: false,
            journal_owner: None,
            discard_pending: None,
            stage: None,
            working_agents: 0,
            progress: None,
            preparation: None,
            interrupt: false,
            archive: false,
            needs_login: None,
            local_copy_changed: false,
            untouched: false,
        }
    }

    fn own(daemon: &Daemon) {
        daemon
            .store
            .kv_set(&format!("{OWNED}cloud-one"), "1")
            .unwrap();
        reserve(daemon, "cloud-one", "w1", "t1").unwrap();
    }

    #[test]
    fn a_return_releases_only_its_workspace_after_the_remote_copy_is_discarded() {
        let (_temp, daemon) = fixture();
        own(&daemon);
        reserve(&daemon, "cloud-one", "w2", "t2").unwrap();
        let mut info = transfer(TransferPhase::Failed);
        finished(&daemon, &info).unwrap();
        assert_eq!(uses(&daemon, "cloud-one").unwrap().len(), 2);
        info.phase = TransferPhase::Done;
        info.discard_pending = Some("remote disk busy".into());
        finished(&daemon, &info).unwrap();
        assert!(allow_delete(&daemon, "cloud-one").is_err());
        info.discard_pending = None;
        finished(&daemon, &info).unwrap();
        assert_eq!(
            uses(&daemon, "cloud-one").unwrap(),
            vec![("w2".into(), "t2".into())]
        );
    }

    #[test]
    fn a_failed_retry_does_not_release_an_earlier_uncertain_handoff() {
        let (_temp, daemon) = fixture();
        own(&daemon);
        reserve(&daemon, "cloud-one", "w1", "retry").unwrap();
        let mut info = transfer(TransferPhase::Failed);
        info.id = "retry".into();
        info.to = info.from.take();
        finished(&daemon, &info).unwrap();
        assert!(allow_delete(&daemon, "cloud-one").is_err());
        info.phase = TransferPhase::Cancelled;
        finished(&daemon, &info).unwrap();
        assert!(allow_delete(&daemon, "cloud-one").is_err());
        info.journal_owner = Some("confirmed cancellation".into());
        finished(&daemon, &info).unwrap();
        assert!(allow_delete(&daemon, "cloud-one").is_ok());
    }

    #[test]
    fn cancelling_preparation_without_a_vm_forgets_its_ownership() {
        let (_temp, daemon) = fixture();
        own(&daemon);
        let mut info = transfer(TransferPhase::Cancelled);
        info.to = Some(TransferEndpoint::Cloud {
            beyond_limit: false,
        });
        finished(&daemon, &info).unwrap();
        assert!(uses(&daemon, "cloud-one").unwrap().is_empty());
        assert!(!owned(&daemon, "cloud-one").unwrap());
        daemon
            .store
            .kv_set(&format!("{DELETE}cloud-one"), "1")
            .unwrap();
        assert!(allow_use(&daemon, "cloud-one").is_err());
        forget(&daemon, "cloud-one");
    }

    #[test]
    fn cancelling_a_queued_preparation_cannot_release_an_earlier_reservation() {
        let (_temp, daemon) = fixture();
        own(&daemon);
        let mut info = transfer(TransferPhase::Cancelled);
        info.id = "retry".into();
        info.to = Some(TransferEndpoint::Cloud {
            beyond_limit: false,
        });
        finished(&daemon, &info).unwrap();
        assert!(allow_delete(&daemon, "cloud-one").is_err());
        assert!(allow_use(&daemon, "cloud-one").is_ok());
    }

    #[test]
    fn reservations_and_deletion_intent_survive_reopening_the_store() {
        let (_temp, daemon) = fixture();
        own(&daemon);
        let paths = daemon.paths.clone();
        daemon
            .store
            .kv_set(&format!("{DELETE}cloud-one"), "1")
            .unwrap();
        drop(daemon);
        let daemon = Daemon::open(paths).unwrap();
        assert_eq!(
            reserved(&daemon, "w1").unwrap().as_deref(),
            Some("cloud-one")
        );
        assert!(allow_delete(&daemon, "cloud-one").is_err());
        assert!(allow_use(&daemon, "cloud-one").is_err());
    }

    #[tokio::test]
    async fn cleanup_waits_for_an_admission_to_record_its_reservation() {
        let (_temp, daemon) = fixture();
        let held = daemon.workers.lifecycle.lock().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(20), cleanup(&daemon))
                .await
                .is_err()
        );
        drop(held);
        cleanup(&daemon).await.unwrap();
        assert!(!owned(&daemon, "manual").unwrap());
    }
    #[test]
    fn preparation_cancellation_depends_on_its_own_attempt_not_older_completed_handoffs() {
        let (_temp, daemon) = fixture();
        own(&daemon);
        let mut info = transfer(TransferPhase::Failed);
        info.to = Some(TransferEndpoint::Cloud {
            beyond_limit: false,
        });
        let save = |info: &TransferInfo| {
            daemon.store.with(|c| c.execute("INSERT OR REPLACE INTO workspace_transfers(id,workspace_id,slug,started_at,info) VALUES(?1,?2,?3,?4,?5)", rusqlite::params![info.id, info.workspace_id, info.slug, info.started_at, serde_json::to_string(info).unwrap()]))
        };
        save(&info).unwrap();
        assert!(preparation_only(&daemon, "cloud-one", "w1").unwrap());
        info.to = Some(TransferEndpoint::Machine {
            name: "cloud-one".into(),
        });
        save(&info).unwrap();
        assert!(!preparation_only(&daemon, "cloud-one", "w1").unwrap());
    }
}
