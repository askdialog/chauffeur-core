//! Dispatch of the calls of `/peer` (`PeerCall`): another daemon driving a transfer. The server
//! owns the archive handles, which live as long as their connection.

use std::sync::Arc;

use chauffeur_protocol::client::EmptyParams;
use chauffeur_protocol::peer::{PEER_PROTOCOL_VERSION, PeerCall, PeerHelloResult};
use serde_json::Value;

use crate::{Daemon, Error, Result};

pub async fn handle(daemon: &Arc<Daemon>, call: PeerCall) -> Result<Value> {
    match call {
        PeerCall::Hello(params) => {
            if params.protocol_version != PEER_PROTOCOL_VERSION {
                return Err(Error::BadRequest(format!(
                    "peer protocol {} is not supported (daemon speaks {PEER_PROTOCOL_VERSION}); run the same Chauffeur on both ends",
                    params.protocol_version
                )));
            }
            to_value(PeerHelloResult {
                protocol_version: PEER_PROTOCOL_VERSION,
                daemon_version: env!("CARGO_PKG_VERSION").to_string(),
            })
        }
        PeerCall::CloudTransferPeer(_) => to_value(crate::cloud_transfer::peer(daemon)?),
        PeerCall::CloudTransferSourceEstimate(params) => {
            to_value(chauffeur_protocol::cloud_transfer::TransferSizeEstimate {
                bytes: crate::cloud_workspace_package::check_folder(daemon, &params.workspace)
                    .await?,
            })
        }
        PeerCall::CloudTransferJournal(params) => {
            let daemon = daemon.clone();
            to_value(
                tokio::task::spawn_blocking(move || {
                    crate::cloud_transfer::journal(&daemon, params)
                })
                .await
                .map_err(|e| Error::Internal(e.into()))??,
            )
        }
        PeerCall::CloudTransferSourceStop(p) => {
            to_value(chauffeur_protocol::cloud_transfer::CloudTransferDigest {
                digest: crate::cloud_transfer::source_stop(daemon, p.state).await?,
            })
        }
        PeerCall::CloudTransferSourceCapture(p) => {
            to_value(crate::cloud_transfer::source_capture(daemon, p.stopping, p.stopped).await?)
        }
        PeerCall::CloudTransferSourceSeal(p) => {
            to_value(crate::cloud_transfer::source_seal(daemon, p.state).await?)
        }
        PeerCall::CloudTransferSourceRetire(p) => {
            crate::cloud_transfer::source_retire(daemon, p.state)?;
            to_value(EmptyParams {})
        }
        PeerCall::CloudTransferSourceRelease(p) => {
            crate::cloud_transfer::source_release(daemon, p.state)?;
            to_value(EmptyParams {})
        }
        PeerCall::CloudTransferDestinationAbandon(p) => {
            crate::cloud_transfer::destination_abandon(daemon, p.state).await?;
            to_value(EmptyParams {})
        }
        PeerCall::CloudBackupConfigure(p) => {
            crate::cloud_workspace_backup::configure(daemon, p)?;
            to_value(EmptyParams {})
        }
        PeerCall::WorkerCredentialsInstall(p) => {
            to_value(crate::worker_credentials::install(daemon, p).await?)
        }
        PeerCall::WorkerClaudeConfigStatus(_) => {
            to_value(crate::worker_claude_config::status(daemon))
        }
        PeerCall::WorkerClaudeConfigWrite(p) => {
            to_value(crate::worker_claude_config::write(daemon, p)?)
        }
        PeerCall::WorkerClaudeConfigInstall(p) => {
            to_value(crate::worker_claude_config::install(daemon, p).await?)
        }
        PeerCall::CloudTransferSourceDiscard(p) => {
            to_value(crate::cloud_transfer::source_discard(daemon, p.state).await?)
        }
        PeerCall::CloudTransferDestinationRestore(p) => {
            to_value(crate::cloud_transfer::destination_restore(daemon, p).await?)
        }
        PeerCall::CloudTransferDestinationActivate(p) => to_value(
            crate::cloud_transfer::destination_activate(daemon, p.state, p.toolchains)?,
        ),
        PeerCall::CloudTransferDestinationSetup(p) => {
            to_value(crate::cloud_transfer::destination_setup(daemon, p).await?)
        }
        PeerCall::CloudTransferDestinationCheck(p) => {
            to_value(crate::cloud_transfer::destination_check(daemon, p).await?)
        }
        PeerCall::CloudArchiveSourceOpen(_)
        | PeerCall::CloudArchiveReceiveBegin(_)
        | PeerCall::CloudArchiveRead(_)
        | PeerCall::CloudArchiveWrite(_)
        | PeerCall::CloudArchiveCommit(_)
        | PeerCall::CloudArchiveClose(_) => Err(Error::BadRequest(
            "Archive I/O requires its owning private Unix connection".into(),
        )),
    }
}

fn to_value<T: serde::Serialize>(value: T) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}
