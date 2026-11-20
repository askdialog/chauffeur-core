//! The import from Claude Code (`claudecode.import`, docs/adr/claude-code-import.md): which
//! sessions it reads and where each would land. For now it plans only.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct ClaudeCodeImportParams {
    /// Sessions active in the last `since_days` days (60 by default).
    #[serde(default)]
    pub since_days: Option<u32>,
    /// Sessions active since this day (`YYYY-MM-DD`, from its midnight here), over `since_days`.
    #[serde(default)]
    pub since_date: Option<String>,
    /// Every session, whatever its age.
    #[serde(default)]
    pub all: bool,
    /// Plan only: nothing changes.
    #[serde(default)]
    pub dry_run: bool,
    /// Claude Code's folder, when it is not `CLAUDE_CONFIG_DIR` or `~/.claude`.
    #[serde(default)]
    pub claude_dir: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ClaudeCodeImportResult {
    pub dry_run: bool,
    /// The window read, in days; none with `all` or `since_date`.
    pub since_days: Option<u32>,
    /// The day the window starts, when asked by date.
    pub since_date: Option<String>,
    /// Claude Code's folder read.
    pub claude_dir: String,
    /// The sessions found, the most recent first.
    pub sessions: Vec<ClaudeCodeSessionPlan>,
    /// The workspaces the import makes.
    pub workspaces: Vec<ClaudeCodeWorkspacePlan>,
    /// One history exploration per repo, for the sessions that left no live branch.
    pub histories: Vec<ClaudeCodeHistoryPlan>,
    /// Live branches checked out in a clone or one of the user's worktrees, released there.
    pub releases: Vec<ClaudeCodeRelease>,
    pub warnings: Vec<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ClaudeCodeOutcome {
    /// Will be imported (a dry run).
    Import,
    /// Still running in Claude Code: refused until it is closed.
    Running,
    /// Not imported: `reason` says why.
    Skipped,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ClaudeCodeSessionPlan {
    pub id: String,
    pub title: String,
    /// The folder it ran in.
    pub folder: String,
    /// Its last message (RFC 3339).
    pub last_active: String,
    /// The branches it wrote on or created, by repo.
    pub branches: Vec<ClaudeCodeBranch>,
    pub outcome: ClaudeCodeOutcome,
    pub reason: Option<String>,
    /// Where it lands: a workspace's name, or `history of <repo>`.
    pub lands_in: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[ts(export)]
pub struct ClaudeCodeBranch {
    pub repo: String,
    pub branch: String,
    /// It exists and is not merged into the default branch, or its checkout holds changes.
    pub live: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ClaudeCodeWorkspaceKind {
    /// A live branch: on it, its sessions as chats.
    Branch,
    /// A session with two live branches or more: a new branch named from its title, the
    /// session as its chat, one child workspace per branch.
    Parent,
    /// Uncommitted changes on a clone's default branch: a new branch carrying them.
    Changes,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ClaudeCodeWorkspacePlan {
    pub name: String,
    pub kind: ClaudeCodeWorkspaceKind,
    /// The branch it is on; none for a new branch named at import.
    pub branch: Option<String>,
    pub repos: Vec<String>,
    /// The parent workspace, for a branch an orchestrating session wrote.
    pub parent: Option<String>,
    /// The sessions it holds as chats, by id.
    pub sessions: Vec<String>,
    /// Repos whose uncommitted default-branch changes it carries.
    pub changes_from: Vec<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ClaudeCodeHistoryPlan {
    pub repo: String,
    /// The base branch it is checked out at.
    pub base: String,
    /// Its sessions, closed tabs resumable read-only.
    pub sessions: Vec<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[ts(export)]
pub struct ClaudeCodeRelease {
    pub repo: String,
    pub branch: String,
    /// The checkout holding it: the clone or one of the user's worktrees.
    pub folder: String,
}
