//! The versioned private Core ↔ official engine protocol; never exposed on the client socket.
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const HOST_API_VERSION: u32 = 1;
pub const UI_API_VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ExtensionStatement {
    pub sql: String,
    pub params: Vec<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ExtensionSchema {
    pub schema: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ExtensionKey {
    pub key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ExtensionValue {
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ExtensionWorkspace {
    pub workspace: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ExtensionWorkspaceList {
    pub include_archived: bool,
    pub parent: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ExtensionChatList {
    pub workspace: String,
    pub include_closed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ExtensionOfficeSettings {
    pub enabled: Option<bool>,
    pub url: String,
    pub dev_email: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ExtensionIdentityToken {
    pub id_token: Option<String>,
    pub dev_email: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum ExtensionActivity {
    ChatCreated,
    TurnEnded,
    StepsDone {
        steps: u32,
    },
    PrsRead {
        workspace: String,
        prs: Vec<crate::client::PrInfo>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ExtensionViewers {
    pub count: u32,
}

methods! {
    pub enum ExtensionHostCall / ExtensionHostMethod {
        #[serde(rename = "storage.bootstrap")]
        StorageBootstrap(ExtensionSchema) -> (),
        #[serde(rename = "storage.import_legacy")]
        StorageImportLegacy(crate::client::EmptyParams) -> (),
        #[serde(rename = "storage.query")]
        StorageQuery(ExtensionStatement) -> Vec<Vec<Value>>,
        #[serde(rename = "storage.batch")]
        StorageBatch(Vec<ExtensionStatement>) -> (),
        #[serde(rename = "credentials.create")]
        CredentialsCreate(ExtensionValue) -> (),
        #[serde(rename = "credentials.read")]
        CredentialsRead(ExtensionKey) -> Option<String>,
        #[serde(rename = "credentials.delete")]
        CredentialsDelete(ExtensionKey) -> (),
        #[serde(rename = "workspace.get")]
        WorkspaceGet(ExtensionWorkspace) -> crate::model::WorkspaceInfo,
        #[serde(rename = "workspace.list")]
        WorkspaceList(ExtensionWorkspaceList) -> Vec<crate::model::WorkspaceInfo>,
        #[serde(rename = "chat.list")]
        ChatList(ExtensionChatList) -> Vec<crate::model::ChatInfo>,
        #[serde(rename = "workspace.move_house")]
        WorkspaceMoveHouse(crate::client::MoveHouseParams) -> crate::model::WorkspaceInfo,
        #[serde(rename = "kv.get")]
        KvGet(ExtensionKey) -> Option<String>,
        #[serde(rename = "kv.set")]
        KvSet(ExtensionValue) -> (),
        #[serde(rename = "kv.delete")]
        KvDelete(ExtensionKey) -> (),
        #[serde(rename = "office.settings")]
        OfficeSettings(crate::client::EmptyParams) -> ExtensionOfficeSettings,
        #[serde(rename = "extension.enabled")]
        ExtensionEnabled(crate::client::EmptyParams) -> bool,
        #[serde(rename = "identity.sign_in")]
        IdentitySignIn(crate::client::EmptyParams) -> crate::identity::IdentityState,
        #[serde(rename = "identity.sign_out")]
        IdentitySignOut(crate::client::EmptyParams) -> crate::identity::IdentityState,
        #[serde(rename = "identity.token")]
        IdentityToken(crate::client::EmptyParams) -> ExtensionIdentityToken,
        #[serde(rename = "shell.environment")]
        ShellEnvironment(crate::client::EmptyParams) -> std::collections::BTreeMap<String, String>,
    }
}

methods! {
    pub enum ExtensionEngineNotification / ExtensionEngineNotificationMethod {
        #[serde(rename = "core.event")]
        CoreEvent(Box<crate::events::EventEnvelope>),
        #[serde(rename = "core.activity")]
        CoreActivity(ExtensionActivity),
        #[serde(rename = "extension.activate")]
        Activate(crate::client::EmptyParams),
        #[serde(rename = "extension.activation_changed")]
        ActivationChanged(crate::client::EmptyParams),
        #[serde(rename = "extension.viewers")]
        Viewers(ExtensionViewers),
        #[serde(rename = "identity.changed")]
        IdentityChanged(crate::client::EmptyParams),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Init {
    pub api: u32,
    pub extension: String,
    pub version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Failure {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(formatter)
    }
}
impl std::error::Error for Failure {}
impl Failure {
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            code: 503,
            message: message.into(),
            data: None,
        }
    }
    pub fn internal(error: impl std::fmt::Display) -> Self {
        Self {
            code: -32603,
            message: error.to_string(),
            data: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Frame {
    Ping {
        id: u64,
    },
    Pong {
        id: u64,
    },
    Initialize {
        init: Init,
    },
    Ready {
        api: u32,
        extension: String,
        version: String,
    },
    Call {
        id: u64,
        method: String,
        params: Value,
    },
    Result {
        id: u64,
        outcome: std::result::Result<Value, Failure>,
    },
    HostCall {
        id: u64,
        method: String,
        params: Value,
    },
    HostResult {
        id: u64,
        outcome: std::result::Result<Value, Failure>,
    },
    Notify {
        method: String,
        params: Value,
    },
    Event {
        event: Value,
    },
    Shutdown,
}
