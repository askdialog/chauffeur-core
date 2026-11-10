//! GCP workers: Compute Engine VMs the daemon creates with the user's `gcloud`, installs Chauffeur
//! on and reaches through a socket forwarded over SSH, so that a workspace can be transferred
//! there (docs/cloud-workers.md).
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::model::AgentKind;

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum WorkerState {
    /// The VM is being created.
    Creating,
    /// Chauffeur is being installed and started on it.
    Installing,
    Ready,
    /// Creating or installing failed: the VM may exist and cost; delete it, or create it again.
    Failed,
    Deleting,
    /// An on-demand worker whose last workspace came back is being stopped.
    Stopping,
    /// Stopped with its disk, which keeps its agents' logins: only the disk costs. The next
    /// transfer to Cloud starts it again and installs this Mac's Chauffeur on it.
    Stopped,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct WorkerInfo {
    /// Shown in the app and named by `chauffeur cloud transfer --to`, as a machine.
    pub name: String,
    /// The Compute Engine instance, `chauffeur-worker-<name>`.
    pub instance: String,
    pub project: String,
    pub zone: String,
    pub machine_type: String,
    /// The estimated cost of the VM, its disk and its address, in US dollars an hour.
    pub hourly_usd: f64,
    pub state: WorkerState,
    /// The forwarded socket reaches the worker's daemon.
    pub connected: bool,
    /// The local end of the forwarded socket.
    pub socket: String,
    /// The `Host` this machine's editors open it with over SSH (Remote-SSH), once its entry is written
    /// and the ssh config includes it.
    pub ssh_host: Option<String>,
    /// Why it does not open in an editor of this machine yet, with what to do (the `Include` line
    /// to add by hand…); none once `ssh_host` is set.
    pub ssh_issue: Option<String>,
    /// What creating, installing or deleting it did, one line per step.
    pub log: Vec<String>,
    /// Why it failed; for a ready worker not `connected`, why its forward does not reach it now
    /// (gcloud signed out, the key refused…), once a try ended.
    pub error: Option<String>,
    pub created_at: String,
}

/// A machine type a worker can be created with.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct WorkerMachineType {
    pub name: String,
    pub vcpus: u32,
    pub memory_gb: u32,
    /// The estimated cost of the VM, its disk and its address, in US dollars an hour.
    pub hourly_usd: f64,
}

/// `worker.list`: the workers, and what a new one would be.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct WorkersResult {
    pub workers: Vec<WorkerInfo>,
    pub machine_types: Vec<WorkerMachineType>,
    pub default_machine_type: String,
    /// Where new workers are created (`[workers]`).
    pub project: String,
    pub zone: String,
    pub disk_gb: u32,
    /// The date of the price table the estimates come from.
    pub prices_as_of: String,
    /// Workers created without asking again; one more needs `beyond_limit`.
    pub limit: u32,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct WorkerCreateParams {
    /// Lowercase letters, digits and dashes, starting with a letter.
    pub name: String,
    /// One of `worker.list`'s machine types; by default its default one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub machine_type: Option<String>,
    /// The user confirmed one more worker than the limit.
    #[serde(default)]
    pub beyond_limit: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct WorkerParams {
    pub name: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct WorkerLoginParams {
    pub name: String,
    pub agent: AgentKind,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct WorkerLoginCodeParams {
    pub name: String,
    pub agent: AgentKind,
    pub code: String,
}

/// What a worker gets of this Mac's own GitHub and AWS access on each send, so that its user
/// signs in to neither again there. Secrets: never logged, never in a result or an event.
#[derive(Serialize, Deserialize, TS, Clone, Default)]
#[ts(export)]
pub struct WorkerCredentials {
    /// The token of each host the Mac's `gh` is signed in to.
    pub github: Vec<WorkerGithubToken>,
    /// The AWS CLI's `config`, whole; none when the Mac has none.
    pub aws_config: Option<String>,
    /// The AWS CLI's SSO token cache (`sso/cache/*.json`).
    pub aws_sso_cache: Vec<WorkerAwsSsoFile>,
}

/// Names the hosts and files only: the tokens stay out of any debug output.
impl std::fmt::Debug for WorkerCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerCredentials")
            .field(
                "github",
                &self.github.iter().map(|t| &t.host).collect::<Vec<_>>(),
            )
            .field("aws_config", &self.aws_config.is_some())
            .field(
                "aws_sso_cache",
                &self
                    .aws_sso_cache
                    .iter()
                    .map(|f| &f.name)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[derive(Serialize, Deserialize, TS, Clone)]
#[ts(export)]
pub struct WorkerGithubToken {
    pub host: String,
    pub token: String,
}

#[derive(Serialize, Deserialize, TS, Clone)]
#[ts(export)]
pub struct WorkerAwsSsoFile {
    /// Its file name in `sso/cache`, `<hash>.json`.
    pub name: String,
    pub contents: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
pub struct WorkerCredentialsInstalled {
    /// What could not be installed, for the transfer's log.
    pub warnings: Vec<String>,
}

/// `worker.claude_config_status`: the digest of the Claude Code configuration a worker last
/// installed from a Mac; none before the first, so that an unchanged one is not sent again.
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
pub struct WorkerClaudeConfigStatus {
    pub digest: Option<String>,
}

/// `worker.claude_config_write`: a chunk of the archive of a Mac's Claude Code configuration, at
/// `offset` of the archive `digest` names (base64url); offset 0 starts that archive again.
#[derive(Serialize, Deserialize, TS, Clone)]
#[ts(export)]
pub struct WorkerClaudeConfigChunk {
    pub digest: String,
    #[ts(type = "number")]
    pub offset: u64,
    pub bytes: String,
}

/// Names the archive and the place only: its contents (settings, MCP headers) stay out of logs.
impl std::fmt::Debug for WorkerClaudeConfigChunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerClaudeConfigChunk")
            .field("digest", &self.digest)
            .field("offset", &self.offset)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// `worker.claude_config_install`: the archive `digest` names, written whole (`bytes` long).
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct WorkerClaudeConfigInstallParams {
    pub digest: String,
    #[ts(type = "number")]
    pub bytes: u64,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
pub struct WorkerClaudeConfigInstalled {
    /// What was left out (an MCP server that cannot run there), for the transfer's log.
    pub warnings: Vec<String>,
}
