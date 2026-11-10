//! `chauffeur cloud transfer`: asks the source's daemon to move workspaces to a destination
//! daemon, a controller daemon keeping the journal (`cloud.transfer.start`, once per workspace),
//! then follows the transfers the daemon drives, printing each step as it comes. Stopping this
//! command does not stop them; `cloud.transfer.status` reads where each is.
use std::path::Path;
use std::time::Duration;

use anyhow::bail;
use chauffeur_protocol::client::{ClientCall, WorkspaceParams};
use chauffeur_protocol::cloud_transfer::{
    CloudTransferStartParams, CloudTransferStatus, TransferEndpoint, TransferInfo, TransferPhase,
};

use chauffeur_protocol::model::WorkspaceInfo;

use crate::client::Client;

/// What to ask of each workspace.
#[derive(Clone, Copy)]
pub struct Ask<'a> {
    /// The destination; none brings back here a workspace that moved away, from the machine it
    /// went to.
    pub to: Option<&'a str>,
    pub controller: Option<&'a Path>,
    pub keep_local_copy: bool,
    /// Cancel that transfer instead.
    pub cancel: bool,
    /// Interrupt the agents rather than wait for the end of their turns.
    pub interrupt: bool,
    pub yes: bool,
    pub beyond_limit: bool,
}

/// A destination named with a path is a daemon socket; otherwise a machine of Settings → Transfer.
fn endpoint(to: &str) -> TransferEndpoint {
    if to.contains('/') || to.starts_with('~') {
        TransferEndpoint::Socket {
            path: to.to_owned(),
        }
    } else {
        TransferEndpoint::Machine {
            name: to.to_owned(),
        }
    }
}

async fn params(
    driver: &mut Client,
    workspace: String,
    ask: &Ask<'_>,
) -> anyhow::Result<CloudTransferStartParams> {
    let (workspace, source, destination) = match ask.to {
        Some("cloud") => (
            workspace,
            TransferEndpoint::Here,
            TransferEndpoint::Cloud {
                beyond_limit: ask.beyond_limit,
            },
        ),
        Some(to) => (workspace, TransferEndpoint::Here, endpoint(to)),
        None => {
            let info: WorkspaceInfo = driver
                .call(ClientCall::WorkspaceGet(WorkspaceParams { workspace }))
                .await?;
            let Some(name) = info.moved_to else {
                bail!("{} runs here; it has nothing to come back from", info.slug);
            };
            (info.id, endpoint(&name), TransferEndpoint::Here)
        }
    };
    Ok(CloudTransferStartParams {
        workspace,
        source,
        destination,
        controller: ask.controller.map(|path| path.display().to_string()),
        keep_local_copy: ask.keep_local_copy,
        interrupt: ask.interrupt,
        archive: false,
    })
}

/// Asks each workspace's transfer, then follows those that started until each ends: one failing
/// leaves the others going. Returns each workspace's outcome, in the order named; a transfer that
/// failed or was cancelled is an error.
pub async fn run(
    driver: &mut Client,
    workspaces: Vec<String>,
    ask: &Ask<'_>,
) -> Vec<(String, anyhow::Result<TransferInfo>)> {
    // Several at once: each line says whose it is.
    let prefix = workspaces.len() > 1;
    let mut outcomes = Vec::new();
    for workspace in workspaces {
        let asked = async {
            let params = params(driver, workspace.clone(), ask).await?;
            if ask.cancel {
                let info: TransferInfo =
                    driver.call(ClientCall::CloudTransferCancel(params)).await?;
                for line in &info.log {
                    print_line(prefix, &info.slug, line);
                }
                return Ok(info);
            }
            driver.call(ClientCall::CloudTransferStart(params)).await
        }
        .await;
        if let Err(error) = &asked
            && prefix
        {
            eprintln!("{workspace}: {error:#}");
        }
        outcomes.push((workspace, asked));
    }
    if ask.cancel {
        return outcomes;
    }
    // How many log lines of each running transfer were printed.
    let mut printed = vec![0; outcomes.len()];
    loop {
        let mut running = false;
        for ((_, outcome), printed) in outcomes.iter_mut().zip(printed.iter_mut()) {
            let Ok(info) = outcome else { continue };
            if info.phase != TransferPhase::Running {
                continue;
            }
            let read: anyhow::Result<CloudTransferStatus> = driver
                .call(ClientCall::CloudTransferStatus(WorkspaceParams {
                    workspace: info.workspace_id.clone(),
                }))
                .await;
            let current = match read {
                Ok(status) => status.transfer.filter(|t| t.id == info.id),
                Err(error) => {
                    *outcome = Err(error);
                    continue;
                }
            };
            let Some(current) = current else {
                *outcome = Err(anyhow::anyhow!("the daemon lost track of the transfer"));
                continue;
            };
            for line in &current.log[(*printed).min(current.log.len())..] {
                print_line(prefix, &current.slug, line);
            }
            *printed = current.log.len();
            running |= current.phase == TransferPhase::Running;
            *info = current;
        }
        if !running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    outcomes
        .into_iter()
        .map(|(workspace, outcome)| {
            let outcome = outcome.and_then(|info| match info.phase {
                TransferPhase::Failed => {
                    bail!("{}", info.error.as_deref().unwrap_or("the transfer failed"))
                }
                // Cancelled from elsewhere while this command followed it.
                TransferPhase::Cancelled => bail!("the transfer was cancelled"),
                _ => Ok(info),
            });
            (workspace, outcome)
        })
        .collect()
}

/// A step goes to standard error, indented when it details the one before.
fn print_line(prefix: bool, slug: &str, line: &str) {
    let mark = if line.starts_with(' ') { "" } else { "· " };
    if prefix {
        eprintln!("{slug}: {mark}{line}");
    } else {
        eprintln!("{mark}{line}");
    }
}
