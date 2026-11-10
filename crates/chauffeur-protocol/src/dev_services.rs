//! A workspace's development services (docs/workspace-development-services.md): the Docker
//! containers that work or write in it, detected, then confirmed by the user as data that follows
//! the workspace to another machine or as a cache rebuilt there; and its web previews, the ports
//! of its block that answer HTTP, confirmed the same way.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The compose file and service a container was started from; a destination starts the service
/// again from them.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ComposeService {
    /// The compose project, for another machine to start the service again under, so that the
    /// repo's own `docker compose` commands find it there. Empty in declarations saved before it
    /// was recorded, which keep their exact encoding: detect and save again to record it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project: String,
    /// The compose file, relative to the workspace folder (`shop-api/docker-compose.yml`).
    pub file: String,
    pub service: String,
}

/// A PostgreSQL server a detected container runs: its user and the databases it holds, each
/// exported when its data follows the workspace.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct DetectedPostgres {
    pub username: String,
    /// Empty while the container is stopped: its databases are read once it runs.
    pub databases: Vec<String>,
}

/// A container that works or writes in the workspace, as detection found it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct DetectedService {
    /// Its compose service, or else its container's name.
    pub name: String,
    /// Its container, short.
    pub container: String,
    /// The image it was started from, as named (`postgres:16`).
    pub image: String,
    pub running: bool,
    pub compose: Option<ComposeService>,
    pub postgres: Option<DetectedPostgres>,
    /// Suggested: its data follows the workspace (a database); otherwise a cache rebuilt there.
    pub data: bool,
    /// Why it cannot be declared as it is, and what to change.
    pub problem: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct DevServicesDetected {
    pub services: Vec<DetectedService>,
    /// The ports of the workspace's block that answer HTTP now, and those declared.
    #[serde(default)]
    pub previews: Vec<DetectedPreview>,
}

/// A port of the workspace's block that answers HTTP, or a declared preview.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct DetectedPreview {
    /// Its declared name, else a suggestion: the run script listening on it, or `port-<port>`.
    pub name: String,
    /// Its port on the machine the workspace runs on.
    pub port: u16,
    /// Its place in the workspace's port block, which is what a declaration keeps.
    pub offset: u16,
    /// Something answers HTTP on it now.
    pub listening: bool,
    pub declared: bool,
}

/// A port declared as a web preview, by its port on the machine the workspace runs on.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct PreviewChoice {
    pub name: String,
    pub port: u16,
}

/// One detected service, confirmed: its data follows the workspace (a PostgreSQL server, every
/// database of it exported), or it is a cache rebuilt on the destination.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct DevServiceChoice {
    pub name: String,
    pub data: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct DevServicesSaveParams {
    pub workspace: String,
    /// Every service to declare; the others are left out of the profile.
    pub services: Vec<DevServiceChoice>,
    /// The revision these choices were made on; none for a first profile.
    #[ts(type = "number | null")]
    pub revision: Option<u64>,
    /// Every preview to declare; none keeps those declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub previews: Option<Vec<PreviewChoice>>,
}

/// A declared service, as the profile keeps it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct DeclaredService {
    pub name: String,
    pub image: Option<String>,
    pub compose: Option<ComposeService>,
    pub data: bool,
    pub databases: Vec<String>,
}

/// A workspace's declared development services; revision none when it has none yet.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct DevServices {
    #[ts(type = "number | null")]
    pub revision: Option<u64>,
    pub services: Vec<DeclaredService>,
    #[serde(default)]
    pub previews: Vec<DeclaredPreview>,
}

/// A declared web preview: a port of the workspace's block, by its place in it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct DeclaredPreview {
    pub name: String,
    pub offset: u16,
    /// Its port on the machine the workspace runs on; none when its block has no such place.
    pub port: Option<u16>,
}

/// What a declared preview's port does now, on the machine the workspace runs on.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PreviewState {
    /// Something accepts connections on it.
    Ready,
    /// Nothing yet, while a run script of the workspace runs.
    Starting,
    /// Nothing, and the workspace's last run script failed.
    Error,
    /// Nothing, and no run script runs.
    Stopped,
}

/// A declared preview, as `previews.list` gives it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct Preview {
    pub name: String,
    pub offset: u16,
    /// Where it opens on the machine asked: its own port, or for a moved workspace the port of
    /// its block here that reaches it.
    pub port: u16,
    pub state: PreviewState,
}

/// A workspace's declared previews and the machine they run on: none when it runs on the
/// machine asked.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct Previews {
    pub machine: Option<String>,
    pub previews: Vec<Preview>,
}
