//! JSON-RPC between the daemon and the agent sidecar, one JSON object per line on the sidecar's
//! stdin (daemon → sidecar) and stdout (sidecar → daemon). Both sides send requests: the daemon
//! drives sessions, the sidecar asks the daemon for what only it can do (create a worktree).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::client::ContextDetail;
use crate::events::{ChatEvent, ContextUsage};
use crate::model::{
    AgentKind, AgentModel, Attachment, ChatStatus, PermissionMode, RequestResponse, SlashCommand,
    TurnOutcome, WorkspaceState,
};

methods! {
    /// Requests the daemon sends to the sidecar.
    pub enum SidecarCall / SidecarMethod {
        /// Starts (or resumes) an agent session for a chat. Returns once the session accepts input.
        #[serde(rename = "session.start")]
        SessionStart(Box<SessionStartParams>) -> SessionStartResult,
        /// Sends a user message; queued if the agent is mid-turn. When the session was idle, its
        /// `turn_started` event is emitted before this request (or `session.start` with a prompt)
        /// is answered: the daemon relies on that order to know a turn is running.
        #[serde(rename = "session.send")]
        SessionSend(SessionSendParams) -> Empty,
        #[serde(rename = "session.interrupt")]
        SessionInterrupt(SessionParams) -> Empty,
        /// Answers a `permission_request` or `question` event the session emitted.
        #[serde(rename = "session.respond")]
        SessionRespond(SessionRespondParams) -> Empty,
        #[serde(rename = "session.configure")]
        SessionConfigure(SessionConfigureParams) -> Empty,
        /// Stops one task the session reported with `task_started`; its `task_ended` follows.
        /// Refused by agents without such tasks (Codex).
        #[serde(rename = "session.stop_task")]
        SessionStopTask(SessionTaskParams) -> Empty,
        /// How full the session's context window is now. Refused by agents that read it only after
        /// a turn (Codex), and for a chat without a running session.
        #[serde(rename = "session.context_usage")]
        SessionContextUsage(SessionContextUsageParams) -> SessionContextUsageResult,
        /// Connects one of the session's MCP servers again: starts its login when it needs one (the
        /// result's `auth_url` is for the user to open; the session reports the server connected once
        /// the login completes), reconnects it otherwise.
        #[serde(rename = "session.mcp_connect")]
        SessionMcpConnect(SessionMcpParams) -> McpConnectResult,
        /// Ends the session's process; the chat can be resumed later with its agent session id.
        #[serde(rename = "session.stop")]
        SessionStop(SessionParams) -> SessionStopResult,
        /// Ends the session's process if nothing is lost by it: no turn, no message waiting, no
        /// background task that would wake the agent. The next message resumes the chat.
        #[serde(rename = "session.close_idle")]
        SessionCloseIdle(SessionParams) -> CloseIdleResult,
        /// The models an agent offers, from its own catalog.
        #[serde(rename = "agent.models")]
        AgentModels(AgentModelsParams) -> AgentModelsResult,
        /// The slash commands an agent offers in a folder, as a session started there would.
        #[serde(rename = "agent.commands")]
        AgentCommands(AgentCommandsParams) -> AgentCommandsResult,
        /// A short title for a piece of work described by `text`, from a small Claude model.
        #[serde(rename = "agent.title")]
        AgentTitle(AgentTitleParams) -> AgentTitleResult,
        /// A workspace's title again, from its conversation so far: the current one while it still
        /// names the work, else a new one, from a small Claude model.
        #[serde(rename = "agent.retitle")]
        AgentRetitle(AgentRetitleParams) -> AgentTitleResult,
        /// A design brief written from a source chat's conversation, by the model given (Claude).
        #[serde(rename = "agent.brief")]
        AgentBrief(AgentBriefParams) -> AgentBriefResult,
        /// One turn of the agent and model given, without tools, settings or transcript, as
        /// titles are asked: the answer's text.
        #[serde(rename = "agent.ask")]
        AgentAsk(AgentAskParams) -> AgentAskResult,
        /// What is left of the plan's usage limits (Claude's 5-hour and weekly windows, Codex's),
        /// as the agent's own CLI reads them.
        #[serde(rename = "agent.usage")]
        AgentUsage(AgentModelsParams) -> AgentUsageResult,
        /// Read-only native account metadata; no model turn or forced credential refresh.
        #[serde(rename = "agent.account")]
        AgentAccount(AgentModelsParams) -> NativeAccountInfo,
        /// Starts logging the agent's CLI in to its account: the result's URL is for the user to open
        /// in a browser; the sidecar waits for the login to complete (a few minutes at most) and
        /// reports it with `agent.login_ended`. A login started meanwhile replaces it, unreported.
        #[serde(rename = "agent.login")]
        AgentLogin(AgentLoginStartParams) -> AgentLoginResult,
        /// Gives the login `agent.login` started headless the code its page showed (Claude Code's
        /// manual page); its end is still reported with `agent.login_ended`. Refused when no such
        /// login runs.
        #[serde(rename = "agent.login_code")]
        AgentLoginCode(AgentLoginCodeParams) -> (),
        /// Ends the agent's login under way, reported as not completed; nothing when none runs.
        #[serde(rename = "agent.login_cancel")]
        AgentLoginCancel(AgentModelsParams) -> (),
        /// A subagent's conversation from its transcript, without a running session. Refused by
        /// agents without such subagents (Codex).
        #[serde(rename = "agent.subagent_messages")]
        AgentSubagentMessages(AgentSubagentMessagesParams) -> AgentSubagentMessagesResult,
        /// Copies an agent session into a new one of its own (Claude Code's `forkSession`, Codex's
        /// `thread/fork`), without running it: the new id resumes it.
        #[serde(rename = "agent.fork")]
        AgentFork(AgentForkParams) -> AgentForkResult,
        /// Writes a stopped chat's native conversation (Claude's session files, Codex's rollout and
        /// the parents it was forked from) as a strict bundle to a new private file. Refused for a
        /// chat with a running session.
        #[serde(rename = "context.capture")]
        ContextCapture(ContextCaptureParams) -> ContextCaptureResult,
        /// Restores selected native bundles into a new, daemon-selected private home, never running them.
        #[serde(rename = "context.restore")]
        ContextRestore(ContextRestoreParams) -> ContextRestoreResult,
        /// Endpoint-only encryption/decryption of a package; keys never enter argv or artifacts.
        #[serde(rename = "package.seal")]
        PackageSeal(PackageCryptParams) -> PackageCryptResult,
        #[serde(rename = "package.open")]
        PackageOpen(PackageCryptParams) -> PackageCryptResult,
        #[serde(rename = "sidecar.ping")]
        Ping(PingParams) -> PingResult,
        /// The environment agents started from now on get: the login shell's, read again
        /// (`env.reload`). Sessions, model and usage reads, logins; running sessions keep theirs.
        #[serde(rename = "sidecar.set_env")]
        SetEnv(SetEnvParams) -> Empty,
    }
}

methods! {
    /// Messages the sidecar sends to the daemon: notifications (`session.event`) and requests
    /// (`hook.*`), told apart by the presence of an id.
    pub enum SidecarToDaemon / SidecarToDaemonMethod {
        #[serde(rename = "session.event")]
        SessionEvent(SessionEventParams),
        /// Claude's WorktreeCreate hook: returns the folder of the new worktree group.
        #[serde(rename = "hook.worktree_create")]
        WorktreeCreate(WorktreeCreateParams) -> WorktreeCreateResult,
        #[serde(rename = "hook.worktree_remove")]
        WorktreeRemove(WorktreeRemoveParams) -> Empty,
        /// The agent's `list_repos` tool: the repos it may propose for the chat's workspace.
        #[serde(rename = "tool.list_repos")]
        ToolListRepos(ToolChatParams) -> ToolListReposResult,
        /// The agent's `add_repo` tool, once the user said yes: adds the repo to the chat's workspace
        /// as `workspace.add_repo` does. Refused in a design chat.
        #[serde(rename = "tool.add_repo")]
        ToolAddRepo(ToolAddRepoParams) -> ToolAddRepoResult,
        /// The agent's `delegate` tool (story 49): starts a delegated chat in the chat's workspace with
        /// the task, or gives one it started earlier more work, and returns at once. Refused in a
        /// design chat and in a delegated chat.
        #[serde(rename = "tool.delegate")]
        ToolDelegate(ToolDelegateParams) -> ToolDelegateStarted,
        /// Waits until the delegated chat's turn is over (through its questions to the user), and
        /// returns its answer.
        #[serde(rename = "tool.delegate_wait")]
        ToolDelegateWait(ToolDelegateChildParams) -> ToolDelegateResult,
        /// Interrupts a delegated chat: its parent gave up the call (an interrupt).
        #[serde(rename = "tool.delegate_stop")]
        ToolDelegateStop(ToolDelegateChildParams) -> Empty,
        /// The parent's turn ended before the delegated chat answered, without an interrupt: the
        /// delegated chat works on, and once it is done the daemon sends its answer to the parent
        /// as a new message.
        #[serde(rename = "tool.delegate_detach")]
        ToolDelegateDetach(ToolDelegateChildParams) -> Empty,
        /// A login `agent.login` started ended: completed, failed, or given up after its timeout.
        #[serde(rename = "agent.login_ended")]
        AgentLoginEnded(AgentLoginEndedParams),
    }
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct AgentLoginEndedParams {
    pub agent: AgentKind,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct AgentLoginStartParams {
    pub agent: AgentKind,
    /// The browser is on another machine than the CLI (a worker signed in from the Mac): a flow
    /// whose page never comes back to this machine's localhost (Claude Code's manual page and its
    /// code to paste back, Codex's device code).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub headless: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct AgentLoginCodeParams {
    pub agent: AgentKind,
    /// As the login page shows it (Claude Code's `<code>#<state>`).
    pub code: String,
}

/// A login to finish in a browser.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct AgentLoginResult {
    pub url: String,
    /// The code to type on the page (Codex's device code).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub user_code: Option<String>,
    /// The page shows a code to paste back (`agent.login_code`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub asks_code: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct SessionMcpParams {
    pub chat_id: String,
    /// As `mcp_status` names it.
    pub server: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct McpConnectResult {
    /// A login page for the user to open; none when the server was reconnected without one.
    pub auth_url: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct PingParams {}

#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
pub struct SetEnvParams {
    /// Every variable: those absent are removed.
    pub env: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct PingResult {
    pub sidecar_version: String,
    /// Agent runtime versions the sidecar was built with, e.g. `claude-agent-sdk` → `0.3.296`.
    pub agents: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct AgentModelsParams {
    pub agent: AgentKind,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct AgentModelsResult {
    pub models: Vec<AgentModel>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct AgentCommandsParams {
    pub agent: AgentKind,
    pub cwd: String,
    #[serde(default)]
    pub additional_directories: Vec<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct AgentCommandsResult {
    pub commands: Vec<SlashCommand>,
}

/// Which subagent of an agent session to read. The agent id when known (a `task_started`'s
/// `task_id`); else the sidecar finds it from the tool call's result (which may name it) or its
/// prompt (the first message of the subagent's transcript).
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct AgentSubagentMessagesParams {
    pub agent: AgentKind,
    /// The chat's agent session id (its resume id).
    pub session_id: String,
    /// The folder the session runs in (the workspace folder): the agent keeps its transcripts by
    /// folder.
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// The prompt the subagent was given (the tool call's input).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// The tool call's result, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct AgentSubagentMessagesResult {
    /// The subagent found; none when it could not be identified.
    pub agent_id: Option<String>,
    /// Its transcript was read; false when there is none (yet).
    pub found: bool,
    pub events: Vec<ChatEvent>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
pub struct AgentForkParams {
    pub agent: AgentKind,
    /// The session to copy (a chat's resume id).
    pub session_id: String,
    /// The folder the session runs in: the agent keeps its sessions by folder.
    pub cwd: String,
    /// The environment its sessions get (the agent's home folders may be set there).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Copies the session only up to there, inclusive (a turn's `ForkPoint::up_to`); none: the
    /// whole session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub up_to: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ContextCaptureParams {
    pub chat_id: String,
    pub agent: AgentKind,
    /// The native session to capture: the chat's resume id, or one it was forked from.
    pub session_id: String,
    /// The folder the chat's sessions ran in (Claude keeps its sessions by folder).
    pub cwd: String,
    /// The chat's environment: the agent's home folders may be set there.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// An absolute path, not yet existing, in an existing folder.
    pub output: String,
    /// A backup of a running chat: its session may be live, and each transcript is taken up to
    /// its last complete line.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub live: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct PackageTransferContext {
    pub workspace_id: String,
    pub owner_peer_id: String,
    pub controller_peer_id: String,
    pub operation_id: String,
    pub source_peer_id: String,
    pub target_peer_id: String,
    #[ts(type = "number")]
    pub source_epoch: u64,
    /// The authenticated journal's base64url snapshot digest.
    pub snapshot_digest: String,
    /// A backup, not a transfer: `operation_id` is the backup's, `source_peer_id` the worker that
    /// took it, `source_epoch` the one it runs the workspace under, `target_peer_id` and
    /// `snapshot_digest` empty: what the Mac keeps of the transfer and the object's name are enough
    /// to open it. Authenticated under its own label, so neither can pass for the other.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub backup: bool,
}

#[derive(Serialize, Deserialize, TS, Clone)]
#[ts(export, optional_fields)]
pub struct PackageCryptParams {
    pub input: String,
    pub output: String,
    /// A fresh operation key, supplied only over the private endpoint pipe.
    pub key: String,
    pub context: PackageTransferContext,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ciphertext_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "number")]
    pub ciphertext_bytes: Option<u64>,
}

impl std::fmt::Debug for PackageCryptParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackageCryptParams")
            .field("input", &self.input)
            .field("output", &self.output)
            .field("context", &self.context)
            .field("key", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct PackageCryptResult {
    /// Base64url SHA-256 of the entire ciphertext, including framing and authentication tag.
    pub digest: String,
    #[ts(type = "number")]
    pub bytes: u64,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ContextRestoreParams {
    /// The destination's own native homes, where its agents read their login and
    /// conversations: files are added there, never replaced.
    pub claude_home: String,
    pub codex_home: String,
    pub contexts: Vec<ContextRestoreEntry>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ContextRestoreEntry {
    pub agent: AgentKind,
    pub session_id: String,
    pub cwd: String,
    pub path: String,
    pub sha256: String,
    #[ts(type = "number")]
    pub bytes: u64,
}

/// What a restoration added to the native homes, so that the copy it made can later take exactly
/// that away: never a file already there (kept or extended), nor a home it created.
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default, PartialEq, Eq)]
#[ts(export)]
pub struct ContextRestoreResult {
    /// The files it created, absolute.
    #[serde(default)]
    pub files: Vec<String>,
    /// The folders it created inside a home, absolute, outermost first.
    #[serde(default)]
    pub folders: Vec<String>,
}

/// A conversation that cannot be carried is `blocked`, with nothing written; a failure of the
/// capture itself (a running session, the output, the sidecar) is an error instead.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
#[ts(export)]
pub enum ContextCaptureResult {
    Captured {
        /// Lowercase hex SHA-256 of the file written.
        sha256: String,
        #[ts(type = "number")]
        bytes: u64,
    },
    /// Its native history is missing, damaged or unsupported.
    Blocked { reason: String },
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct AgentForkResult {
    /// The new session's id, resumed by `session.start`.
    pub session_id: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct AgentTitleParams {
    pub text: String,
}

/// What a workspace is named again from.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct AgentRetitleParams {
    /// The workspace's chats as text (the daemon's rendering of their event logs).
    pub conversation: String,
    /// The title it has now; none while it has only its slug.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
}

/// The usage limits of an agent's plan.
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default, PartialEq)]
#[ts(export)]
pub struct AgentUsageResult {
    /// The subscription (`max`, `pro`, `plus`…); none with an API key, which has no plan limits.
    pub plan: Option<String>,
    pub windows: Vec<UsageWindow>,
    /// Credits spent beyond the plan's limits this month, when the plan has them (Claude's
    /// extra usage); none otherwise.
    #[serde(default)]
    pub extra_usage: Option<ExtraUsage>,
    /// Manual usage-limit resets; absent when the CLI does not provide them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reset_credits: Option<ResetCredits>,
    /// The agent's CLI is not logged in, or its credentials were refused: why. Its limits are
    /// then empty.
    #[serde(default)]
    pub signed_out: Option<String>,
}

/// Available manual resets. Details may be unavailable or capped by the provider.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ResetCredits {
    pub available_count: u32,
    pub credits: Option<Vec<ResetCredit>>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ResetCredit {
    pub title: String,
    pub description: Option<String>,
    /// RFC 3339; none when the credit does not expire.
    pub expires_at: Option<String>,
}

/// Spend beyond a plan's limits, for the month. Amounts are in minor units of `currency` (cents
/// for USD).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ExtraUsage {
    /// False when it is turned off or cannot cover more use (its limit reached).
    pub enabled: bool,
    pub used: Option<f64>,
    /// None: no monthly limit.
    pub limit: Option<f64>,
    /// ISO 4217 (`USD`…).
    pub currency: Option<String>,
}

/// One usage limit: a share of a window of time that resets.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct UsageWindow {
    /// `5h`, `7d`, `7d Opus`…
    pub label: String,
    /// 0 to 100.
    pub used_percent: f64,
    /// RFC 3339.
    pub resets_at: Option<String>,
    /// The window's length, to tell how fast it is used.
    pub window_secs: Option<u32>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct CloseIdleResult {
    /// The chat has no session now; false when it was busy and kept.
    pub closed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub confirmed_launch_ids: Option<Vec<String>>,
}

/// Exact dispatch intents whose native roots this sidecar has confirmed stopped.
/// Missing metadata (old sidecar or untracked session) never proves an exit.
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
pub struct SessionStopResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub confirmed_launch_ids: Option<Vec<String>>,
}

/// What a design brief is written from.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct AgentBriefParams {
    /// The source chat's conversation as text (the daemon's rendering of its event log).
    pub conversation: String,
    /// The source chat's title, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Claude Code's own default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// A question asked in one turn (`agent.ask`).
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct AgentAskParams {
    pub agent: AgentKind,
    /// The system prompt.
    pub instructions: String,
    /// The user message.
    pub request: String,
    /// The agent's own default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// How long the turn may take before it is given up.
    #[ts(type = "number")]
    pub timeout_ms: u64,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct AgentAskResult {
    pub text: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct AgentBriefResult {
    pub brief: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct AgentTitleResult {
    /// A few words, in English, sentence case, no final punctuation.
    pub title: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct SessionStartParams {
    pub chat_id: String,
    pub workspace_id: String,
    pub agent: AgentKind,
    /// The workspace folder.
    pub cwd: String,
    /// The workspace's repos: Claude Code loads the skills and commands of each.
    #[serde(default)]
    pub additional_directories: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Fast mode, for models that have one.
    #[serde(default)]
    pub fast: bool,
    pub permission_mode: PermissionMode,
    /// An exploration's chat: nothing it runs may change the repos or reach beyond them (no
    /// edits, commits, pushes, PRs); stronger than, and kept over, the permission mode.
    #[serde(default)]
    pub read_only: bool,
    /// Where Claude Code keeps its plans: the workspace's notes folder (`.chauffeur/plans`),
    /// created by the daemon. In an exploration, the only place the agent may write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plans_dir: Option<String>,
    /// A design chat's folder (`.chauffeur/designs/<chat>`), created by the daemon: the session
    /// starts in it, and it is the only place the agent may write, whatever the permission mode
    /// (story 47).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub design_dir: Option<String>,
    /// The workspace's notes folder (`.chauffeur`), given for an exploration (its shell commands
    /// may not write there: its plans come through the Write tool and the plan guard) and for a
    /// design chat (it reads the notes, the user's attachments among them, without asking).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes_dir: Option<String>,
    /// Resume this agent session instead of starting a new one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_session_id: Option<String>,
    /// Variables added to the agent's environment (`CHAUFFEUR_*`).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Appended to the agent's system prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<String>,
    /// First user message, sent once the session is up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Images sent with the first message.
    #[serde(default)]
    pub prompt_attachments: Vec<Attachment>,
    /// Compact the conversation through Jev, when the user turned it on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jev: Option<JevParams>,
    /// Ask the agent for a suggested next message after each turn (Claude's prompt suggestions;
    /// `prompt_suggestions` in settings.toml).
    #[serde(default)]
    pub prompt_suggestions: bool,
    /// The agent gets the `delegate` tool, to start delegated chats (story 49): every chat but a
    /// design chat and a delegated chat.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub delegate: bool,
    /// The rules the user allowed always earlier in this chat (`request_resolved.rules`), kept
    /// over a new agent session (an idle one closed, a restart).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<String>>", optional)]
    pub always_allow: Vec<String>,
    // Daemon-owned dispatch intent, absent for legacy callers; never a transfer permit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub launch_id: Option<String>,
}

/// Compaction through Jev (TypeSafe): what Jev keeps of the conversation replaces the agent's own
/// compaction (for Claude, Chauffeur's copy of the fast-jev-compaction plugin; for Codex, a new
/// thread holding it), after a turn that fills the context past the threshold.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct JevParams {
    /// The user's TypeSafe API key.
    pub api_key: String,
    /// Share of the context in use, in percent as Claude Code's `/context` counts it, from which
    /// the chat is compacted.
    pub threshold_percent: u8,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct SessionStartResult {
    /// Known at once when resuming, and for agents that create their session up front (Codex);
    /// otherwise reported later by a `session_started` event.
    pub agent_session_id: Option<String>,
    // Optional native root observation; the daemon verifies its kernel identity and ancestry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub native_pid: Option<u32>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct SessionParams {
    pub chat_id: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct SessionTaskParams {
    pub chat_id: String,
    pub task_id: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct SessionSendParams {
    pub chat_id: String,
    pub text: String,
    /// Images sent with the text, already saved by the daemon.
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// The daemon's id for the message: a message that arrives mid-turn is reported with
    /// `user_message_delivered` once the agent took it in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct SessionContextUsageParams {
    pub chat_id: String,
    #[serde(default)]
    pub detail: ContextDetail,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct SessionContextUsageResult {
    pub usage: ContextUsage,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct SessionRespondParams {
    pub chat_id: String,
    pub request_id: String,
    pub response: RequestResponse,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct SessionConfigureParams {
    pub chat_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct SessionEventParams {
    pub chat_id: String,
    pub event: ChatEvent,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct WorktreeCreateParams {
    pub chat_id: String,
    /// The worktree name Claude chose.
    pub name: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct WorktreeCreateResult {
    pub path: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct WorktreeRemoveParams {
    pub chat_id: String,
    pub path: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct ToolChatParams {
    pub chat_id: String,
}

/// The repos an agent may propose: the organization's, those already in its workspace marked.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ToolListReposResult {
    /// The workspace's slug.
    pub workspace: String,
    /// The workspace is an exploration: an added repo is a detached checkout of its base branch.
    pub explore: bool,
    /// Repos are added only to a ready workspace: the agent is told so, the user is not asked.
    pub state: WorkspaceState,
    pub repos: Vec<ToolRepo>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ToolRepo {
    pub name: String,
    pub description: Option<String>,
    /// Already in the workspace: its worktree's folder.
    pub path: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct ToolAddRepoParams {
    pub chat_id: String,
    pub repo: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ToolAddRepoResult {
    /// The new worktree's folder.
    pub path: String,
    /// Its branch; none in an exploration (a detached checkout).
    pub branch: Option<String>,
    /// The repo's setup script was started (it runs in the background).
    pub setup: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct ToolDelegateParams {
    /// The parent chat, whose agent called the tool.
    pub chat_id: String,
    /// The delegated chat's agent; none: the parent's, or the other one for a review.
    pub agent: Option<AgentKind>,
    /// The task, the delegated chat's message.
    pub prompt: String,
    /// A delegated chat the parent started earlier, to give more work instead of starting one.
    pub chat: Option<String>,
    /// The parent's tool call, kept on a new delegated chat (`ChatInfo::parent_tool_use`).
    pub tool_use_id: Option<String>,
    /// A review or a second opinion: on the other agent than the parent's unless `agent` names one.
    pub review: Option<bool>,
    /// How strong a model the task needs; none is `default`.
    pub tier: Option<DelegateTier>,
    /// A model the user asked for by name; it picks the agent when `agent` names none.
    pub model: Option<String>,
    /// The reasoning effort, among those the model accepts.
    pub effort: Option<String>,
}

/// How strong a model a delegated chat runs on; Chauffeur picks the model of each agent.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DelegateTier {
    /// The quick, cheap model: searches, simple edits, short answers.
    Fast,
    /// The parent's model on the parent's agent, else the user's default model of the agent.
    #[default]
    Default,
    /// The agent's strongest everyday model, for hard work.
    Strongest,
}

/// A delegated chat that just got its task.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ToolDelegateStarted {
    /// The delegated chat's id.
    pub chat: String,
    pub agent: AgentKind,
    pub title: Option<String>,
    /// The model it runs on; none: the agent's own default.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    /// What Chauffeur chose otherwise than asked, and why (another agent, out of its limits).
    #[serde(default)]
    pub notice: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
#[ts(optional_fields)]
pub struct ToolDelegateChildParams {
    /// The parent chat.
    pub chat_id: String,
    /// The delegated chat.
    pub child: String,
}

/// A delegated chat whose turn is over.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ToolDelegateResult {
    pub chat: String,
    pub agent: AgentKind,
    pub title: Option<String>,
    pub status: ChatStatus,
    /// How its last turn ended; none when no turn ended (the agent could not start).
    pub outcome: Option<TurnOutcome>,
    /// The last turn's error, or why the agent stopped.
    pub error: Option<String>,
    /// Its last message (of the agent itself, not of its own subagents).
    pub text: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
pub struct Empty {}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum NativeAuthMode {
    Subscription,
    ApiKey,
    External,
    SignedOut,
    Unknown,
}
/// Public native CLI metadata only; never token sources, credential bytes or SDK error payloads.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct NativeAccountInfo {
    pub mode: NativeAuthMode,
    pub email: Option<String>,
    pub organization: Option<String>,
}
