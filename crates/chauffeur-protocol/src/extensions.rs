//! Official packages, their independent lifecycle, and the versioned host/UI contracts.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ExtensionId {
    Database,
    Office,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExtensionInfo {
    pub id: ExtensionId,
    pub name: String,
    pub description: String,
    pub publisher: String,
    pub version: String,
    pub enabled: bool,
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub package: Option<ExtensionPackageInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExtensionUi {
    pub revision: String,
    pub base_url: String,
    pub script: String,
    pub stylesheet: Option<String>,
    pub assets: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ExtensionPackagePhase {
    #[default]
    Missing,
    Installed,
    Downloading,
    Waiting,
    Starting,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExtensionPackageInfo {
    pub desired_installed: bool,
    pub installed: bool,
    pub auto_update: bool,
    pub available: Option<String>,
    pub phase: ExtensionPackagePhase,
    pub error: Option<String>,
    pub ui: Option<ExtensionUi>,
    pub can_restore: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExtensionConfigureParams {
    pub id: ExtensionId,
    pub auto_update: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExtensionWatchParams {
    pub id: ExtensionId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ExtensionCommand {
    Check,
    Install,
    Update,
    Remove,
    Restore,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExtensionManageParams {
    pub id: ExtensionId,
    pub command: ExtensionCommand,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExtensionCallParams {
    pub id: ExtensionId,
    pub method: String,
    pub params: serde_json::Value,
    #[serde(default)]
    pub workspace: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExtensionRelease {
    pub manifest_version: u32,
    pub id: ExtensionId,
    pub version: String,
    pub requires_core: String,
    pub host_api: u32,
    pub ui_api: u32,
    pub state_format: u32,
    pub targets: Vec<ExtensionReleaseTarget>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExtensionReleaseTarget {
    pub platform: String,
    pub archive: String,
    pub sha256: String,
    pub bytes: u32,
    pub executable: String,
    pub script: String,
    pub stylesheet: Option<String>,
    pub assets: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExtensionSetParams {
    pub id: ExtensionId,
    pub enabled: bool,
}
