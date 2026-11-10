//! PostgreSQL connections and bounded SQL results; passwords are write-only inputs.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DatabaseEnvironment {
    Local,
    Staging,
    Production,
    Other,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DatabaseTls {
    Disable,
    VerifyFull,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct DatabaseConnection {
    pub id: String,
    pub name: String,
    pub workspace: Option<String>,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub environment: DatabaseEnvironment,
    pub tls: DatabaseTls,
    pub read_only: bool,
    pub has_password: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct DatabaseSaveParams {
    pub id: Option<String>,
    pub name: String,
    pub workspace: Option<String>,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub environment: DatabaseEnvironment,
    pub tls: DatabaseTls,
    /// None keeps an existing password; an empty string clears it.
    pub password: Option<String>,
    /// None defaults to read-only for production.
    pub read_only: Option<bool>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct DatabaseListParams {
    pub workspace: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct DatabaseParams {
    pub connection: String,
    pub workspace: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct DatabaseQueryParams {
    pub connection: String,
    pub workspace: String,
    pub request: String,
    pub sql: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct DatabaseCancelParams {
    pub request: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct DatabaseResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    #[ts(type = "number")]
    pub affected: u64,
    pub truncated: bool,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct DatabaseColumn {
    pub name: String,
    pub data_type: String,
    pub primary_key: bool,
    pub editable: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct DatabaseTable {
    pub schema: String,
    pub name: String,
    pub kind: String,
    pub columns: Vec<DatabaseColumn>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct DatabaseCellChange {
    pub column: String,
    pub value: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct DatabaseRowChange {
    /// Original complete row, used for optimistic concurrency checks.
    pub original: Vec<Option<String>>,
    pub cells: Vec<DatabaseCellChange>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct DatabaseApplyParams {
    pub connection: String,
    pub workspace: String,
    pub request: String,
    pub schema: String,
    pub table: String,
    pub changes: Vec<DatabaseRowChange>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct DatabaseHistoryEntry {
    pub sql: String,
    pub at: String,
}
