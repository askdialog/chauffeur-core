use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A repository a workspace can include: one of a GitHub owner the account reaches, or a clone
/// only found on this machine.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct RepoInfo {
    /// Short name, e.g. `web`. Also the worktree's folder name inside a workspace.
    pub name: String,
    /// What names this repo in a request (`workspace.create`, `workspace.add_repo`): `name` for
    /// the main organization's repos and the local ones, `owner/name` for the others.
    #[serde(default)]
    pub key: String,
    /// `owner/name` on GitHub.
    pub full_name: String,
    /// The GitHub owner (an account or an organization); empty for a clone only found here.
    #[serde(default)]
    pub owner: String,
    /// The owner is an organization (GitHub draws it square), not a person.
    #[serde(default)]
    pub owner_is_org: bool,
    #[serde(default)]
    pub private: bool,
    /// A clone under the clone folders that no GitHub owner lists.
    #[serde(default)]
    pub local: bool,
    /// Local clone the worktrees are created from, when one exists.
    pub clone_path: Option<String>,
    pub default_branch: Option<String>,
    pub description: Option<String>,
    pub pushed_at: Option<String>,
    /// When a workspace last took it in (RFC 3339): the picker's Recent.
    #[serde(default)]
    pub last_used: Option<String>,
    /// Part of the user's default selection for new workspaces.
    pub is_default: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum WorkspaceState {
    Creating,
    Ready,
    Archiving,
    Archived,
    /// An archive script or a cleanup step failed: leftovers may remain on disk.
    CleanupFailed,
}

impl WorkspaceState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "creating",
            Self::Ready => "ready",
            Self::Archiving => "archiving",
            Self::Archived => "archived",
            Self::CleanupFailed => "cleanup_failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "creating" => Self::Creating,
            "ready" => Self::Ready,
            "archiving" => Self::Archiving,
            "archived" => Self::Archived,
            "cleanup_failed" => Self::CleanupFailed,
            _ => return None,
        })
    }
}

/// What the agents of a workspace are doing, folded over its chats.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Activity {
    Idle,
    Working,
    NeedsInput,
    Error,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct TicketRef {
    /// Linear identifier, e.g. `XYZ-1374`.
    pub identifier: String,
    pub title: Option<String>,
    pub url: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum WorktreeOrigin {
    /// Created with the workspace, or added to it by the user.
    Workspace,
    /// Created by an agent through `chauffeur worktree add` or Claude's WorktreeCreate hook.
    Agent,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct WorktreeInfo {
    pub id: String,
    pub repo: String,
    pub path: String,
    pub branch: String,
    /// Commit the branch was cut from.
    pub base_commit: Option<String>,
    /// Branch the worktree was cut from and PRs target, e.g. `staging`.
    pub base_branch: String,
    pub origin: WorktreeOrigin,
    /// Groups the worktrees an agent created together (one per repo).
    pub group: Option<String>,
    pub removed: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct WorkspaceInfo {
    pub id: String,
    /// Folder name, frozen at creation, e.g. `xyz-1374-facet-filters`.
    pub slug: String,
    /// The name shown, when it is not the slug: drawn from the first message of a workspace
    /// created without ticket or name, or given by the user (`workspace.rename`).
    #[serde(default)]
    pub title: Option<String>,
    pub path: String,
    /// Branch shared by the workspace's own worktrees; empty in an exploration.
    pub branch: String,
    pub parent_id: Option<String>,
    pub state: WorkspaceState,
    pub activity: Activity,
    /// An exploration: each repo checked out, detached, at its base branch; read-only chats.
    #[serde(default)]
    pub explore: bool,
    /// Local date of the automatic session review, independent of its displayed name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub daily_review_date: Option<String>,
    pub tickets: Vec<TicketRef>,
    pub worktrees: Vec<WorktreeInfo>,
    /// First port of the workspace's block (`CHAUFFEUR_PORT`).
    pub port_base: u16,
    pub port_count: u16,
    pub created_at: String,
    pub archived_at: Option<String>,
    /// Why the last archive or creation step failed.
    pub last_error: Option<String>,
    /// A chat finished, failed or asked for input since the user last looked at the workspace
    /// (`workspace.seen`).
    #[serde(default)]
    pub unread: bool,
    /// Kept at the top of the sidebar.
    #[serde(default)]
    pub pinned: bool,
    /// Its place in the sidebar, highest first within the pinned and the other workspaces: a new
    /// workspace goes on top, the user drags them around (`workspace.move`).
    #[serde(default)]
    pub position: f64,
    /// Its house's place in the Office once the user moved one (`workspace.move_house`): houses
    /// go by it from the plaza outward; one without it (none moved since it was made) comes
    /// after, by creation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub house_position: Option<u32>,
    /// Tasks its chats' agents run now (the sum of their `running_tasks`).
    #[serde(default)]
    pub running_tasks: u32,
    /// The sidebar section it is filed in (`SectionInfo.id`); none: the "Workspaces" list. A
    /// pinned workspace keeps its section, shown in it again once unpinned; an archived one keeps
    /// it for its restore.
    #[serde(default)]
    pub section_id: Option<String>,
    /// That section's name.
    #[serde(default)]
    pub section: Option<String>,
    /// Transferred away: it runs on that machine now, and stays here read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub moved_to: Option<String>,
    /// RFC 3339: a moved workspace that was reminded of after an hour without activity, idle
    /// since then; none once something happens there or the user asked to be reminded later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub idle_since: Option<String>,
    /// Its encrypted backups, while it runs on a GCP worker (or ran there last).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub backup: Option<Box<crate::cloud_backup::BackupStatus>>,
}

/// A user-named group of workspaces in the sidebar, between the pinned workspaces and the others
/// (`section.*`).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct SectionInfo {
    pub id: String,
    pub name: String,
    /// Folded in the sidebar: its workspaces hidden, ⌘1–9 skipping them.
    #[serde(default)]
    pub folded: bool,
    // The daemon-owned reviews section, independent of its user-editable name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reviews: Option<bool>,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AgentKind {
    #[default]
    Claude,
    Codex,
}

impl AgentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PermissionMode {
    /// Claude Code's auto mode: a classifier runs what it judges safe and blocks the rest, asking
    /// the user when it cannot decide. The default for new chats (decided with Justin 2026-10-17).
    #[default]
    Auto,
    Default,
    AcceptEdits,
    Plan,
    BypassPermissions,
}

impl PermissionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Default => "default",
            Self::AcceptEdits => "accept_edits",
            Self::Plan => "plan",
            Self::BypassPermissions => "bypass_permissions",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "auto" => Self::Auto,
            "default" => Self::Default,
            "accept_edits" => Self::AcceptEdits,
            "plan" => Self::Plan,
            "bypass_permissions" => Self::BypassPermissions,
            _ => return None,
        })
    }
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ChatStatus {
    Starting,
    Idle,
    Working,
    /// Waiting on a permission request or a question.
    NeedsInput,
    Error,
    /// The workspace was archived; the chat is read-only.
    Closed,
}

impl ChatStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Idle => "idle",
            Self::Working => "working",
            Self::NeedsInput => "needs_input",
            Self::Error => "error",
            Self::Closed => "closed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "starting" => Self::Starting,
            "idle" => Self::Idle,
            "working" => Self::Working,
            "needs_input" => Self::NeedsInput,
            "error" => Self::Error,
            "closed" => Self::Closed,
            _ => return None,
        })
    }
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ChatInfo {
    pub id: String,
    pub workspace_id: String,
    pub agent: AgentKind,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Fast mode: the model's faster, pricier service tier, when it has one.
    #[serde(default)]
    pub fast: bool,
    pub permission_mode: PermissionMode,
    pub title: Option<String>,
    pub status: ChatStatus,
    /// The agent's own session id, used to resume after a restart.
    pub agent_session_id: Option<String>,
    pub created_at: String,
    /// Tasks of the agent running now (background commands, subagents), between their
    /// `task_started` and `task_ended` events. Kept by the daemon in memory.
    #[serde(default)]
    pub running_tasks: u32,
    /// The permission mode the agent reported running in, when it did since the daemon started
    /// (kept in memory). It differs from `permission_mode` when Claude Code fell back from auto to
    /// `default` because auto is unavailable for the chat (decided with Justin 2026-10-18).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub permission_mode_effective: Option<PermissionMode>,
    /// Closed by the user: out of the workspace's tabs, its conversation kept, reopened on demand.
    /// Not the `closed` status, which is a chat of an archived workspace.
    #[serde(default)]
    pub closed: bool,
    /// RFC 3339: when the chat last had an event stored (its creation, before any).
    #[serde(default)]
    pub last_activity_at: Option<String>,
    /// The MCP servers of the chat's agent session and where each stands, as the agent last
    /// reported them since the daemon started (kept in memory); empty before it did.
    #[serde(default)]
    pub mcp_servers: Vec<McpServer>,
    /// A chat that codes, or a design chat (story 47); fixed at creation.
    #[serde(default)]
    pub kind: ChatKind,
    /// The chat a design chat was started from: its design brief summarizes it, its screens are
    /// sent back to it. Kept as an id once that chat is closed or deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source_chat: Option<String>,
    /// A design chat's design brief, until the chat's first message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub brief: Option<DesignBrief>,
    /// A delegated chat (story 49): the chat whose agent started it with its `delegate` tool, and
    /// to which its answers go back. Kept as an id once that chat is closed or deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub parent_chat: Option<String>,
    /// The parent's tool call that started this chat, for the parent's chat to link to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub parent_tool_use: Option<String>,
    /// How far the agent is through its own plan or task list, when it keeps one (Codex's plan,
    /// Claude Code's todos or tasks): steps done out of all. Kept by the daemon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub progress: Option<ChatProgress>,
    /// Its tab's place among its workspace's chats once the user moved one (`chat.move`). Tabs
    /// go by it; a chat without one (none moved since it was made) comes after, by creation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tab_position: Option<u32>,
    /// A fork or switch into the other agent: the agent its conversation was held with. Until the chat's
    /// own agent has a session, each message handed over begins with that conversation as text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub forked_from_agent: Option<AgentKind>,
}

/// Steps done out of all, in an agent's plan.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[ts(export)]
pub struct ChatProgress {
    pub done: u32,
    pub total: u32,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ChatKind {
    /// The agent works on the code.
    #[default]
    Chat,
    /// The agent designs screens: it reads the repos and writes only into the chat's design
    /// folder of the notes, shown on a canvas. Claude Code or Codex.
    Design,
}

impl ChatKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Design => "design",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "chat" => Some(Self::Chat),
            "design" => Some(Self::Design),
            _ => None,
        }
    }
}

/// The LLM-written summary of a source chat that a design chat starts from: it fills the design
/// chat's composer, for the user to edit and send.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct DesignBrief {
    pub state: BriefState,
    /// The brief, once written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub text: Option<String>,
    /// Why it could not be written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum BriefState {
    Writing,
    Ready,
    Failed,
}

/// An MCP server of an agent session (a claude.ai connector, a server of `.mcp.json` or of
/// Codex's config) and whether the agent reaches it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct McpServer {
    /// As the agent names it (`claude.ai Linear`, `linear`).
    pub name: String,
    pub status: McpStatus,
    /// Why it failed, when the agent says.
    pub error: Option<String>,
    /// A tool call of the agent failed on it while it was not connected: a chat needs it.
    #[serde(default)]
    pub failed_call: bool,
    /// Environment variables it needs that the agent's environment lacks (unset or empty), by
    /// name only: exported from the login shell's profile, they reach the agent once the
    /// environment is reloaded (`env.reload`). Never their values.
    #[serde(default)]
    pub missing_env: Vec<String>,
    /// Every environment variable its declaration takes a value from, set or not (`missing_env`
    /// among them), by name only, when it is broken: reloading the environment restarts its chat
    /// only when one of them was set or changed.
    #[serde(default)]
    pub uses_env: Vec<String>,
    /// Where it is declared, when it lacks variables and the agent says, for the user to remove
    /// it there when it is not wanted: `~/.codex/config.toml [mcp_servers.slack]`,
    /// `~/.claude.json`, a repo's `.mcp.json`.
    #[serde(default)]
    pub declared_in: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum McpStatus {
    Connected,
    /// Still connecting.
    Pending,
    /// Its login expired or never happened: the user must sign in again.
    NeedsAuth,
    Failed,
    /// Turned off by the user.
    Disabled,
}

impl McpStatus {
    /// Something the user should fix: a login to redo, a server that does not answer.
    pub fn is_broken(self) -> bool {
        matches!(self, McpStatus::NeedsAuth | McpStatus::Failed)
    }
}

/// Whether an agent's own CLI is logged in to its account.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AccountStatus {
    SignedIn,
    /// Logged out, or its credentials were refused: every chat of the agent fails until the user
    /// signs in again.
    SignedOut,
    /// Not read yet, or the read failed for another reason (the network, a timeout).
    #[default]
    Unknown,
}

/// One question of Claude's AskUserQuestion tool.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct Question {
    pub question: String,
    pub header: String,
    pub options: Vec<QuestionOption>,
    pub multi_select: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
    /// What the option looks like, in markdown (a code snippet, a mockup), shown beside the
    /// options while it is focused. Claude's AskUserQuestion only; absent for Codex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub preview: Option<String>,
}

/// The user's answer to a permission request or a question.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum RequestResponse {
    /// Allow the tool call; `always` keeps allowing it for this chat.
    Allow {
        always: bool,
    },
    Deny {
        message: Option<String>,
    },
    /// An MCP confirmation; `always` (on an accepted one offering a rule) keeps accepting the same
    /// confirmation for this chat.
    Elicitation {
        action: ElicitationAction,
        content: serde_json::Value,
        #[serde(default)]
        always: bool,
    },
    /// Answers keyed by question text.
    Answer {
        answers: std::collections::BTreeMap<String, String>,
    },
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ElicitationAction {
    Accept,
    Decline,
    Cancel,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TurnOutcome {
    Success,
    Error,
    Interrupted,
}

/// A terminal of a workspace, run by the daemon in a pseudo-terminal.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct TerminalInfo {
    pub id: String,
    pub workspace_id: String,
    /// The folder it started in (`terminal.cwd` reads where its shell is now).
    pub cwd: String,
    /// The repo or the workspace it started in.
    pub title: String,
    /// The name the user gave its tab (`terminal.rename`), shown instead of `title`; kept for as
    /// long as the terminal lives.
    #[serde(default)]
    pub name: Option<String>,
    pub cols: u16,
    pub rows: u16,
    /// The shell is alive.
    pub running: bool,
    pub exit_code: Option<i32>,
    /// A command runs in the shell's foreground (seen for a second at least): the terminal's
    /// foreground process group is not the shell's.
    pub command_running: bool,
    /// When the last command seen running gave the terminal back to the shell.
    pub command_finished_at: Option<String>,
}

/// A Linear project milestone, offered with the tickets: picked, its open issues are attached.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct LinearMilestone {
    pub id: String,
    pub name: String,
    /// The project's name.
    pub project: String,
    /// `YYYY-MM-DD`.
    pub target_date: Option<String>,
    /// Its issues not completed nor canceled: those a workspace attaches.
    pub open_issues: u32,
}

/// A Linear issue as Chauffeur uses it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct LinearIssue {
    pub identifier: String,
    pub title: String,
    pub url: String,
    /// The git branch name Linear suggests for the issue.
    pub branch_name: Option<String>,
    pub state: Option<String>,
    /// The state's kind: `triage`, `backlog`, `unstarted`, `started`, `completed` or `canceled`.
    #[serde(default)]
    pub state_type: Option<String>,
    pub team: Option<String>,
    pub assignee: Option<String>,
    pub description: Option<String>,
    /// Repos named in the description's `## Repo` section.
    pub repos: Vec<String>,
    pub children: Vec<TicketRef>,
    pub parent: Option<TicketRef>,
}

/// A workflow state of a Linear team, an issue's status.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct LinearState {
    pub id: String,
    pub name: String,
    /// `triage`, `backlog`, `unstarted`, `started`, `completed` or `canceled`.
    #[serde(rename = "type")]
    pub kind: String,
    /// As Linear shows it, e.g. `#f2c94c`.
    pub color: String,
}

/// A model from an agent's own catalog (Claude Code's `supportedModels`, Codex's `model/list`).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct AgentModel {
    /// What a chat's `model` takes, e.g. `opus` or `gpt-6.1-sol`.
    pub id: String,
    /// The model id the agent resolves `id` to (`opus` → `claude-opus-5-5`), which a chat reports
    /// once its session starts.
    pub resolved: Option<String>,
    /// Display name, e.g. `Opus 5.5`.
    pub name: String,
    pub description: String,
    /// Effort levels the model accepts, lowest first; empty when it takes none.
    pub efforts: Vec<String>,
    /// The effort the agent uses when a chat sets none, when the agent says.
    pub default_effort: Option<String>,
    /// Whether the model has a fast mode (a faster, pricier service tier).
    pub supports_fast: bool,
    /// The model a chat without a model runs on.
    pub is_default: bool,
}

/// One agent's models, or why they could not be listed.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct AgentModels {
    pub agent: AgentKind,
    pub models: Vec<AgentModel>,
    pub error: Option<String>,
}

/// A favorite model setting, picked in one keystroke from the model picker.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
#[ts(optional_fields)]
pub struct LoadoutEntry {
    pub agent: AgentKind,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default)]
    pub fast: bool,
}

/// A file to attach to a message (an image, or any other file: a zip, an HTML page): its bytes,
/// or a file on the daemon's machine. Images are shown to the agent; other files are named to it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default, PartialEq)]
#[ts(export)]
#[ts(optional_fields)]
pub struct ImageInput {
    /// The name it is shown under, e.g. `Screenshot 2026-10-13 at 10.27.png`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The type the client gives. An image's is read from its bytes; another file's comes from
    /// its name's extension, else this, else `application/octet-stream`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// The bytes, base64-encoded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// An absolute path the daemon copies the image from (local clients such as the CLI).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// A file attached to a user message (or an image a tool returned), kept in the workspace's notes
/// folder (`.chauffeur/attachments/`), so it outlives the worktrees.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct Attachment {
    /// File name in the attachments folder; `/attachments/<workspace>/<file>` serves it when it is
    /// an image.
    pub file: String,
    /// The name it was attached under.
    pub name: String,
    pub media_type: String,
    #[ts(type = "number")]
    pub size: u64,
    /// Absolute path, for the agent.
    pub path: String,
}

/// A command a chat runs when a message starts with `/name`: a skill or a custom command (the
/// user's, a plugin's, a repo's), or one of the agent's own.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct SlashCommand {
    /// Without the slash, e.g. `acme-skills:implement-ticket`.
    pub name: String,
    pub description: String,
    /// What follows the name, e.g. `<ticket>`; empty when it takes nothing in particular.
    pub argument_hint: String,
    /// One of the agent's own commands, rather than a skill or command someone wrote.
    pub builtin: bool,
}

/// What an app the daemon can open a folder in is for.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum OpenAppKind {
    Editor,
    Terminal,
    Files,
}

/// An app installed on the daemon's machine that opens folders (Cursor, Ghostty, the file manager…).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct OpenApp {
    /// `cursor`, `vscode`, `zed`, `ghostty`…, and `finder` on macOS, `files` on Linux.
    pub id: String,
    pub name: String,
    pub kind: OpenAppKind,
    /// The app used when none is named: the last editor or terminal used, else the first editor.
    pub is_default: bool,
}

/// How a message sent while the agent works is delivered.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SendMode {
    /// Into the running turn, read at the agent's next step.
    #[default]
    Steer,
    /// Held in the chat's queue until the turn ends, then sent as the next turn.
    Queue,
}

/// A message waiting in a chat's queue.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct QueuedMessage {
    pub id: String,
    pub text: String,
    pub attachments: Vec<Attachment>,
    pub created_at: String,
}

/// Whether Chauffeur keeps the machine awake while its agents work (`keep_awake` in
/// `settings.toml`).
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum KeepAwake {
    /// The machine sleeps as it would without Chauffeur.
    Never,
    /// No idle sleep while an agent works; the display may sleep, closing a MacBook's lid still
    /// sleeps it.
    #[default]
    WhileWorking,
    /// macOS only: sleep turned off (`pmset disablesleep`) while an agent works, even with the lid
    /// closed, through a sudoers rule installed once with an administrator's password.
    LidClosed,
}

impl KeepAwake {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::WhileWorking => "while_working",
            Self::LidClosed => "lid_closed",
        }
    }
}

/// Whether the errors the app shows as toasts are reported to Chauffeur's Sentry
/// (`error_reports` in `settings.toml`; docs/adr/error-reports-to-sentry.md).
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ErrorReports {
    /// Each error toast is reported on its own.
    Always,
    /// Each error toast offers to send its report.
    #[default]
    Ask,
    /// Nothing is reported.
    Never,
}

impl ErrorReports {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Ask => "ask",
            Self::Never => "never",
        }
    }
}

/// What became of an error report.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ErrorReportOutcome {
    Sent,
    /// Sentry could not be reached: sent later, within 7 days.
    Queued,
    /// The same error was reported less than 10 minutes ago.
    Duplicate,
    /// 50 reports were sent today.
    Limited,
}

/// What the daemon holds against sleep now.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PowerHold {
    /// Nothing: the machine sleeps as usual.
    #[default]
    None,
    /// No idle sleep (a power assertion); the display may sleep, the lid still sleeps a MacBook.
    IdleSleep,
    /// Sleep turned off altogether, the lid closed included (`pmset disablesleep 1`).
    AllSleep,
}

/// Whether Chauffeur keeps the machine awake now, and why (`power.get`).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Default)]
#[ts(export)]
pub struct PowerState {
    /// The setting in force.
    pub keep_awake: KeepAwake,
    pub holding: PowerHold,
    /// Chats whose agent works: a turn running, background tasks, or messages waiting for their
    /// workspace to be ready.
    pub working_chats: u32,
    /// Tasks the agents run in the background.
    pub running_tasks: u32,
    /// Workspaces being created (fetch, worktrees, setup scripts).
    pub creating_workspaces: u32,
    /// Seconds the hold is kept after the last work ended, to bridge the gap to the next turn.
    pub lingering_secs: Option<u32>,
    /// In lid-closed mode: sleep was given back because the battery is low.
    pub low_battery: bool,
    /// The battery's charge, when there is a battery and it was read (lid-closed mode only).
    pub battery_percent: Option<u8>,
    /// Something did not work (the sudoers rule is missing, no way to hold sleep on this system).
    pub warning: Option<String>,
}

/// A newer packaged Chauffeur and where its installation stands (`update.status`). A build from
/// source, or a daemon that could not ask GitHub (no `gh`, no network, no access), has none.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Default)]
#[ts(export)]
pub struct UpdateStatus {
    /// This daemon's version, `X.Y.Z`.
    pub current: String,
    /// The latest published version when it is newer than this one, `X.Y.Z`.
    pub available: Option<String>,
    pub phase: UpdatePhase,
    /// Why the last installation failed.
    pub error: Option<String>,
    /// The page of this build's published releases; none for a build without a release
    /// repository (`CHAUFFEUR_RELEASE_REPO`).
    #[serde(default)]
    pub releases_url: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum UpdatePhase {
    #[default]
    Idle,
    /// The disk image is being downloaded and checked against its published SHA-256.
    Downloading,
    /// The new app is being copied in place of this one.
    Installing,
    /// The new app is in place: quitting and reopening it starts the new daemon.
    Installed,
    Failed,
}
