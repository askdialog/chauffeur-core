//! The import from Claude Code (`claudecode.import`, docs/adr/claude-code-import.md): which
//! sessions it reads and where each lands: a workspace on a live branch, a parent with its
//! children, a workspace carrying a clone's uncommitted default-branch changes, or, for those that
//! left no live branch, their repo's history exploration.

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
    /// A released checkout's changes, once copied, are also saved in a git ref kept as an
    /// archive's snapshot is (30 days by default), and the checkout goes back to its default
    /// branch.
    #[serde(default)]
    pub move_changes: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ClaudeCodeImportResult {
    pub dry_run: bool,
    /// Asked with `move_changes`: the released checkouts are cleaned.
    #[serde(default)]
    pub move_changes: bool,
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
    /// The repos of its workspaces that have no setup script: one guessed from their lockfiles.
    #[serde(default)]
    pub setups: Vec<ClaudeCodeSetup>,
    pub warnings: Vec<String>,
}

/// A setup script guessed for a repo that has none (committed, or in the user's settings).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[ts(export)]
pub struct ClaudeCodeSetup {
    pub repo: String,
    /// The command its root lockfiles give (`npm install`…); none when its only lockfiles are in
    /// subfolders.
    pub command: Option<String>,
    /// The lockfiles it was read from, at the root; with no command, those in subfolders.
    pub lockfiles: Vec<String>,
}

/// A workspace's outcome from a daemon that plans only.
fn planned() -> ClaudeCodeOutcome {
    ClaudeCodeOutcome::Import
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ClaudeCodeOutcome {
    /// Will be imported (a dry run).
    Import,
    /// Imported: a session's chat is in `lands_in` (a closed tab there for a history
    /// exploration); a workspace was made (`workspace`).
    Imported,
    /// Imported by an earlier run: left as it is.
    AlreadyImported,
    /// Could not be imported: `reason` says why; the others went on.
    Failed,
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
    /// Where it lands: a workspace's name, or `history of <repo>` (the exploration's name once
    /// imported).
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
    /// `import` in a plan; `imported` or `failed` once run, `reason` saying why it failed, or
    /// `already_imported` for changes an earlier import carried. (Defaults: a daemon from before
    /// plans only.)
    #[serde(default = "planned")]
    pub outcome: ClaudeCodeOutcome,
    #[serde(default)]
    pub reason: Option<String>,
    /// The workspace made, or the one an earlier import made on that branch, by name.
    #[serde(default)]
    pub workspace: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ClaudeCodeHistoryPlan {
    pub repo: String,
    /// The base branch it is checked out at.
    pub base: String,
    /// Its sessions, resumable read-only: the latest an open tab, the others closed.
    pub sessions: Vec<String>,
    /// The exploration they go to: one an earlier import made, while it is not archived, or the
    /// one made now; none when a dry run would make one.
    pub workspace: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[ts(export)]
pub struct ClaudeCodeRelease {
    pub repo: String,
    pub branch: String,
    /// The checkout holding it: the clone or one of the user's worktrees.
    pub folder: String,
    /// Its folder is gone, or half deleted under a temporary folder (a cleanup cut short): the
    /// branch's commits come, never its changes.
    #[serde(default)]
    pub commits_only: bool,
    /// Moved out (`move_changes`): the ref its changes are saved in, and the branch (or commit)
    /// it went back to.
    #[serde(default)]
    pub moved: Option<ClaudeCodeMoved>,
}

/// Where a released checkout's changes went with `move_changes`.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[ts(export)]
pub struct ClaudeCodeMoved {
    pub saved_in: String,
    pub back_on: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_from_a_daemon_that_only_planned_still_reads() {
        let workspace: ClaudeCodeWorkspacePlan = serde_json::from_str(
            r#"{"name":"w","kind":"branch","branch":"b","repos":["r"],"parent":null,"sessions":[],"changes_from":[]}"#,
        )
        .unwrap();
        assert_eq!(workspace.outcome, ClaudeCodeOutcome::Import);
        assert!(workspace.reason.is_none() && workspace.workspace.is_none());
        let release: ClaudeCodeRelease =
            serde_json::from_str(r#"{"repo":"r","branch":"b","folder":"/f"}"#).unwrap();
        assert!(!release.commits_only);
    }
}
