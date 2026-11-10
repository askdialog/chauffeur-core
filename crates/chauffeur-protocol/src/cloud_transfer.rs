//! Private Unix calls a local coordinator (`chauffeur cloud transfer`, a proof tool) makes to drive
//! one workspace transfer across a source, a controller and a destination daemon. Journal states
//! travel as the daemon's own JSON, which this crate does not interpret.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::cloud_archive::CloudArchiveBinding;
use crate::model::AgentKind;

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudTransferPeer {
    pub peer_id: String,
}

/// The controller's journal of one workspace, bound to the workspace, its owner and this daemon
/// as controller.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[ts(export)]
pub enum CloudTransferJournalParams {
    Read {
        workspace_id: String,
        owner_peer_id: String,
    },
    /// Registers the workspace with its owner running it.
    Register {
        workspace_id: String,
        owner_peer_id: String,
    },
    Prepare {
        workspace_id: String,
        owner_peer_id: String,
        #[ts(type = "number")]
        revision: u64,
        operation_id: String,
        target_peer_id: String,
    },
    Advance {
        workspace_id: String,
        owner_peer_id: String,
        #[ts(type = "number")]
        revision: u64,
        operation_id: String,
        #[ts(type = "number")]
        source_epoch: u64,
        step: CloudTransferStep,
    },
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(tag = "step", rename_all = "snake_case", deny_unknown_fields)]
#[ts(export)]
pub enum CloudTransferStep {
    BeginStop,
    SourceStopped {
        peer_id: String,
        receipt_digest: String,
    },
    SnapshotPublished {
        peer_id: String,
        snapshot_digest: String,
    },
    DestinationRestored {
        peer_id: String,
        snapshot_digest: String,
        receipt_digest: String,
    },
    Commit {
        snapshot_digest: String,
        destination_receipt_digest: String,
    },
    Cancel,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudTransferJournal {
    /// Absent when the journal has no such workspace, or a stale revision changed nothing.
    pub state: Option<Value>,
}

/// One journal state the controller returned, for the step that needs exactly it.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudTransferState {
    pub state: Value,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudTransferDigest {
    pub digest: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudTransferCaptureParams {
    pub stopping: Value,
    pub stopped: Value,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudTransferCaptured {
    pub snapshot_digest: String,
    /// Chats whose conversation cannot resume on the destination, with the reason.
    pub blocked_chats: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudTransferSealed {
    /// The source's staged ciphertext, for `cloud.archive.source_open`.
    pub binding: CloudArchiveBinding,
    /// The archive key, base64url. The coordinator hands it to the destination only.
    pub key: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudTransferRestoreParams {
    pub published: Value,
    pub key: String,
    /// The destination's received ciphertext, as `cloud.archive.receive_begin` bound it.
    pub binding: CloudArchiveBinding,
    /// On a way back, keep the copy retired there beside the returning one, even if it changed.
    #[serde(default)]
    pub keep_local_copy: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudTransferRestored {
    pub receipt_digest: String,
    pub workspace_path: String,
    /// Where the retired copy kept on request now is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kept_copy: Option<String>,
    /// The workspace came back to a machine it had left, by the journal's next transfer after the
    /// commit that retired its copy there: that copy was replaced.
    #[serde(default)]
    pub returned: bool,
}

/// What the destination is asked before the source stops: whether it can take the workspace.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct TransferSizeEstimate {
    #[ts(type = "number")]
    pub bytes: u64,
}

/// What the destination is asked before the source stops: whether it can take the workspace.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudTransferCheckParams {
    pub workspace_id: String,
    pub slug: String,
    /// The source's endpoint identity: a copy here that a transfer to it retired comes back.
    pub source_peer_id: String,
    /// Carried files, notes, events and Git history before writers stop; checked again at restore.
    #[serde(default)]
    #[ts(type = "number")]
    pub estimated_bytes: u64,
    /// The agents of the workspace's chats, which must be logged in there.
    pub agents: Vec<AgentKind>,
    /// The controller journal's current state, the one the transfer will start from; absent when
    /// the controller does not know the workspace yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "unknown")]
    pub journal: Option<Value>,
    /// On a way back, a copy retired there that changed since it left is kept, not refused.
    #[serde(default)]
    pub keep_local_copy: bool,
    /// The compose projects of the workspace's declared services, which start there again: Docker
    /// with compose must answer there, and none of them may have containers there yet.
    #[serde(default)]
    pub compose_projects: Vec<String>,
    /// The caller signs logged-out agents in from its own machine (a worker of the Mac's daemon):
    /// they are listed in `logged_out` rather than as problems. Left out when false, for a
    /// daemon predating it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub sign_in_from_caller: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudTransferReadiness {
    /// The destination's host name, for messages.
    pub machine: String,
    /// What stops the transfer, each with what to do.
    pub problems: Vec<String>,
    /// What works less well there, without stopping the transfer.
    pub warnings: Vec<String>,
    /// The agents of the workspace's chats logged out there, when the caller signs them in
    /// (`sign_in_from_caller`): they stop the transfer too.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logged_out: Vec<AgentKind>,
    /// The copy the workspace left here was edited since: a return refused for it goes on when
    /// asked to keep that copy (`keep_local_copy`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub local_copy_changed: bool,
}

/// A committed state to activate on the destination.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudTransferActivateParams {
    pub state: Value,
    /// A GCP worker this Mac created: its setup scripts wait for `cloud.transfer.destination_setup`,
    /// which first installs the toolchains its repos declare.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub toolchains: bool,
}

/// The workspace whose setup scripts a worker starts, its repos' toolchains installed first.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudTransferSetupParams {
    pub workspace_id: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudTransferSetup {
    #[serde(default)]
    pub failures: Vec<String>,
    /// What was installed for each repo, and what could not be, for the transfer's log.
    pub toolchains: Vec<String>,
    /// The setup scripts started, as the workspace's processes name them (`setup <repo>`).
    pub setup: Vec<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudTransferActivated {
    /// The setup scripts started, as the workspace's processes name them (`setup <repo>`); none
    /// when they wait for `cloud.transfer.destination_setup`.
    pub setup: Vec<String>,
    /// The restoration replaced a copy retired here: the workspace came back, and the copy it
    /// left on the source goes.
    #[serde(default)]
    pub returned: bool,
}

/// The source's copy of a workspace that went back where it came from, deleted or not yet.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudTransferDiscarded {
    /// The machine that kept the copy.
    pub machine: String,
    /// Why the deletion is still pending there, retried in the background; none once deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pending: Option<String>,
}

/// One end of a transfer: this daemon, a machine of `[transfer]`, or a daemon socket named
/// directly (`chauffeur cloud transfer --to <socket>`).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum TransferEndpoint {
    Here,
    /// An on-demand GCP worker, shared until the last workspace returns here.
    Cloud {
        #[serde(default)]
        beyond_limit: bool,
    },
    Machine {
        name: String,
    },
    Socket {
        path: String,
    },
}

/// `cloud.transfer.start`: this daemon drives the transfer in the background, whoever asked;
/// closing the app does not stop it.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CloudTransferStartParams {
    /// The workspace, by id or name, as the source knows it.
    pub workspace: String,
    pub source: TransferEndpoint,
    pub destination: TransferEndpoint,
    /// The controller daemon's socket; by default `[transfer] controller`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub controller: Option<String>,
    /// On a way back, keep the destination's retired copy beside the returning one even if it
    /// was edited since it moved.
    #[serde(default)]
    pub keep_local_copy: bool,
    /// Interrupt the workspace's agents rather than wait for the end of their turns. Left out
    /// when false: a daemon or mobile host that predates it refuses an unknown field.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub interrupt: bool,
    /// Archive the workspace here once it is back: what archiving a moved workspace asks. Left out
    /// when false, as `interrupt` is.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub archive: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TransferPhase {
    Running,
    Done,
    Failed,
    /// Cancelled before it committed: the workspace runs on its source again.
    Cancelled,
}

/// Where a running transfer is. Until it stops the source, cancelling it leaves the workspace
/// running there, nothing of it stopped (agents it interrupted stay interrupted).
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TransferStage {
    /// Creating or reconnecting the on-demand worker before stopping the source.
    PreparingWorker,
    /// Asking the destination whether it can take the workspace.
    Preparing,
    /// Waiting for the workspace's agents to end their turn (`working_agents`).
    WaitingForTurns,
    /// Waiting for another transfer this daemon drives to end: a few run at once.
    Queued,
    /// Stopping the source, then capturing and sealing the workspace.
    Stopping,
    Sending,
    Restoring,
    /// Committed: activating the destination, retiring the source, running the setup scripts.
    Activating,
}

/// A transfer this daemon drives or drove, the latest one per workspace.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct TransferProgress {
    pub stage_started_at: String,
    /// Bytes acknowledged by the destination, before its final integrity check.
    #[ts(type = "number")]
    pub confirmed_bytes: u64,
    #[ts(type = "number")]
    pub total_bytes: u64,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
#[ts(export)]
pub enum TransferPreparation {
    Running {
        scripts: Vec<String>,
    },
    Ready,
    /// Ownership has moved. Inspect or rerun setup on the destination, never repeat the handoff.
    NeedsAttention {
        scripts: Vec<String>,
        message: String,
    },
}

/// A transfer this daemon drives or drove, the latest one per workspace.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct TransferInfo {
    /// Orders updates to one transfer, including progress with no new log line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub revision: Option<u32>,
    pub id: String,
    pub workspace_id: String,
    pub slug: String,
    /// Where it comes from and goes to, as shown: a machine's name, or this machine's.
    pub source: String,
    pub destination: String,
    pub phase: TransferPhase,
    /// What it did so far, one line per step, warnings and setup outcomes included.
    pub log: Vec<String>,
    pub error: Option<String>,
    /// Where the workspace now is, and a copy kept beside it, once done.
    pub workspace_path: Option<String>,
    pub kept_copy: Option<String>,
    /// It came back to a machine it had left.
    pub returned: bool,
    pub started_at: String,
    pub finished_at: Option<String>,
    /// The ends to name again to retry or cancel it from an app: absent when one is a socket,
    /// which only the CLI names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub from: Option<TransferEndpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub to: Option<TransferEndpoint>,
    /// It was asked to keep the destination's edited retired copy; a retry asks the same.
    #[serde(default)]
    pub keep_local_copy: bool,
    /// The peer the controller's journal keys the workspace under, once read: a cancellation
    /// reads the journal with it when the other end does not answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub journal_owner: Option<String>,
    /// A return left its copy on the source, which could not delete it yet: why. The source
    /// retries every ten minutes; this daemon asks it again and clears this once it is gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub discard_pending: Option<String>,
    /// Where it is while it runs; none once it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub stage: Option<TransferStage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub progress: Option<TransferProgress>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub preparation: Option<TransferPreparation>,
    /// While it waits for the end of their turns: how many of the workspace's agents work, or wait
    /// for the user's answer.
    #[serde(default)]
    pub working_agents: u32,
    /// Its agents are interrupted rather than waited for (asked at the start, or since).
    #[serde(default)]
    pub interrupt: bool,
    /// The workspace is archived here once back (archiving it while it ran on another machine);
    /// a retry asks the same.
    #[serde(default)]
    pub archive: bool,
    /// It failed, before anything stopped, because these agents are not signed in on the worker
    /// it goes to: signing them in there (`worker.login`) resumes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub needs_login: Option<TransferLogin>,
    /// It failed because the copy the workspace left on its destination was edited since: asked
    /// again with `keep_local_copy`, it keeps that copy beside the returning one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub local_copy_changed: bool,
    /// It failed before it asked the controller to prepare it, with no other transfer of the
    /// workspace in flight: nothing of the workspace stopped, was fenced or staged. Retrying starts
    /// it again; cancelling only ends it here.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub untouched: bool,
}

/// The agents a worker lacks the login of, for a transfer to it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct TransferLogin {
    pub worker: String,
    pub agents: Vec<AgentKind>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct CloudTransferStatus {
    pub transfer: Option<TransferInfo>,
}

/// `cloud.transfer.machines`: the machines of `[transfer]` a workspace can go to, by name only
/// (their sockets stay on this machine); the default controller is prepared automatically.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct CloudTransferMachines {
    pub machines: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transfer_that_waits_for_its_agents_is_asked_as_a_daemon_predating_interruption_reads_it() {
        let mut params = CloudTransferStartParams {
            workspace: "w".into(),
            source: TransferEndpoint::Here,
            destination: TransferEndpoint::Machine { name: "vm".into() },
            controller: None,
            keep_local_copy: false,
            interrupt: false,
            archive: false,
        };
        let waits = serde_json::to_value(&params).unwrap();
        assert!(waits.get("interrupt").is_none(), "{waits}");
        assert!(waits.get("archive").is_none(), "{waits}");
        params.interrupt = true;
        assert_eq!(serde_json::to_value(&params).unwrap()["interrupt"], true);
        let read: CloudTransferStartParams = serde_json::from_value(waits).unwrap();
        assert!(!read.interrupt);
    }
}
