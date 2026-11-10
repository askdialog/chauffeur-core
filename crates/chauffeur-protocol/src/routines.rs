//! Routines (story 56, docs/adr/routines.md): a prompt the daemon runs by itself at set times, in
//! a workspace of its own, each run a new chat. A routine is a TOML file (`RoutineSpec`) whose
//! every edit is a version.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::model::{AgentKind, ChatStatus, PermissionMode};

/// A draft runs only by hand; an active routine also runs on its schedule; a paused one was
/// active and runs only by hand again.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RoutineState {
    #[default]
    Draft,
    Active,
    Paused,
}

impl RoutineState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Active => "active",
            Self::Paused => "paused",
        }
    }
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Weekday {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

/// When an active routine runs: each of `times` (`"08:00"`, local time) of each of `days`, or of
/// every `every_days`-th day from `from`.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
#[ts(export)]
pub struct RoutineSchedule {
    pub days: Vec<Weekday>,
    pub times: Vec<String>,
    /// Every this many days instead of on `days` (1: every day), counted from `from`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub every_days: Option<u32>,
    /// `"2026-11-10"`: the first day of an `every_days` schedule.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub from: Option<String>,
}

/// A routine's file, as TOML.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct RoutineSpec {
    pub name: String,
    pub prompt: String,
    /// The repos of the routine's workspace; one missing from it is added when the file is saved.
    #[serde(default)]
    pub repos: Vec<String>,
    #[serde(default)]
    pub agent: AgentKind,
    /// None: the agent's default model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub effort: Option<String>,
    #[serde(default)]
    pub permission_mode: PermissionMode,
    #[serde(default)]
    pub schedule: RoutineSchedule,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RoutineTrigger {
    /// Run now, by the user.
    Manual,
    /// Its schedule, a missed occurrence caught up included.
    Schedule,
}

impl RoutineTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Schedule => "schedule",
        }
    }
}

/// One run: a chat of the routine's workspace.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct RoutineRun {
    pub chat_id: String,
    /// The version of the file it ran.
    pub version: u32,
    pub trigger: RoutineTrigger,
    pub started_at: String,
    /// The run's chat as it stands; none once the chat was deleted.
    pub status: Option<ChatStatus>,
    /// Its tab was closed (a later run closes the finished ones).
    pub closed: bool,
    /// The agent's last message, when it wrote one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_text: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct RoutineInfo {
    pub id: String,
    /// The routine's own workspace, where each run is a new chat.
    pub workspace_id: String,
    /// The chat that drafts and edits the file.
    pub refine_chat: Option<String>,
    /// The chat it was made from, kept as an id once that chat is gone.
    pub source_chat: Option<String>,
    pub state: RoutineState,
    /// The current version of the file, from 1.
    pub version: u32,
    pub spec: RoutineSpec,
    /// The current file, as TOML.
    pub file: String,
    /// RFC 3339: the next occurrence of an active routine's schedule.
    pub next_run_at: Option<String>,
    /// An occurrence is due and waits for the previous run to end.
    pub pending: bool,
    pub last_run: Option<RoutineRun>,
    /// Why the last scheduled run could not start.
    pub error: Option<String>,
    pub created_at: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct RoutineVersion {
    pub version: u32,
    pub file: String,
    pub created_at: String,
}

/// Names a routine by its id, its name, or its workspace's id or slug.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct RoutineParams {
    pub routine: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct RoutineCreateParams {
    /// The chat the routine is made from.
    pub chat: String,
    /// Its name; by default the chat's title.
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct RoutineEditParams {
    pub routine: String,
    /// The whole new file, as TOML.
    pub file: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct RoutineSetActiveParams {
    pub routine: String,
    /// Activate (it runs on its schedule) or pause.
    pub active: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct RoutinesResult {
    pub routines: Vec<RoutineInfo>,
}

/// Its versions, newest first, and its runs, newest first.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct RoutineHistory {
    pub versions: Vec<RoutineVersion>,
    pub runs: Vec<RoutineRun>,
}

/// Where a suggestion of Find routines stands: shown until the user acts on it; an ignored one is
/// never proposed again.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RoutineSuggestionState {
    #[default]
    New,
    /// Kept for later, out of the new ones.
    Later,
    Ignored,
    /// Made into a routine (`routine_id`).
    Created,
}

impl RoutineSuggestionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Later => "later",
            Self::Ignored => "ignored",
            Self::Created => "created",
        }
    }
}

/// One of the user's messages a suggestion comes from.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct RoutineSuggestionRequest {
    pub workspace_id: String,
    /// The workspace's name as the sidebar shows it, kept once the workspace is gone.
    pub workspace: String,
    pub chat_id: String,
    /// RFC 3339.
    pub at: String,
    /// The message, cut to a few hundred characters.
    pub text: String,
    /// The time window it names ("des 3 derniers jours", "every morning"), when it names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub window: Option<String>,
}

/// A routine Find routines proposes, written properly, from the requests it saw.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct RoutineSuggestion {
    pub id: String,
    pub state: RoutineSuggestionState,
    /// Why it should be a routine, in a sentence.
    pub why: String,
    /// The routine proposed: its file as `spec`, and as TOML.
    pub spec: RoutineSpec,
    pub file: String,
    /// The requests it comes from, the newest first.
    pub requests: Vec<RoutineSuggestionRequest>,
    /// The routine made from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub routine_id: Option<String>,
    pub created_at: String,
}

/// An analysis under way.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct RoutineFindRun {
    pub started_at: String,
    pub days: u32,
    pub agent: AgentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub model: Option<String>,
    /// How many of the user's messages it reads.
    pub messages: u32,
}

/// Find routines as it stands: the analysis under way, how the last one ended, and the
/// suggestions waiting for the user (new and later ones).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Default)]
#[ts(export)]
pub struct RoutineFindInfo {
    pub running: Option<RoutineFindRun>,
    /// RFC 3339: when the last analysis ended.
    pub last_ended_at: Option<String>,
    /// Why the last analysis failed.
    pub last_error: Option<String>,
    /// How many suggestions the last analysis made.
    pub last_found: Option<u32>,
    pub suggestions: Vec<RoutineSuggestion>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct RoutineFindParams {
    /// The days back to read the user's messages from.
    pub days: u32,
    /// The agent and model that run the analysis.
    pub agent: AgentKind,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct RoutineSuggestionParams {
    pub suggestion: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct RoutineSuggestionSetParams {
    pub suggestion: String,
    /// `later` or `ignored`.
    pub state: RoutineSuggestionState,
}
