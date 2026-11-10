//! Transfers of workspaces to other machines, and the backups of those on a GCP worker.
use crate::client::Client;
use anyhow::bail;
use chauffeur_protocol::client::{ClientCall, EmptyParams, WorkspaceParams};
use chauffeur_protocol::cloud_backup::{BackupHealth, BackupStatus};
use chauffeur_protocol::cloud_transfer::{
    CloudTransferStatus, TransferInfo, TransferPhase, TransferStage,
};
use chauffeur_protocol::model::WorkspaceInfo;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum Action {
    /// Move workspaces to another machine's daemon, a controller daemon keeping the journal.
    /// The source daemon drives them (this one unless `--from` names one), so they go on if this
    /// command stops. Each waits for the end of its agents' turns before stopping them, and has
    /// its own outcome: one failing leaves the others going.
    #[command(hide = true)]
    Transfer {
        #[arg(required = true)]
        workspaces: Vec<String>,
        /// The destination: cloud (on-demand GCP), a machine, or a daemon socket path.
        #[arg(
            long,
            required_unless_present_any = ["back", "status"],
            conflicts_with_all = ["back", "status"]
        )]
        to: Option<String>,
        /// Bring a workspace that moved away back here; this daemon drives it.
        #[arg(long)]
        back: bool,
        /// The controller daemon's socket; by default the one of Settings → Transfer.
        #[arg(long)]
        controller: Option<PathBuf>,
        /// The source daemon's socket, when it is not this one.
        #[arg(long)]
        from: Option<PathBuf>,
        /// On a way back, keep the destination's old copy beside the returning one even if it
        /// was edited since it moved, rather than refuse.
        #[arg(long)]
        keep_local_copy: bool,
        /// Cancel the transfer named by `--to` or `--back` instead, if it has not committed: the
        /// source runs the workspace again and the destination drops what it received.
        #[arg(long, conflicts_with = "keep_local_copy")]
        cancel: bool,
        /// Interrupt the workspace's agents rather than wait for the end of their turns.
        #[arg(long, conflicts_with_all = ["cancel", "status"])]
        interrupt: bool,
        /// Print the workspace's latest transfer instead: where it is, how it ended, and a copy
        /// its source has not deleted yet.
        #[arg(long, conflicts_with_all = ["back", "cancel", "keep_local_copy", "controller"])]
        status: bool,
        /// Accept the estimated cost of an on-demand worker and exceeding the worker limit.
        #[arg(long)]
        yes: bool,
    },
    /// Recover a worker's backup here, deleting its VM with its disks kept first.
    Recover {
        workspace: String,
        /// Backup id; the newest stored backup by default.
        #[arg(long)]
        backup: Option<String>,
        /// Confirm stopping every workspace of this worker and losing edits newer than the backup.
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        keep_local_copy: bool,
    },
    /// Back up a workspace running on a GCP worker now, on that worker, or show where its
    /// backups stand.
    Backup {
        workspace: String,
        /// Only show where its backups stand.
        #[arg(long)]
        status: bool,
    },
}

/// One workspace's outcome in `--json`: its transfer, or why it did not move.
#[derive(serde::Serialize)]
struct Outcome {
    workspace: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    transfer: Option<TransferInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Starts the transfer of each workspace in `driver`'s daemon, which drives them, and follows them
/// until each ends; see `crate::transfer`. Fails when one did not move, after the others did.
pub async fn transfer(
    driver: &mut Client,
    workspaces: Vec<String>,
    ask: &crate::transfer::Ask<'_>,
    json: bool,
) -> anyhow::Result<()> {
    let mut ask = crate::transfer::Ask { ..*ask };
    if ask.to == Some("cloud") && !ask.cancel {
        let result: chauffeur_protocol::worker::WorkersResult =
            driver.call(ClientCall::WorkerList(EmptyParams {})).await?;
        let offer = result
            .machine_types
            .iter()
            .find(|m| m.name == result.default_machine_type)
            .ok_or_else(|| {
                anyhow::anyhow!("No default worker estimate is available; read worker list")
            })?;
        eprintln!(
            "Cloud: reuse an automatic worker or create {} in {} ({}), about ${:.2}/h including disk and address while it runs, then stopped with only its disk billed once its last workspace returns; network and model use extra (prices {}).",
            offer.name, result.zone, result.project, offer.hourly_usd, result.prices_as_of
        );
        ask.beyond_limit = result.workers.len() as u32 >= result.limit;
        if ask.beyond_limit {
            eprintln!(
                "{} workers already exist; creation would exceed the limit of {}.",
                result.workers.len(),
                result.limit
            );
        }
        if !ask.yes && !crate::confirm("Send to Cloud with this worker cost?")? {
            bail!("nothing transferred");
        }
    }
    let mut outcomes = crate::transfer::run(driver, workspaces, &ask).await;
    if outcomes.len() == 1 {
        let (_, outcome) = outcomes.remove(0);
        let info = outcome?;
        super::print_or(json, &info, || ended(&info));
        return Ok(());
    }
    let failed = outcomes.iter().filter(|(_, o)| o.is_err()).count();
    let total = outcomes.len();
    let listed: Vec<Outcome> = outcomes
        .into_iter()
        .map(|(workspace, outcome)| match outcome {
            Ok(info) => Outcome {
                workspace,
                transfer: Some(info),
                error: None,
            },
            Err(error) => Outcome {
                workspace,
                transfer: None,
                error: Some(format!("{error:#}")),
            },
        })
        .collect();
    super::print_or(json, &listed, || {
        listed
            .iter()
            .map(|outcome| match (&outcome.transfer, &outcome.error) {
                (Some(info), _) => format!("{}: {}", outcome.workspace, ended(info)),
                (None, error) => format!(
                    "{}: failed: {}",
                    outcome.workspace,
                    error.as_deref().unwrap_or_default()
                ),
            })
            .collect::<Vec<_>>()
            .join("\n")
    });
    if failed > 0 {
        bail!("{failed} of {total} transfers did not go through; the others did");
    }
    Ok(())
}

/// How a transfer that went through ended, for its line.
fn ended(info: &TransferInfo) -> String {
    if info.phase == TransferPhase::Cancelled {
        return format!("Cancelled; {} runs on the source again.", info.slug);
    }
    let path = info.workspace_path.as_deref().unwrap_or_default();
    let mut text = if info.returned {
        format!("Back in {path}.")
    } else {
        format!(
            "Moved to {path}; the source keeps it read-only. Agents there use that machine's own login."
        )
    };
    if let Some(chauffeur_protocol::cloud_transfer::TransferPreparation::NeedsAttention {
        message,
        ..
    }) = &info.preparation
    {
        text.push_str(&format!("\nPreparation needs attention: {message}"));
    }
    if let Some(kept) = &info.kept_copy {
        text.push_str(&format!(
            "\nThe copy that was there is kept at {kept}; remove it once you no longer need it."
        ));
    }
    text
}

/// The latest transfer of each workspace that `driver` drove, as `cloud.transfer.status` reads it.
pub async fn transfer_status(
    driver: &mut Client,
    workspaces: Vec<String>,
    json: bool,
) -> anyhow::Result<()> {
    let mut statuses = Vec::new();
    for workspace in workspaces {
        let status: CloudTransferStatus = driver
            .call(ClientCall::CloudTransferStatus(WorkspaceParams {
                workspace: workspace.clone(),
            }))
            .await?;
        statuses.push((workspace, status));
    }
    if let [(_, status)] = statuses.as_slice() {
        super::print_or(json, status, || status_text(status));
        return Ok(());
    }
    // Several: one array in `--json`, a line per workspace named otherwise.
    let listed: Vec<&CloudTransferStatus> = statuses.iter().map(|(_, s)| s).collect();
    super::print_or(json, &listed, || {
        statuses
            .iter()
            .map(|(workspace, status)| format!("{workspace}: {}", status_text(status)))
            .collect::<Vec<_>>()
            .join("\n")
    });
    Ok(())
}

fn status_text(status: &CloudTransferStatus) -> String {
    let Some(info) = &status.transfer else {
        return "No transfer of this workspace was driven from here.".into();
    };
    let mut text = match info.phase {
        TransferPhase::Running if info.stage == Some(TransferStage::WaitingForTurns) => {
            format!(
                "Waiting for the end of {} agent turn(s) before transferring {} to {}{}",
                info.working_agents,
                info.slug,
                info.destination,
                if info.interrupt {
                    "; interrupting them"
                } else {
                    ""
                }
            )
        }
        TransferPhase::Running => format!(
            "Transferring to {}: {}",
            info.destination,
            info.log.last().map(|l| l.trim()).unwrap_or("starting")
        ),
        TransferPhase::Done if info.returned && info.archive => {
            format!("Back from {}; archived here.", info.source)
        }
        TransferPhase::Done if info.returned => format!("Back from {}.", info.source),
        TransferPhase::Done => format!("Moved to {}.", info.destination),
        TransferPhase::Failed => format!(
            "The transfer to {} failed: {}",
            info.destination,
            info.error.as_deref().unwrap_or("no reason given")
        ),
        TransferPhase::Cancelled => {
            format!("Cancelled; {} runs on {} again.", info.slug, info.source)
        }
    };
    if let Some(pending) = &info.discard_pending {
        text.push_str(&format!(
            "\nIts copy on {} is not deleted yet ({pending}); it is retried there.",
            info.source
        ));
    }
    text
}

/// Where a workspace's backups stand, in a few lines.
fn backup_text(slug: &str, status: Option<&BackupStatus>) -> String {
    let Some(status) = status else {
        return format!("{slug} is not backed up: only a workspace running on a GCP worker is.");
    };
    let last = match (&status.last_success_at, status.backups.first()) {
        (Some(at), Some(newest)) if newest.live => format!("last backup {at}, taken during a turn"),
        (Some(at), _) => format!("last backup {at}"),
        (None, _) => "no backup yet".into(),
    };
    let mut text = match status.health {
        BackupHealth::Pending => format!("{slug}: {last}; the first one is on its way."),
        BackupHealth::Current => format!("{slug}: {last}, current."),
        BackupHealth::Stale => format!(
            "{slug}: {last}, stale: none succeeded for fifteen minutes; is the worker reachable?"
        ),
        BackupHealth::Failed => format!(
            "{slug}: {last}; the last attempt failed: {}",
            status.error.as_deref().unwrap_or("no reason given")
        ),
    };
    text.push_str(&format!(
        "
{} kept.",
        status.backups.len()
    ));
    if !status.left_out.is_empty() {
        text.push_str(&format!(
            "
Left out (in no repo, outside .chauffeur/): {}",
            status.left_out.join(", ")
        ));
    }
    text
}

pub async fn run(action: Action, client: &mut Client, json: bool) -> anyhow::Result<()> {
    match action {
        Action::Recover {
            workspace,
            backup,
            yes,
            keep_local_copy,
        } => {
            use chauffeur_protocol::cloud_backup::{
                BackupRecoverParams, BackupRecovered, BackupRecoveryPlan,
            };
            let plan: BackupRecoveryPlan = client
                .call(ClientCall::CloudBackupRecoveryPlan(WorkspaceParams {
                    workspace,
                }))
                .await?;
            let selected = backup
                .as_deref()
                .and_then(|id| plan.backups.iter().find(|b| b.id == id))
                .or_else(|| {
                    if backup.is_none() {
                        plan.backups.first()
                    } else {
                        None
                    }
                })
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "No matching backup in storage; read the worker's backups before recovering"
                    )
                })?;
            eprintln!(
                "Recover backup {} ({}) here. Edits newer than it may be lost; databases are not backed up.",
                selected.id, selected.taken_at
            );
            eprintln!(
                "Delete worker {} and stop these workspaces: {}. Its disks are kept for manual recovery and still cost storage.",
                plan.worker,
                plan.affected_workspaces.join(", ")
            );
            if !yes && !crate::confirm("Delete the old worker VM and recover this backup here?")? {
                bail!("nothing recovered");
            }
            let result: BackupRecovered = client
                .call(ClientCall::CloudBackupRecover(BackupRecoverParams {
                    workspace: plan.workspace,
                    worker: plan.worker,
                    epoch: plan.epoch,
                    backup: selected.id.clone(),
                    affected_workspaces: plan.affected_workspaces,
                    keep_local_copy,
                }))
                .await?;
            super::print_or(json, &result, || {
                format!(
                    "Recovered in {}. Kept disk {} still costs storage; inspect it before removing it manually.",
                    result.workspace_path, result.retained_disk
                )
            });
            Ok(())
        }

        Action::Backup { workspace, status } => {
            let info: WorkspaceInfo = client
                .call(ClientCall::WorkspaceGet(WorkspaceParams {
                    workspace: workspace.clone(),
                }))
                .await?;
            let backup = if status {
                info.backup.map(|backup| *backup)
            } else {
                Some(
                    client
                        .call(ClientCall::CloudBackupRun(WorkspaceParams { workspace }))
                        .await?,
                )
            };
            super::print_or(json, &backup, || backup_text(&info.slug, backup.as_ref()));
            Ok(())
        }
        Action::Transfer {
            workspaces,
            status: true,
            ..
        } => transfer_status(client, workspaces, json).await,
        Action::Transfer {
            workspaces,
            to,
            controller,
            keep_local_copy,
            cancel,
            interrupt,
            yes,
            ..
        } => {
            let ask = crate::transfer::Ask {
                to: to.as_deref(),
                controller: controller.as_deref(),
                keep_local_copy,
                cancel,
                interrupt,
                yes,
                beyond_limit: false,
            };
            transfer(client, workspaces, &ask, json).await
        }
    }
}
