use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::model::{
    AccountStatus, AgentKind, Attachment, ChatInfo, ChatStatus, ImageInput, LoadoutEntry,
    McpServer, PermissionMode, Question, QueuedMessage, SectionInfo, TurnOutcome, WorkspaceInfo,
    WorkspaceState, WorktreeInfo,
};

/// One entry of the event log.
///
/// Persisted events carry a `seq`, strictly increasing across the whole log: a client that saw
/// `seq` n resumes with `after_seq: n`. Ephemeral events (text deltas, task progress, raw script
/// output) have no `seq` and are never replayed.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct EventEnvelope {
    #[ts(type = "number | null")]
    pub seq: Option<i64>,
    /// RFC 3339 timestamp.
    pub ts: String,
    pub event: Event,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "scope", rename_all = "snake_case")]
#[ts(export)]
pub enum Event {
    /// A workspace or a chat was created or changed: clients refresh their lists from these.
    Registry { event: RegistryEvent },
    Workspace {
        workspace_id: String,
        event: WorkspaceEvent,
    },
    Chat {
        workspace_id: String,
        chat_id: String,
        event: ChatEvent,
    },
    /// The Office's presence: ephemeral, only to subscriptions with `office`.
    Office { event: crate::office::OfficeEvent },
    /// Opaque package events; capabilities remain owned by Core, not by package metadata.
    Extension {
        id: crate::extensions::ExtensionId,
        event: Value,
    },
}

impl Event {
    pub fn workspace_id(&self) -> Option<&str> {
        match self {
            Event::Registry { event } => match event {
                RegistryEvent::WorkspaceUpserted { workspace } => Some(&workspace.id),
                RegistryEvent::ChatUpserted { chat } => Some(&chat.workspace_id),
                RegistryEvent::ExtensionsChanged { .. }
                | RegistryEvent::LoadoutChanged { .. }
                | RegistryEvent::SectionsChanged { .. }
                | RegistryEvent::AccountChanged { .. }
                | RegistryEvent::IdentityChanged { .. }
                | RegistryEvent::MentionsChanged { .. }
                | RegistryEvent::LoginEnded { .. }
                | RegistryEvent::IdleReminder { .. }
                | RegistryEvent::RoutineUpserted { .. }
                | RegistryEvent::RoutineRemoved { .. }
                | RegistryEvent::RoutineSuggestionsChanged { .. } => None,
                RegistryEvent::WorkspaceRemoved { workspace_id, .. }
                | RegistryEvent::ChatRemoved { workspace_id, .. } => Some(workspace_id),
                RegistryEvent::TransferEnded { transfer } => Some(&transfer.workspace_id),
            },
            Event::Workspace { workspace_id, .. } | Event::Chat { workspace_id, .. } => {
                Some(workspace_id)
            }
            Event::Office { .. } | Event::Extension { .. } => None,
        }
    }

    pub fn chat_id(&self) -> Option<&str> {
        match self {
            Event::Chat { chat_id, .. } => Some(chat_id),
            Event::Registry {
                event: RegistryEvent::ChatUpserted { chat },
            } => Some(&chat.id),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum RegistryEvent {
    /// Activation changed on this machine; clients discard views of disabled extensions.
    ExtensionsChanged {
        extensions: Vec<crate::extensions::ExtensionInfo>,
    },
    WorkspaceUpserted {
        workspace: WorkspaceInfo,
    },
    ChatUpserted {
        chat: ChatInfo,
    },
    LoadoutChanged {
        entries: Vec<LoadoutEntry>,
    },
    /// The sidebar's sections, in order, after one was created, renamed, folded, moved or deleted.
    SectionsChanged {
        sections: Vec<SectionInfo>,
    },
    /// A routine was made, edited, activated, paused or ran.
    RoutineUpserted {
        // Boxed: by far larger than the other variants.
        routine: Box<crate::routines::RoutineInfo>,
    },
    /// A routine deleted; its workspace stays.
    RoutineRemoved {
        routine_id: String,
    },
    /// Find routines started or ended, or a suggestion was acted on.
    RoutineSuggestionsChanged {
        find: Box<crate::routines::RoutineFindInfo>,
    },
    /// A workspace whose creation failed: nothing of it is kept, its name is free again.
    WorkspaceRemoved {
        workspace_id: String,
        slug: String,
        error: Option<String>,
    },
    /// A chat deleted for good, with its conversation.
    ChatRemoved {
        workspace_id: String,
        chat_id: String,
    },
    /// Moved workspaces left an hour without activity (an agent's turn, the user acting on it):
    /// one reminder for those that reached it together; each now shows `idle_since`.
    IdleReminder {
        workspace_ids: Vec<String>,
    },
    /// A transfer this daemon drove ended moved, back or failed (not cancelled): clients that do
    /// not follow the workspace tell the user.
    TransferEnded {
        // Boxed: the largest variant by far.
        transfer: Box<crate::cloud_transfer::TransferInfo>,
    },
    /// An agent's CLI got logged out (its credentials refused) or back in: clients read
    /// `usage.get` again.
    AccountChanged {
        agent: AgentKind,
        account: AccountStatus,
        /// Why it is signed out, as the agent said.
        error: Option<String>,
    },
    /// The user signed in to Chauffeur or out, or what is known of their account changed:
    /// clients show or hide what depends on it (a member's features, the team option of shares).
    IdentityChanged {
        identity: crate::identity::IdentityState,
    },
    /// The user's mentions changed (one came, one was read, here or on a share's page).
    /// Ephemeral: `mentions.list` gives them.
    MentionsChanged {
        mentions: Vec<crate::design_share::Mentioned>,
        /// The ids of those that just came: clients tell the user of each, once.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[ts(as = "Option<Vec<String>>", optional)]
        new: Vec<String>,
    },
    /// A login `agent.login` or `worker.login` started ended: completed, failed, cancelled or
    /// given up after its timeout. One replaced by a newer login of the same agent is not told.
    LoginEnded {
        agent: AgentKind,
        /// The worker it ran on (`worker.login`); none for this machine's own.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        worker: Option<String>,
        success: bool,
        error: Option<String>,
    },
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum WorkspaceEvent {
    StateChanged {
        state: WorkspaceState,
        error: Option<String>,
    },
    /// A transfer of the workspace this daemon drives moved on.
    Transfer {
        // Boxed: the largest variant by far.
        transfer: Box<crate::cloud_transfer::TransferInfo>,
    },
    /// The connection to the machine a moved workspace runs on went down or came back:
    /// ephemeral, to the subscriptions the Mac's daemon relays there only. Down, its chats'
    /// events stop until it is back.
    Relay {
        machine: String,
        connected: bool,
        /// Why it went down, as said to the user: "vm does not answer (…)".
        error: Option<String>,
    },
    /// The persisted history a subscription asked for (`after_seq`) has been sent: what comes
    /// next is live. Ephemeral, once per workspace of the filter, so a client can show a loading
    /// state while a long conversation arrives.
    Replayed,
    WorktreeAdded {
        worktree: WorktreeInfo,
    },
    WorktreeRemoved {
        worktree_id: String,
    },
    /// A setup, run or archive script started. `process_id` identifies its output lines.
    ProcessStarted {
        process_id: String,
        kind: ProcessKind,
        repo: Option<String>,
        name: String,
        command: String,
    },
    /// One line of a script's output. Ephemeral: the daemon keeps a tail per process instead.
    ProcessOutput {
        process_id: String,
        stream: OutputStream,
        line: String,
    },
    ProcessExited {
        process_id: String,
        exit_code: Option<i32>,
    },
    /// Output of a terminal, base64-encoded bytes. Ephemeral: `terminal.snapshot` gives the
    /// scrollback.
    TerminalOutput {
        terminal_id: String,
        data: String,
        /// Offset of this chunk's first byte in the terminal's whole output.
        #[ts(type = "number")]
        offset: u64,
    },
    TerminalExited {
        terminal_id: String,
        exit_code: Option<i32>,
    },
    /// A command started or finished in a terminal (`TerminalInfo::command_running`). Ephemeral:
    /// `terminal.list` gives where each terminal stands.
    TerminalActivity {
        terminal_id: String,
        command_running: bool,
    },
    /// The workspace's share changed (story 53): connected, guests, who is viewing. Ephemeral.
    Share {
        state: Box<crate::client::WorkspaceShareState>,
    },
    /// A markdown file's share changed (story 57): connected, reviewers, who is looking, a
    /// reviewer's comment. Ephemeral.
    FileShare {
        state: Box<crate::client::DesignShareState>,
    },
    /// A step of creation or archive worth showing, e.g. "fetching web".
    Progress {
        message: String,
    },
    Warning {
        message: String,
    },
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProcessKind {
    Setup,
    Run,
    Archive,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// A delegated chat whose answer came to its parent as a message (`user_message`).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct DelegatedAnswer {
    pub chat: String,
    pub agent: AgentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
}

/// What happens in a chat, independent of the agent behind it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ChatEvent {
    /// A message from the user, or from the parent agent of a child workspace.
    UserMessage {
        text: String,
        /// Sent while the agent was working: it steers the turn, read at the agent's next step
        /// (`user_message_delivered` says when).
        queued: bool,
        /// Images sent with the text; absent from events written before images existed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        attachments: Option<Vec<Attachment>>,
        /// Identifies the message for `user_message_delivered`; absent from older events.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        message_id: Option<String>,
        /// The guest whose suggestion this is (story 53): shown to people, never to the agent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        suggested_by: Option<crate::client::CommentAuthor>,
        /// The delegated chat whose answer this is, sent by Chauffeur once the parent's turn had
        /// ended before it (story 49): shown to people; the agent reads it in the text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        delegated_answer: Option<DelegatedAnswer>,
    },
    /// The agent took in a message sent while it was working.
    UserMessageDelivered {
        message_id: String,
    },
    /// The chat's queue: messages held until the agent finishes its turn, then sent one by one.
    QueueChanged {
        messages: Vec<QueuedMessage>,
    },
    SessionStarted {
        agent_session_id: String,
        model: Option<String>,
    },
    /// A confirmed switch: the new agent reads the conversation as text with the next message.
    AgentChanged {
        from: AgentKind,
        to: AgentKind,
    },
    TurnStarted,
    /// Ephemeral: the next chunk (not the accumulated text) of an assistant message, superseded
    /// by `assistant_text` with the same `message_id`.
    TextDelta {
        message_id: String,
        text: String,
        /// Set when a subagent writes it: see `assistant_text`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        parent_tool_use_id: Option<String>,
    },
    /// Ephemeral: streamed thinking, superseded by `thinking`.
    ThinkingDelta {
        message_id: String,
        text: String,
        /// Set when a subagent thinks it: see `assistant_text`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        parent_tool_use_id: Option<String>,
    },
    /// What the agent says. A subagent's text (`parent_tool_use_id`: the tool call that started
    /// it) is ephemeral: shown live in the subagent's view, never stored; its transcript gives it
    /// back afterwards (`chat.subagent_messages`).
    AssistantText {
        message_id: String,
        text: String,
        parent_tool_use_id: Option<String>,
    },
    /// The agent's thinking; a subagent's (`parent_tool_use_id` set) is ephemeral, as its text.
    Thinking {
        message_id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        parent_tool_use_id: Option<String>,
    },
    ToolCall {
        tool_use_id: String,
        name: String,
        input: Value,
        /// Set when a subagent made the call.
        parent_tool_use_id: Option<String>,
    },
    ToolResult {
        tool_use_id: String,
        output: String,
        is_error: bool,
        /// From the agent only: the result's images (a screenshot, an image file it looked at),
        /// as bytes or a file. The daemon saves them as `attachments` and never keeps these.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        images: Option<Vec<ImageInput>>,
        /// The result's images, saved in the workspace's notes folder.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        attachments: Option<Vec<Attachment>>,
    },
    PermissionRequest {
        request_id: String,
        tool_name: String,
        input: Value,
        /// The agent's own sentence for the prompt, when it gives one.
        title: Option<String>,
        /// The subagent asking, when one does rather than the agent itself (Claude only).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        agent: Option<RequestAgent>,
    },
    McpElicitation {
        request_id: String,
        server_name: String,
        message: String,
        schema: Option<Value>,
        url: Option<String>,
        error: Option<String>,
        /// What Always allow on it would keep allowing in the chat, offered for a confirmation with
        /// nothing to fill in (`cua_repl: Allow Computer Use to use "Google Chrome"?`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        rule: Option<String>,
    },
    Question {
        request_id: String,
        questions: Vec<Question>,
    },
    /// A permission request or a question was answered. `outcome`: `allowed`, `denied`,
    /// `answered` or `cancelled` (by an interrupt).
    RequestResolved {
        request_id: String,
        outcome: String,
        /// Allowed with Always allow: the rules the chat now keeps allowing (Claude:
        /// `mcp__linear__save_issue`, `Bash(npm test:*)`; Codex, what its session approval
        /// covers: `Bash(make) in /folder`, `apply_patch(/folder/a.ts)`), or, for an open request
        /// that such a rule allowed in turn, the rules that covered it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        rules: Option<Vec<String>>,
    },
    TurnEnded {
        outcome: TurnOutcome,
        #[ts(type = "number | null")]
        duration_ms: Option<u64>,
        /// Total cost of the agent session so far, not of this turn alone.
        cost_usd: Option<f64>,
        /// API-equivalent token cost of this turn alone, not the user's subscription bill.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        estimated_cost_usd: Option<f64>,
        /// Tokens the agent session has used so far, not this turn alone: every model call's
        /// input (cache reads and writes included) and output, subagents and compaction
        /// included, a running total like `cost_usd` (Claude Code). Absent from older events.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional, type = "number")]
        session_tokens: Option<u64>,
        /// Tokens this turn alone used: every model call's input (cached included) and output
        /// (Codex). Absent from older events.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional, type = "number")]
        tokens: Option<u64>,
        /// Tokens in the context window after the turn, when the agent reports it.
        #[ts(type = "number | null")]
        context_tokens: Option<u64>,
        error: Option<String>,
        /// Where the turn ended in the agent's own store, which a fork from this turn copies up
        /// to (`chat.fork` with `turn`): set on a turn that ended successfully, absent from older
        /// events.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        fork_point: Option<ForkPoint>,
    },
    StatusChanged {
        status: ChatStatus,
    },
    /// Something the user should see that is not part of the conversation.
    Notice {
        level: NoticeLevel,
        message: String,
        /// A short label shown beside the message: the category of a call auto mode blocked
        /// (`Credential materialization`). Absent from most notices, and from older events.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        badge: Option<String>,
    },
    /// Ephemeral: a command the user runs from the chat (`chat.shell`) started; its
    /// `shell_ended` follows, with the same `shell_id`.
    ShellStarted {
        shell_id: String,
        command: String,
    },
    /// A command the user ran from the chat, in the workspace folder, and its output. The agent
    /// reads it with the user's next message (the first one sent while it ran, if any); it never
    /// starts a turn.
    ShellEnded {
        shell_id: String,
        command: String,
        /// None when it was killed: past its timeout, or by a signal.
        exit_code: Option<i32>,
        /// Its stdout and stderr as they came, cut in the middle past a cap.
        output: String,
        timed_out: bool,
        /// The user's message sent while it ran, which the agent got once it ended, with it.
        /// Absent when it ended first (it goes with the user's next message), and from older events.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        message_id: Option<String>,
    },
    /// The agent started a task that runs beside its turn, or past it: a shell command left in
    /// the background, a subagent, a workflow. Tasks the agent keeps for itself (watchers) are not
    /// reported. Sent again with the same `task_id` when a task changes (moved to the background).
    TaskStarted {
        task_id: String,
        kind: TaskKind,
        description: String,
        /// The subagent's type (`general-purpose`, `Explore`, a custom agent's name).
        subagent_type: Option<String>,
        /// The tool call that started it.
        tool_use_id: Option<String>,
        /// Runs in the background: the agent's turn may end before it does.
        background: bool,
        /// The subagent task that started this one, when a subagent did.
        parent_task_id: Option<String>,
    },
    /// Ephemeral: where a running task stands.
    TaskProgress {
        task_id: String,
        /// What it is doing now, when it says (a subagent's current step).
        activity: Option<String>,
        /// The tool it used last.
        last_tool: Option<String>,
        /// A one-line status, when the agent gives one.
        summary: Option<String>,
        #[ts(type = "number | null")]
        duration_ms: Option<u64>,
        tool_uses: Option<u32>,
    },
    /// Ephemeral: the agent's guess at the user's next message, sent after a turn ends (Claude's
    /// prompt suggestions). The app offers it in an empty composer until a new turn starts.
    PromptSuggestion {
        text: String,
    },
    /// Ephemeral: the agent's automatic permission check (Claude's auto mode) on a tool call has
    /// run a few seconds and goes on (`checking`), or is over (not `checking`: it says nothing of
    /// the verdict, which the tool's result, a denial or a permission request gives). Best
    /// effort: one may be lost, so clients also forget a check once the call has its result or the
    /// turn ends.
    /// A design chat's share changed (story 51): connected, viewers, a reviewer's comment in
    /// `comments.json`. Ephemeral.
    DesignShare {
        state: Box<crate::client::DesignShareState>,
    },
    /// The chat's suggestions from the guests of its workspace's share changed (story 53): all
    /// of them, the decided ones too. Ephemeral: `chat.suggestions` reads them.
    Suggestions {
        suggestions: Vec<crate::workspace_share::Suggestion>,
    },
    /// The guests' comments on the chat's messages changed (story 53): all of them. Ephemeral:
    /// `chat.share_comments` reads them.
    ShareComments {
        comments: Vec<crate::workspace_share::ShareComment>,
    },
    PermissionCheck {
        tool_use_id: String,
        checking: bool,
    },
    /// The agent's plan for its work, whole each time it changes (Codex's `turn/plan/updated`).
    /// The daemon counts its steps done into the chat's `progress`.
    PlanUpdated {
        steps: Vec<PlanStep>,
    },
    /// A task ended. Every task still running when its agent session ends (closed, crashed, the
    /// sidecar or the daemon restarted) is reported `stopped`.
    TaskEnded {
        task_id: String,
        status: TaskStatus,
        summary: Option<String>,
    },
    /// How full the agent's context window is, read after each turn (Claude Code's `/context`
    /// summary, Codex's last token count against its window). Kept in the log, so that the last
    /// reading outlives the session; never with `details`, which `chat.context_usage` reads on
    /// demand.
    ContextUsage {
        usage: ContextUsage,
    },
    /// Ephemeral, from the sidecar to the daemon: the permission mode the agent actually runs in,
    /// at start and whenever it changes. Claude Code falls back from auto to `default` where auto
    /// is unavailable (the model, the plan or a setting turns it off). The daemon keeps it on
    /// `ChatInfo::permission_mode_effective` rather than in the chat's log.
    PermissionModeEffective {
        mode: PermissionMode,
    },
    /// Ephemeral, from the sidecar to the daemon: the session's MCP servers and where each
    /// stands, at start and whenever one changes. The daemon keeps them on
    /// `ChatInfo::mcp_servers` rather than in the chat's log.
    McpStatus {
        servers: Vec<McpServer>,
    },
    /// Ephemeral, from the sidecar to the daemon: the agent refused its own credentials (logged
    /// out, a token expired or revoked). The daemon marks the agent signed out
    /// (`AgentUsage::account`); the turn's own error still reaches the chat.
    AccountLost {
        message: String,
    },
}

/// The end of a turn in an agent session: a fork from there keeps the session up to it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ForkPoint {
    /// The session the turn ran in (Claude Code's session id, Codex's thread id): the chat may
    /// have moved to another since (a fork, a compaction), this one still holds the turn.
    pub session_id: String,
    /// The last thing the fork keeps: Claude Code's transcript uuid of the turn's last message,
    /// Codex's turn id.
    pub up_to: String,
}

/// How full a chat's context window is.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ContextUsage {
    /// The model whose window it is, when the agent says.
    pub model: Option<String>,
    /// Tokens in the window, as the agent counts them (may exceed `max_tokens`).
    #[ts(type = "number")]
    pub used_tokens: u64,
    /// The window usage is measured against: for Claude, Claude Code's autocompact window.
    #[ts(type = "number")]
    pub max_tokens: u64,
    /// Share of the window in use, rounded, 0 to 100 (more when over).
    pub percent: f64,
    /// Past the window.
    pub over_limit: bool,
    /// The breakdown by category, in the agent's order (rows may be zero); empty when the agent
    /// gives none (Codex).
    pub categories: Vec<ContextCategory>,
    /// Where the chat compacts; none when unknown (Codex without Jev compacts where it does not
    /// say).
    pub compaction: Option<ContextCompaction>,
    /// Chauffeur's estimate rather than the agent's count: the context a Jev compaction of a Codex
    /// thread left, until Codex's first response in the new thread measures it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub estimated: bool,
    /// The items behind the categories: only in a full reading, never in the log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub details: Option<ContextDetails>,
}

/// One row of the breakdown, e.g. "Messages" or "MCP tools (deferred)".
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ContextCategory {
    pub name: String,
    #[ts(type = "number")]
    pub tokens: u64,
    pub kind: ContextCategoryKind,
}

/// What a row of the breakdown is.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ContextCategoryKind {
    /// Occupies the window.
    Used,
    /// The window left.
    Free,
    /// Kept free for compaction.
    Buffer,
    /// Out of the window (tool schemas loaded on demand): listed, not counted.
    Deferred,
}

/// The share of the window from which the chat compacts.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ContextCompaction {
    pub by: CompactionBy,
    pub threshold_percent: f64,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CompactionBy {
    /// Chauffeur's compaction through Jev (story 44), at its threshold.
    Jev,
    /// Claude Code's own autocompact, where its buffer starts.
    ClaudeCode,
}

/// What the categories hold, from a full reading.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Default)]
#[ts(export)]
pub struct ContextDetails {
    pub mcp_tools: Vec<ContextMcpTool>,
    pub memory_files: Vec<ContextMemoryFile>,
    pub skills: Vec<ContextItem>,
    pub agents: Vec<ContextItem>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ContextMcpTool {
    /// The wire name, e.g. `mcp__linear__create_issue`.
    pub name: String,
    pub server: String,
    #[ts(type = "number")]
    pub tokens: u64,
    /// Its schema is in the window; false when deferred (loaded on demand).
    pub loaded: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ContextMemoryFile {
    /// Absolute.
    pub path: String,
    /// Where it comes from: `Project`, `User`, `Local`…
    pub source: String,
    #[ts(type = "number")]
    pub tokens: u64,
}

/// A skill or a custom agent, by name.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ContextItem {
    pub name: String,
    /// Where it is defined: `projectSettings`, `userSettings`, `plugin`…
    pub source: String,
    #[ts(type = "number")]
    pub tokens: u64,
}

/// The subagent behind a permission request: its task's id, and what its `task_started` said
/// when the agent reported one.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct RequestAgent {
    /// The subagent's id: the `task_id` of its task.
    pub id: String,
    #[serde(default)]
    pub subagent_type: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Runs in the background, beside or past the agent's turn.
    #[serde(default)]
    pub background: bool,
}

/// What a task of the agent runs.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskKind {
    /// A shell command.
    Command,
    Subagent,
    /// A workflow, an MCP server's task, a remote agent…
    Other,
}

/// How a task ended.
/// One step of an agent's plan.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[ts(export)]
pub struct PlanStep {
    pub text: String,
    pub status: PlanStepStatus,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PlanStepStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskStatus {
    Completed,
    Failed,
    Stopped,
}

impl ChatEvent {
    /// Ephemeral events are fanned out live but never stored or replayed: deltas, progress,
    /// suggestions, and a subagent's text and thinking (its transcript keeps them).
    pub fn is_ephemeral(&self) -> bool {
        match self {
            ChatEvent::TextDelta { .. }
            | ChatEvent::ThinkingDelta { .. }
            | ChatEvent::TaskProgress { .. }
            | ChatEvent::PromptSuggestion { .. }
            | ChatEvent::ShellStarted { .. }
            | ChatEvent::PermissionCheck { .. }
            | ChatEvent::DesignShare { .. }
            | ChatEvent::Suggestions { .. }
            | ChatEvent::ShareComments { .. }
            | ChatEvent::PermissionModeEffective { .. }
            | ChatEvent::McpStatus { .. }
            | ChatEvent::AccountLost { .. } => true,
            ChatEvent::AssistantText {
                parent_tool_use_id, ..
            }
            | ChatEvent::Thinking {
                parent_tool_use_id, ..
            } => parent_tool_use_id.is_some(),
            _ => false,
        }
    }
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum NoticeLevel {
    Info,
    Warning,
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_permission_request_names_its_subagent_and_older_ones_still_read() {
        let old: ChatEvent = serde_json::from_value(serde_json::json!({
            "type": "permission_request", "request_id": "r", "tool_name": "Bash",
            "input": {"command": "ls"}, "title": null
        }))
        .unwrap();
        assert!(matches!(
            old,
            ChatEvent::PermissionRequest { agent: None, .. }
        ));
        assert!(serde_json::to_value(&old).unwrap().get("agent").is_none());

        let asked: ChatEvent = serde_json::from_value(serde_json::json!({
            "type": "permission_request", "request_id": "r", "tool_name": "Bash",
            "input": {}, "title": null, "agent": {"id": "a1"}
        }))
        .unwrap();
        let ChatEvent::PermissionRequest { agent, .. } = asked else {
            panic!("not a permission request")
        };
        assert_eq!(
            agent,
            Some(RequestAgent {
                id: "a1".into(),
                subagent_type: None,
                description: None,
                background: false
            })
        );
    }
}
