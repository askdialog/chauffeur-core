//! The import from Conductor (`conductor.import`, docs/adr/conductor-import.md): what it plans and
//! what it did. Variables are named, never valued.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct ConductorImportParams {
    /// Conductor's database; by default the one of Conductor's app on this Mac.
    #[serde(default)]
    pub db: Option<String>,
    /// The Conductor workspaces to import, by name, folder, branch or id; empty: every active one
    /// (and every archived one with `include_archived`).
    #[serde(default)]
    pub workspaces: Vec<String>,
    /// Archived Conductor workspaces too, as archived workspaces holding their chat history.
    #[serde(default)]
    pub include_archived: bool,
    /// Plan only: nothing changes, neither here nor in Conductor.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ConductorImportResult {
    pub dry_run: bool,
    /// The database read.
    pub db: String,
    pub repos: Vec<ConductorRepoPlan>,
    pub workspaces: Vec<ConductorWorkspacePlan>,
    /// Archived Conductor workspaces left out, without `include_archived` or names.
    #[ts(type = "number")]
    pub archived_left_out: usize,
}

/// Which clone a Conductor repo uses in Chauffeur.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ConductorClone {
    /// Conductor's clone, its folder joining Chauffeur's clone folders.
    Conductor,
    /// Chauffeur already uses Conductor's clone.
    Shared,
    /// Chauffeur's own clone, another one: branches are fetched from Conductor's into it.
    Chauffeur,
    /// Not imported (no GitHub remote, a clone that is gone): see `skipped`.
    None,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ConductorRepoPlan {
    /// Conductor's name for it.
    pub conductor_name: String,
    /// Chauffeur's: `name`, or `owner/name` outside the main organization; none when skipped.
    pub name: Option<String>,
    /// Conductor's clone.
    pub conductor_path: String,
    pub clone: ConductorClone,
    /// The clone Chauffeur uses.
    pub clone_path: Option<String>,
    /// Scripts typed in Conductor's UI, going to `[repos.<name>.scripts]`: `setup`, `run`,
    /// `archive`, `run_mode`.
    pub scripts: Vec<String>,
    /// Names of the variables going to `[repos.<name>.env]`.
    pub env: Vec<String>,
    /// Why the repo is not imported.
    pub skipped: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ConductorOutcome {
    /// Will be imported (a dry run).
    Import,
    Imported,
    /// An earlier run imported it: `workspace` names the Chauffeur workspace.
    AlreadyImported,
    /// Not imported: `reason` says why.
    Skipped,
    /// Its import failed: `reason` says why; Conductor's side was put back.
    Failed,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ConductorWorkspacePlan {
    pub conductor_id: String,
    pub name: String,
    /// Its Conductor repo's Chauffeur name.
    pub repo: Option<String>,
    pub branch: String,
    /// Its folder in Conductor; none for an archived one.
    pub path: Option<String>,
    pub archived: bool,
    /// The Chauffeur workspace's repos: its own, then its `/add-dir` folders'.
    pub repos: Vec<String>,
    /// Its branch is released in Conductor (a detached HEAD on the same commit there).
    pub release: bool,
    /// Uncommitted files carried over, tracked and untracked.
    #[ts(type = "number")]
    pub changes: usize,
    /// Chats written as Markdown into `.chauffeur/conductor/`.
    #[ts(type = "number")]
    pub chats: usize,
    /// The Linear issue its branch names, attached when Linear has it.
    pub ticket: Option<String>,
    pub outcome: ConductorOutcome,
    pub reason: Option<String>,
    /// The Chauffeur workspace's slug, once imported.
    pub workspace: Option<String>,
    pub warnings: Vec<String>,
}
