//! The steps of one workspace transfer a local proof coordinator (`chauffeur cloud transfer`)
//! drives over private Unix sockets: the controller's journal, the source's stop, capture,
//! sealing and retirement, the destination's restoration and activation. Each step checks what the
//! module it calls checks; the coordinator only carries journal states, digests and the key.
//! The private socket's OS user is trusted, as for the archive bridge.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chauffeur_protocol::cloud_transfer::{
    CloudTransferActivated, CloudTransferCaptured, CloudTransferCheckParams,
    CloudTransferDiscarded, CloudTransferJournal, CloudTransferJournalParams, CloudTransferPeer,
    CloudTransferReadiness, CloudTransferRestoreParams, CloudTransferRestored, CloudTransferSealed,
    CloudTransferSetup, CloudTransferSetupParams, CloudTransferStep,
};
use chauffeur_protocol::model::{AccountStatus, AgentKind};
use serde_json::Value;

use crate::cloud_workspace_archive::{self as archive, ArchiveKey};
use crate::cloud_workspace_artifact as artifact;
use crate::cloud_workspace_handoff::{self as journal, Action, Binding, State};
use crate::cloud_workspace_preparation::{self as preparation, CapturedSource};
use crate::workspace_source_stop::{StopTimeouts, stop_source};
use crate::{Daemon, Error, Result};

/// Captures waiting for the controller to publish their snapshot, by operation. A daemon that
/// restarts in between loses them; their preparation stays pending for explicit recovery.
#[derive(Default)]
pub struct Captures(Mutex<HashMap<String, CapturedSource>>);

fn state(value: Value) -> Result<State> {
    let state: State = serde_json::from_value(value)
        .map_err(|_| Error::BadRequest("Invalid workspace handoff state".into()))?;
    state.validate()?;
    Ok(state)
}

fn operation(state: &State) -> Result<String> {
    Ok(state
        .transfer
        .as_ref()
        .ok_or_else(|| Error::BadRequest("The handoff state has no transfer".into()))?
        .operation_id
        .clone())
}

/// This daemon's endpoint identity, created on first use.
pub fn peer(daemon: &Daemon) -> Result<CloudTransferPeer> {
    daemon.cloud_vault_keys.initialize(&daemon.cloud_vault)?;
    Ok(CloudTransferPeer {
        peer_id: daemon.cloud_vault_keys.peer_id(&daemon.cloud_vault)?,
    })
}

/// The journal this daemon keeps as the workspace's controller.
pub fn journal(
    daemon: &Daemon,
    params: CloudTransferJournalParams,
) -> Result<CloudTransferJournal> {
    let controller = peer(daemon)?.peer_id;
    let binding = |workspace_id: String, owner_peer_id: String| Binding {
        workspace_id,
        owner_peer_id,
        controller_peer_id: controller.clone(),
    };
    let store = &daemon.cloud_vault;
    let state = match params {
        CloudTransferJournalParams::Read {
            workspace_id,
            owner_peer_id,
        } => journal::read(store, &binding(workspace_id, owner_peer_id))?,
        CloudTransferJournalParams::Register {
            workspace_id,
            owner_peer_id,
        } => Some(journal::initialize(
            store,
            binding(workspace_id, owner_peer_id.clone()),
            owner_peer_id,
        )?),
        CloudTransferJournalParams::Prepare {
            workspace_id,
            owner_peer_id,
            revision,
            operation_id,
            target_peer_id,
        } => journal::prepare(
            store,
            &binding(workspace_id, owner_peer_id),
            revision,
            operation_id,
            target_peer_id,
        )?,
        CloudTransferJournalParams::Advance {
            workspace_id,
            owner_peer_id,
            revision,
            operation_id,
            source_epoch,
            step,
        } => journal::advance(
            store,
            &binding(workspace_id, owner_peer_id),
            revision,
            &operation_id,
            source_epoch,
            match step {
                CloudTransferStep::BeginStop => Action::BeginStop,
                CloudTransferStep::SourceStopped {
                    peer_id,
                    receipt_digest,
                } => Action::SourceStopped {
                    peer_id,
                    receipt_digest,
                },
                CloudTransferStep::SnapshotPublished {
                    peer_id,
                    snapshot_digest,
                } => Action::SnapshotPublished {
                    peer_id,
                    snapshot_digest,
                },
                CloudTransferStep::DestinationRestored {
                    peer_id,
                    snapshot_digest,
                    receipt_digest,
                } => Action::DestinationRestored {
                    peer_id,
                    snapshot_digest,
                    receipt_digest,
                },
                CloudTransferStep::Commit {
                    snapshot_digest,
                    destination_receipt_digest,
                } => Action::Commit {
                    snapshot_digest,
                    destination_receipt_digest,
                },
                CloudTransferStep::Cancel => Action::Cancel,
            },
        )?,
    };
    Ok(CloudTransferJournal {
        state: state.map(serde_json::to_value).transpose()?,
    })
}

/// Stops the whole source under the `Stopping` state and returns its receipt's digest. A folder
/// the capture would refuse (a file added while the agents worked) refuses first, nothing stopped.
pub async fn source_stop(daemon: &Arc<Daemon>, stopping: Value) -> Result<String> {
    let stopping = state(stopping)?;
    crate::cloud_workspace_package::check_folder(daemon, &stopping.binding.workspace_id).await?;
    let (_, digest) = stop_source(daemon.clone(), stopping, StopTimeouts::default()).await?;
    Ok(digest)
}

pub async fn source_capture(
    daemon: &Arc<Daemon>,
    stopping: Value,
    stopped: Value,
) -> Result<CloudTransferCaptured> {
    let stopping = state(stopping)?;
    let operation = operation(&stopping)?;
    // Asked again (a resumed transfer): the capture this daemon still holds.
    if let Some(held) = daemon.transfer_captures.0.lock().unwrap().get(&operation) {
        return Ok(CloudTransferCaptured {
            snapshot_digest: held.snapshot_digest().to_owned(),
            blocked_chats: held.blocked_chats().clone(),
        });
    }
    // One an earlier run of this daemon left pending is gone with that process.
    daemon
        .execution
        .settle_stale_preparations(&stopping.binding.workspace_id)?;
    let captured = preparation::capture(daemon.clone(), stopping, state(stopped)?).await?;
    let result = CloudTransferCaptured {
        snapshot_digest: captured.snapshot_digest().to_owned(),
        blocked_chats: captured.blocked_chats().clone(),
    };
    daemon
        .transfer_captures
        .0
        .lock()
        .unwrap()
        .insert(operation, captured);
    Ok(result)
}

/// Seals the captured package under the published snapshot; returns its staged ciphertext and the
/// key the destination needs to open it.
pub async fn source_seal(daemon: &Arc<Daemon>, published: Value) -> Result<CloudTransferSealed> {
    let published = state(published)?;
    let operation = operation(&published)?;
    let captured = daemon
        .transfer_captures
        .0
        .lock()
        .unwrap()
        .remove(&operation);
    let binding = match captured {
        Some(captured) => captured.seal(published).await?.binding,
        // Sealed already (a resumed transfer): the ciphertext it staged then.
        None => {
            let context = archive::transfer_context(&published).map_err(Error::Internal)?;
            let head = artifact::staged_head(&daemon.paths.home, &published, &context.source_peer_id)
                .map_err(Error::Internal)?
                .ok_or_else(|| {
                    Error::Conflict(format!(
                        "This daemon holds no capture or sealed archive of operation {operation}; cancel the transfer and start it again"
                    ))
                })?;
            chauffeur_protocol::cloud_archive::CloudArchiveBinding {
                local_peer_id: context.source_peer_id.clone(),
                context,
                head,
            }
        }
    };
    let key = daemon.preparation_keys.read(
        &daemon.cloud_vault,
        &daemon.cloud_vault_keys,
        &binding.context,
        &binding.local_peer_id,
    )?;
    Ok(CloudTransferSealed {
        key: URL_SAFE_NO_PAD.encode(key.expose_bytes()),
        binding,
    })
}

pub fn source_retire(daemon: &Daemon, committed: Value) -> Result<()> {
    let committed = state(committed)?;
    daemon.execution.retire_committed(&committed)?;
    crate::cloud_workspace_discard::remove_artifacts(daemon, &committed);
    crate::cloud_workspace_backup::leave(daemon, &committed.binding.workspace_id)?;
    // The apps show it moved.
    daemon.workspaces.publish(&committed.binding.workspace_id);
    Ok(())
}

/// A cancelled transfer on the source: the workspace runs here again; a capture it holds goes.
pub fn source_release(daemon: &Daemon, cancelled: Value) -> Result<()> {
    let cancelled = state(cancelled)?;
    let held = daemon
        .transfer_captures
        .0
        .lock()
        .unwrap()
        .remove(&operation(&cancelled)?);
    match held {
        // Its preparation attempt settles with it.
        Some(captured) => captured.cancel(cancelled.clone())?,
        // One an earlier run of this daemon left pending is gone with that process.
        None => daemon
            .execution
            .settle_stale_preparations(&cancelled.binding.workspace_id)?,
    }
    crate::cloud_workspace_discard::release(daemon, &cancelled)
}

/// A cancelled transfer on the destination: what it restored here, still quarantined, and what
/// it received go.
pub async fn destination_abandon(daemon: &Arc<Daemon>, cancelled: Value) -> Result<()> {
    crate::cloud_workspace_discard::abandon(daemon, &state(cancelled)?).await
}

/// Deletes the source's retired copy of a workspace that went back where it came from; a
/// deletion that fails stays pending here and is retried.
pub async fn source_discard(
    daemon: &Arc<Daemon>,
    committed: Value,
) -> Result<CloudTransferDiscarded> {
    let committed = state(committed)?;
    Ok(CloudTransferDiscarded {
        machine: host_name(),
        pending: crate::cloud_workspace_discard::discard(daemon, &committed).await?,
    })
}

/// Opens the ciphertext this daemon received for the published snapshot and restores it under
/// quarantine. A retry after the package was opened restores from it again, which returns the
/// durable receipt of a completed restoration.
pub async fn destination_restore(
    daemon: &Arc<Daemon>,
    params: CloudTransferRestoreParams,
) -> Result<CloudTransferRestored> {
    let published = state(params.published)?;
    let local = peer(daemon)?.peer_id;
    let binding = params.binding;
    if binding.local_peer_id != local
        || archive::transfer_context(&published).map_err(Error::Internal)? != binding.context
    {
        return Err(Error::BadRequest(
            "The received archive is not bound to this destination and snapshot".into(),
        ));
    }
    let artifact::Receive::Stored(_) =
        artifact::receive(&daemon.paths.home, &published, &local, &binding.head, false)
            .map_err(Error::Internal)?
    else {
        return Err(Error::Conflict(
            "The archive has not been received completely; deliver it before restoring".into(),
        ));
    };
    let key: [u8; 32] = URL_SAFE_NO_PAD
        .decode(&params.key)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| Error::BadRequest("Invalid archive key".into()))?;
    let operation = operation(&published)?;
    let packages: PathBuf = daemon.paths.home.join("transfer-packages");
    check_restore_space(daemon, binding.head.bytes).map_err(Error::Internal)?;
    if !packages.exists() {
        crate::cloud_workspace_snapshot::private_dir(&packages)?;
    }
    let package = packages.join(&operation);
    if !package.exists() {
        archive::open(
            daemon,
            &artifact::archive_path(&daemon.paths.home, &operation),
            &package,
            &published,
            &ArchiveKey::from_bytes(key),
            &binding.head,
        )
        .await?;
    }
    let restored = crate::cloud_workspace_restore::restore(
        daemon,
        &package,
        &published,
        &local,
        params.keep_local_copy,
    )
    .await?;
    // Its services start here and its databases restore, still before the commit; coming back,
    // the containers it left here are set aside first.
    crate::workspace_service_provision::provision(
        daemon,
        &package,
        &published,
        &local,
        restored.returned,
        params.keep_local_copy,
    )
    .await?;
    Ok(CloudTransferRestored {
        receipt_digest: restored.digest,
        workspace_path: restored.receipt.workspace_path,
        kept_copy: restored.kept_copy,
        returned: restored.returned,
    })
}

/// Activates the workspace restored for the committed transfer, then starts its repos' setup
/// scripts as for a new workspace.
/// Activates the committed workspace here and starts its setup scripts, unless `toolchains` says
/// they wait for `destination_setup`.
pub fn destination_activate(
    daemon: &Arc<Daemon>,
    committed: Value,
    toolchains: bool,
) -> Result<CloudTransferActivated> {
    let committed = state(committed)?;
    daemon
        .execution
        .activate_restored(&committed, &peer(daemon)?.peer_id)?;
    // The services started for it are now its own here; asked again, there is nothing left.
    crate::workspace_service_provision::activate(
        daemon,
        &committed.binding.workspace_id,
        &operation(&committed)?,
    )?;
    crate::cloud_workspace_discard::remove_artifacts(daemon, &committed);
    // Its dev servers need ports here (`CHAUFFEUR_PORT`), which its notes name.
    let workspace = &committed.binding.workspace_id;
    daemon.workspaces.allocate_arrived_ports(workspace)?;
    if let Err(error) = daemon.workspaces.regenerate_config(workspace) {
        tracing::warn!(%error, workspace, "an arrived workspace's config was not written again");
    }
    daemon.workspaces.publish(&committed.binding.workspace_id);
    // Its restoration's receipt says whether it replaced a retired copy.
    let receipt: Option<String> = daemon.store.with(|c| {
        c.query_row(
            "SELECT receipt FROM workspace_restores WHERE workspace_id=?1",
            [&committed.binding.workspace_id],
            |r| r.get(0),
        )
    })?;
    let returned = receipt
        .and_then(|raw| serde_json::from_str::<crate::cloud_workspace_restore::Restored>(&raw).ok())
        .is_some_and(|restored| restored.returned);
    let setup = match toolchains {
        true => Vec::new(),
        false => daemon
            .workspaces
            .start_arrived_setup(&committed.binding.workspace_id)?,
    };
    Ok(CloudTransferActivated { setup, returned })
}

/// On a worker, the setup an activation left waiting: the toolchains each repo of the workspace
/// declares, installed first, then its setup scripts started.
pub async fn destination_setup(
    daemon: &Arc<Daemon>,
    params: CloudTransferSetupParams,
) -> Result<CloudTransferSetup> {
    let (toolchains, failures) =
        crate::worker_toolchains::install(daemon, &params.workspace_id).await?;
    Ok(CloudTransferSetup {
        toolchains,
        failures,
        setup: daemon
            .workspaces
            .start_arrived_setup(&params.workspace_id)?,
    })
}

/// Less free space than this where workspaces go refuses: a package, its archive and the
/// workspace all land there.
const MIN_FREE_BYTES: u64 = 1024 * 1024 * 1024;

fn required_space(bytes: u64) -> u64 {
    // Encrypted archive, unpacked snapshot, restored worktree and Git objects, plus headroom
    // for native context and metadata. This is an estimate, not a disk quota or reservation.
    bytes.saturating_mul(4).saturating_add(MIN_FREE_BYTES)
}

fn check_restore_space(daemon: &Daemon, bytes: u64) -> anyhow::Result<()> {
    for path in [
        &daemon.paths.home,
        &daemon.paths.workspaces_dir,
        &daemon.paths.repos_dir,
    ] {
        let free = free_bytes(path).ok_or_else(|| {
            anyhow::anyhow!(
                "Could not read free space at {}; check the destination disk before retrying",
                path.display()
            )
        })?;
        anyhow::ensure!(
            free >= required_space(bytes),
            "Not enough space at {}: {} MiB available, {} MiB estimated for restoration; free disk space and resume",
            path.display(),
            free / 1048576,
            required_space(bytes) / 1048576
        );
    }
    Ok(())
}

/// Whether this daemon can take the workspace, asked before the source stops: nothing of it here
/// yet, the sidecar and git present, room to restore, and each agent of its chats logged in (the
/// user's own login on this machine; listed apart when the caller signs them in from its own
/// machine). `gh` logged out only warns.
pub async fn destination_check(
    daemon: &Arc<Daemon>,
    params: CloudTransferCheckParams,
) -> Result<CloudTransferReadiness> {
    let machine = host_name();
    let mut problems = Vec::new();
    let mut warnings = Vec::new();
    // A copy a transfer to this source retired here is replaced when the workspace comes back,
    // while the journal still names the same stay, including after cancelled departures.
    let journal = params.journal.map(state).transpose()?;
    let returning = crate::cloud_workspace_return::committed_fence(daemon, &params.workspace_id)
        .map_err(Error::Internal)?
        .is_some_and(|(_, committed)| {
            crate::cloud_workspace_return::retires_for(&committed, None, &params.source_peer_id)
                && journal
                    .as_ref()
                    .is_some_and(|state| state.continues_execution_of(&committed))
        });
    if !returning && daemon.store.find_workspace(&params.workspace_id)?.is_some() {
        problems.push(format!(
            "{machine} already has workspace {}; archive it there, then retry",
            params.slug
        ));
    }
    crate::repos::validate_repo_name(&params.slug)
        .map_err(|_| Error::BadRequest("Invalid workspace name".into()))?;
    let folder = daemon.paths.workspaces_dir.join(&params.slug);
    let mut local_copy_changed = false;
    if returning
        && !params.keep_local_copy
        && let Some(problem) =
            crate::cloud_workspace_return::unsaved_changes(daemon, &params.workspace_id, &machine)
                .await
    {
        problems.push(problem);
        local_copy_changed = true;
    }
    if !returning && folder.exists() {
        problems.push(format!(
            "{} already exists on {machine}; move it away, then retry",
            folder.display()
        ));
    }
    if !daemon.chats.sidecar.available() {
        problems.push(format!(
            "The agent sidecar is not installed on {machine}; install Chauffeur there, then retry"
        ));
    }
    let git = crate::git::Git::new(daemon.shell_env.clone());
    if let Err(error) = git.run(&daemon.paths.home, &["--version"]).await {
        problems.push(format!(
            "git does not run on {machine} ({error:#}); install it, then retry"
        ));
    }
    if let Err(error) = check_restore_space(daemon, params.estimated_bytes) {
        problems.push(format!("{machine}: {error:#}"));
    }
    problems.extend(
        crate::workspace_service_provision::check(
            daemon,
            &params.workspace_id,
            &params.compose_projects,
            returning,
        )
        .await,
    );
    let mut agents = params.agents;
    agents.sort_by_key(|a| a.as_str());
    agents.dedup();
    let mut logged_out = Vec::new();
    if !agents.is_empty() {
        let usage = daemon.usage.get(daemon, true).await;
        for agent in agents {
            let (name, login) = match agent {
                AgentKind::Claude => ("Claude Code", "claude login"),
                AgentKind::Codex => ("Codex", "codex login"),
            };
            match usage.iter().find(|u| u.agent == agent) {
                Some(u) if u.account == AccountStatus::SignedIn => {}
                Some(u) if u.account == AccountStatus::SignedOut && params.sign_in_from_caller => {
                    logged_out.push(agent)
                }
                Some(u) if u.account == AccountStatus::SignedOut => problems.push(format!(
                    "{name} is not logged in on {machine}: run `{login}` there, then retry"
                )),
                found => warnings.push(format!(
                    "Could not tell whether {name} is logged in on {machine}{}",
                    found
                        .and_then(|u| u.error.as_deref())
                        .map(|e| format!(" ({e})"))
                        .unwrap_or_default()
                )),
            }
        }
    }
    let gh = tokio::process::Command::new("gh")
        .args(["auth", "status"])
        .env_clear()
        .envs(daemon.shell_env.vars())
        .stdin(std::process::Stdio::null())
        .output()
        .await;
    match gh {
        Ok(output) if output.status.success() => {}
        Ok(_) => warnings.push(format!(
            "gh is not logged in on {machine}: pull requests cannot be opened from there until `gh auth login`"
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => warnings.push(format!(
            "GitHub CLI (gh) is missing on {machine}: install gh, then run `gh auth login` there to open pull requests"
        )),
        Err(error) => warnings.push(format!(
            "Could not check GitHub access on {machine}: {error}; check `gh auth status` there before opening pull requests"
        )),
    }
    Ok(CloudTransferReadiness {
        machine,
        problems,
        warnings,
        logged_out,
        local_copy_changed,
    })
}

pub(crate) fn host_name() -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer outlives the call and its length is passed.
    let ok = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } == 0;
    let end = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
    ok.then(|| String::from_utf8_lossy(&buffer[..end]).into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "the destination".into())
}

/// Free bytes for this user on the file system of `path`, or of its nearest existing parent.
pub(crate) fn free_bytes(path: &std::path::Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let existing = path.ancestors().find(|p| p.exists())?;
    let name = std::ffi::CString::new(existing.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `name` is a valid C string and `stat` a properly sized buffer.
    if unsafe { libc::statvfs(name.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    Some(stat.f_bavail as u64 * stat.f_frsize as u64)
}
