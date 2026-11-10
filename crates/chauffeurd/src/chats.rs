//! Chats: rows in the store, sessions in the sidecar. A chat's session is started lazily (first
//! message) and resumed with the agent's own session id after a daemon or sidecar restart.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};

use anyhow::Context;
use chauffeur_protocol::client::{
    CommentAuthor, ConfigureChatParams, ContextDetail, ContextUsageResult, CreateChatParams,
    QueueItem, ShellResult, SubagentMessagesParams, SubagentMessagesResult,
};
use chauffeur_protocol::events::{
    ChatEvent, CompactionBy, ContextCategoryKind, ContextCompaction, ContextUsage, DelegatedAnswer,
    Event, EventEnvelope, ForkPoint, NoticeLevel, RegistryEvent, TaskStatus,
};
use chauffeur_protocol::model::{
    AgentKind, AgentModels, Attachment, BriefState, ChatInfo, ChatKind, ChatProgress, ChatStatus,
    DesignBrief, ImageInput, LoadoutEntry, McpServer, PermissionMode, QueuedMessage,
    RequestResponse, SendMode, SlashCommand, TurnOutcome, WorkspaceInfo, WorkspaceState,
    WorktreeOrigin,
};
use chauffeur_protocol::sidecar::{
    AgentBriefParams, AgentBriefResult, AgentCommandsParams, AgentCommandsResult, AgentForkParams,
    AgentForkResult, AgentRetitleParams, AgentSubagentMessagesParams, AgentSubagentMessagesResult,
    AgentTitleParams, AgentTitleResult, DelegateTier, McpConnectResult, SessionConfigureParams,
    SessionContextUsageParams, SessionContextUsageResult, SessionMcpParams, SessionParams,
    SessionRespondParams, SessionSendParams, SessionStartParams, SessionTaskParams, SidecarCall,
    ToolAddRepoResult, ToolDelegateChildParams, ToolDelegateParams, ToolDelegateResult,
    ToolDelegateStarted, ToolListReposResult, ToolRepo,
};

use crate::events::Activity;
use crate::sidecar::Sidecar;
use crate::store::{WorkspaceRow, now};
use crate::{Daemon, Error, Result, attachments, design};

/// How long a workspace waits for its title before keeping its generated name.
const TITLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

/// A workspace Chauffeur named is named again from its conversation at its 3rd message from the
/// user, then every 10 (SPEC, "Disk").
const RETITLE_FIRST: u32 = 3;
const RETITLE_EVERY: u32 = 10;

/// How much of a workspace's conversation names it again: a few thousand tokens, quick for Haiku.
const RETITLE_BUDGET: usize = 24_000;

/// How long a design brief may take to write (the sidecar gives up at 2 minutes).
const BRIEF_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(150);

/// The store's key for the tasks running now, by chat: those left at a daemon restart are
/// reported stopped.
const RUNNING_TASKS_KEY: &str = "running_tasks";

/// The store's key for the chats whose cut turn Chauffeur resumed on its own and that have not
/// ended a turn since: cut again, they wait for the user, lest a turn that crashes loop.
const AUTO_RESUMED_KEY: &str = "auto_resumed_chats";

/// What the agent reads when Chauffeur resumes a turn that a restart cut.
const RESUME_PROMPT: &str = "Chauffeur restarted while you were working and cut your turn short: \
the commands and background tasks you had running were killed, and any question or permission \
request you had open went unanswered. Before running anything again, check the state it was left \
in (a command may have run halfway), then carry on with what you were doing.";

/// The store's key prefix, by chat, for the user message a failed turn was last resumed after
/// (`resume_failed_turns`): the same message's turn is resumed once.
const FAILED_RESUME_KEY: &str = "failed_turn_resumed:";

/// Where a chat's progress is kept between runs (`3/7`), by chat id.
const PROGRESS_KEY: &str = "chat_progress:";

/// Ollama's models run through Codex's Ollama provider (story 52).
const LOCAL_MODELS_RUN_IN_CODEX: &str =
    "local models run in Codex chats: pick Codex, or a hosted model for Claude Code";

pub struct ChatManager {
    daemon: Weak<Daemon>,
    pub sidecar: Sidecar,
    /// Chats whose session runs in the sidecar.
    live: std::sync::Mutex<HashSet<String>>,
    /// Open permission requests and questions, by chat. A background subagent's may come while
    /// no turn runs, and outlive the turn they came in.
    pending: std::sync::Mutex<HashMap<String, HashSet<String>>>,
    /// Chats whose agent runs a turn: between `turn_started` and `turn_ended`. A background
    /// subagent working on its own opens none.
    in_turn: std::sync::Mutex<HashSet<String>>,
    start_locks: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Serializes message handoffs with provider changes, before either takes the start lock.
    agent_locks: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// First messages of chats whose workspace is still being created, by workspace: shown at
    /// once, given to the agent when the workspace is ready (`workspace_ready`).
    first_messages: std::sync::Mutex<HashMap<String, Vec<FirstMessage>>>,
    /// When each chat last did something (an event, a message), since the daemon started.
    activity: std::sync::Mutex<HashMap<String, std::time::Instant>>,
    started: std::time::Instant,
    /// Chats whose session started before their workspace's tickets changed, or busy with a
    /// failed MCP server when the login shell's environment was read again: closed at their next
    /// message while idle, so that the agent resumes with the new briefing and environment.
    stale_briefing: std::sync::Mutex<HashSet<String>>,
    /// Chats told already that their tickets could not be read (once per chat).
    unread_told: std::sync::Mutex<HashSet<String>>,
    /// The agents' tasks running now (between `task_started` and `task_ended`), by chat.
    tasks: std::sync::Mutex<HashMap<String, HashSet<String>>>,
    /// The permission mode each chat's agent last reported running in.
    effective_modes: std::sync::Mutex<HashMap<String, PermissionMode>>,
    /// How far each chat's agent is through its plan, as read (kept in the store between runs).
    progress: std::sync::Mutex<HashMap<String, Option<ChatProgress>>>,
    /// The Claude Code tasks each chat's agent made, to count them into its progress.
    plan_tasks: std::sync::Mutex<HashMap<String, crate::plan_progress::Tasks>>,
    /// The MCP servers each chat's agent session last reported, while it runs.
    mcp_servers: std::sync::Mutex<HashMap<String, Vec<McpServer>>>,
    /// The design brief being written for each design chat, by number: only the last one asked
    /// is kept (a Regenerate overtakes the brief still being written).
    brief_runs: std::sync::Mutex<HashMap<String, u64>>,
    /// Chats resumed on their own after a cut turn, until they end a turn (kept in the store).
    auto_resumed: std::sync::Mutex<HashSet<String>>,
    /// The user's shell commands running now (`shell`), by id.
    shells: std::sync::Mutex<HashMap<String, RunningShell>>,
    /// Counts the shell commands that ended, for the messages waiting on some.
    shell_ended: tokio::sync::watch::Sender<u64>,
    /// Each chat's last message sent, which tells the one after it once it was handed to the
    /// agent (its sender dropped): messages reach it in the order they were sent, one waiting for
    /// shell commands holding up those after it.
    deliveries: std::sync::Mutex<HashMap<String, tokio::sync::oneshot::Receiver<()>>>,
}

/// A shell command of the user's that runs, and the message that hands it to the agent once it
/// ended: the first one sent while it runs.
struct RunningShell {
    chat_id: String,
    message_id: Option<String>,
}

/// A message logged in its chat, for its agent.
struct Delivery {
    text: String,
    attachments: Vec<Attachment>,
    message_id: String,
    /// The seq of the user's message before it, and its own.
    since: i64,
    sent: i64,
}

/// Who a message comes from, when the user did not write it in the chat.
#[derive(Debug)]
pub enum Sender {
    User,
    /// A guest's suggestion (story 53), the user sent.
    Guest(CommentAuthor),
    /// A delegated chat's answer, once its parent's turn had ended (story 49).
    DelegatedChat(DelegatedAnswer),
}

/// A delegated chat's parent (story 49): the chat whose agent started it, and its tool call.
struct Delegation {
    chat: String,
    tool_use: Option<String>,
}

/// How often a wait on a chat looks again without an event: a lagging receiver misses some.
const WAIT_RECHECK: std::time::Duration = std::time::Duration::from_secs(5);

struct FirstMessage {
    chat_id: String,
    text: String,
    attachments: Vec<Attachment>,
    /// The seq of its event: what the chat held before it is handed over with it.
    sent: i64,
}

impl ChatManager {
    pub fn new(daemon: Weak<Daemon>) -> ChatManager {
        ChatManager {
            sidecar: Sidecar::new(daemon.clone()),
            daemon,
            live: Default::default(),
            pending: Default::default(),
            in_turn: Default::default(),
            start_locks: Default::default(),
            agent_locks: Default::default(),
            first_messages: Default::default(),
            activity: Default::default(),
            started: std::time::Instant::now(),
            stale_briefing: Default::default(),
            unread_told: Default::default(),
            tasks: Default::default(),
            effective_modes: Default::default(),
            progress: Default::default(),
            plan_tasks: Default::default(),
            mcp_servers: Default::default(),
            brief_runs: Default::default(),
            auto_resumed: Default::default(),
            shells: Default::default(),
            shell_ended: tokio::sync::watch::Sender::new(0),
            deliveries: Default::default(),
        }
    }

    /// How many tasks a chat's agent runs now.
    /// How many tasks run in each chat that has some.
    pub fn tasks_by_chat(&self) -> HashMap<String, u32> {
        self.tasks
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, tasks)| !tasks.is_empty())
            .map(|(chat, tasks)| (chat.clone(), tasks.len() as u32))
            .collect()
    }

    pub fn running_tasks(&self, chat_id: &str) -> u32 {
        self.tasks
            .lock()
            .unwrap()
            .get(chat_id)
            .map_or(0, |tasks| tasks.len() as u32)
    }

    /// A chat as clients see it: with its running tasks and the permission mode its agent runs
    /// in, which only the daemon's memory holds, and its last activity, from the event log.
    fn as_seen(&self, mut chat: ChatInfo) -> ChatInfo {
        chat.running_tasks = self.running_tasks(&chat.id);
        chat.permission_mode_effective =
            self.effective_modes.lock().unwrap().get(&chat.id).copied();
        chat.progress = self.progress_of(&chat.id);
        // A session that ended no longer reaches its servers: none to show or reconnect.
        chat.mcp_servers = if self.is_live(&chat.id) {
            self.mcp_servers
                .lock()
                .unwrap()
                .get(&chat.id)
                .cloned()
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        chat.last_activity_at = match self.d().store.chat_last_event_at(&chat.id) {
            Ok(Some(ts)) => Some(ts),
            _ => Some(chat.created_at.clone()),
        };
        chat
    }

    fn d(&self) -> Arc<Daemon> {
        self.daemon.upgrade().expect("daemon alive")
    }

    fn progress_of(&self, chat_id: &str) -> Option<ChatProgress> {
        if let Some(progress) = self.progress.lock().unwrap().get(chat_id) {
            return *progress;
        }
        let stored = self
            .d()
            .store
            .kv_get(&format!("{PROGRESS_KEY}{chat_id}"))
            .ok()
            .flatten()
            .and_then(|text| crate::plan_progress::decode(&text));
        self.progress
            .lock()
            .unwrap()
            .insert(chat_id.to_string(), stored);
        stored
    }

    /// The agent's plan moved: clients and colleagues in the Office see the chat's new progress.
    fn set_progress(&self, chat: &ChatInfo, progress: Option<ChatProgress>) {
        let before = self.progress_of(&chat.id);
        if before == progress {
            return;
        }
        self.progress
            .lock()
            .unwrap()
            .insert(chat.id.clone(), progress);
        let d = self.d();
        let key = format!("{PROGRESS_KEY}{}", chat.id);
        let stored = match progress {
            Some(progress) => d
                .store
                .kv_set(&key, &crate::plan_progress::encode(progress)),
            None => d.store.kv_delete(&key),
        };
        if let Err(error) = stored {
            tracing::warn!(%error, "could not keep a chat's progress");
        }
        d.events.registry(RegistryEvent::ChatUpserted {
            chat: self.as_seen(chat.clone()),
        });
        let done = |p: Option<ChatProgress>| p.map_or(0, |p| p.done);
        let steps = done(progress).saturating_sub(done(before));
        if steps > 0 {
            d.events.report(Activity::StepsDone(steps));
        }
    }

    /// A chat by id, or the first open chat of a workspace given by id or slug.
    pub fn resolve(&self, key: &str) -> Result<ChatInfo> {
        let d = self.d();
        if let Some(chat) = d.store.chat(key)? {
            return Ok(self.as_seen(chat));
        }
        let workspace = d.workspaces.row(key)?;
        let chats = d.store.chats_of(&workspace.id)?;
        let first_open = chats.iter().position(|chat| !chat.closed).unwrap_or(0);
        chats
            .into_iter()
            .nth(first_open)
            .map(|chat| self.as_seen(chat))
            .ok_or_else(|| Error::NotFound(format!("{} has no chat yet", workspace.slug)))
    }

    /// A workspace's chats, oldest first; the closed ones only with `all`.
    pub fn list(&self, workspace: &str, all: bool) -> Result<Vec<ChatInfo>> {
        let d = self.d();
        let row = d.workspaces.row(workspace)?;
        Ok(d.store
            .chats_of(&row.id)?
            .into_iter()
            .filter(|chat| all || !chat.closed)
            .map(|chat| self.as_seen(chat))
            .collect())
    }

    pub async fn create(&self, params: CreateChatParams) -> Result<ChatInfo> {
        self.create_in(params, None, None).await
    }

    /// Retries an unstarted automatic chat without logging its first message twice.
    pub(crate) async fn retry_first_message(
        &self,
        chat_id: &str,
        fallback: &str,
    ) -> Result<ChatInfo> {
        let chat = self.current(chat_id)?;
        if chat.agent_session_id.is_some() || self.is_live(chat_id) {
            return Ok(chat);
        }
        let first = self
            .d()
            .store
            .events_of_chat(chat_id)?
            .into_iter()
            .find_map(|e| match e.event {
                Event::Chat {
                    event: ChatEvent::UserMessage { text, .. },
                    ..
                } => Some(text),
                _ => None,
            });
        match first {
            Some(text) => {
                self.start_session(&chat, Some(text), Vec::new()).await?;
                self.current(chat_id)
            }
            None => {
                self.send_with(chat_id, fallback, &[], SendMode::Steer)
                    .await
            }
        }
    }

    /// Creates a chat; a delegated chat with its `parent`.
    /// `admitted`: the admission of the operation this creation completes, which a fence
    /// installed meanwhile must not refuse halfway.
    async fn create_in(
        &self,
        params: CreateChatParams,
        parent: Option<Delegation>,
        admitted: Option<&crate::workspace_execution::Admission>,
    ) -> Result<ChatInfo> {
        let d = self.d();
        let workspace = d.workspaces.row(&params.workspace)?;
        let _admission = match admitted {
            Some(_) => None,
            None => Some(d.execution.admit(&workspace.id)?),
        };
        // A workspace being created gets its chats at once; their agents start once it is ready.
        let creating = workspace.state == WorkspaceState::Creating;
        if workspace.state != WorkspaceState::Ready && !creating {
            return Err(Error::Conflict(format!(
                "{} is {}",
                workspace.slug,
                workspace.state.as_str()
            )));
        }
        let design = params.kind == Some(ChatKind::Design);
        let source_chat = match params.source_chat.as_deref().filter(|id| !id.is_empty()) {
            Some(_) if !design => {
                return Err(Error::BadRequest(
                    "only a design chat starts from another chat".into(),
                ));
            }
            Some(id) => {
                let source = d
                    .store
                    .chat(id)?
                    .filter(|source| source.workspace_id == workspace.id)
                    .ok_or_else(|| Error::not_found(format!("chat {id} in {}", workspace.slug)))?;
                Some(source)
            }
            None => None,
        };
        // A design chat runs on Claude Code unless Codex is asked for.
        let default = if design {
            agent_choice(&d, params.agent.unwrap_or(AgentKind::Claude))?
        } else {
            default_choice(&d)?
        };
        let agent = params.agent.unwrap_or(default.agent);
        let chosen = params.model.filter(|m| !m.is_empty());
        // The default model, with its effort and speed, belongs to the default agent.
        let (model, effort, fast) = match chosen {
            None if agent == default.agent && !default.model.is_empty() => (
                Some(default.model),
                params.effort.filter(|e| !e.is_empty()).or(default.effort),
                params.fast.unwrap_or(default.fast),
            ),
            model => (
                model,
                params.effort.filter(|e| !e.is_empty()),
                params.fast.unwrap_or(false),
            ),
        };
        if agent != AgentKind::Codex && model.as_deref().is_some_and(crate::ollama::is_local) {
            return Err(Error::BadRequest(LOCAL_MODELS_RUN_IN_CODEX.into()));
        }
        let prompt = params.prompt.filter(|p| !p.trim().is_empty());
        let chat = ChatInfo {
            id: uuid::Uuid::now_v7().to_string(),
            progress: None,
            tab_position: None,
            workspace_id: workspace.id.clone(),
            agent,
            model,
            effort,
            fast,
            // A design chat runs in auto mode (accept edits where auto is unavailable), and its
            // writes stay in its own folder whatever the mode (the sidecar's guard and sandbox);
            // an exploration's chats plan, whatever was asked: they change nothing.
            permission_mode: if design {
                PermissionMode::Auto
            } else if workspace.explore {
                PermissionMode::Plan
            } else {
                params.permission_mode.unwrap_or_default()
            },
            title: params
                .title
                .or_else(|| prompt.as_deref().map(title_of))
                .or_else(|| {
                    source_chat
                        .as_ref()
                        .map(|source| design_title(self.label_of(source).as_deref()))
                }),
            status: if creating {
                ChatStatus::Starting
            } else {
                ChatStatus::Idle
            },
            agent_session_id: None,
            created_at: now(),
            running_tasks: 0,
            permission_mode_effective: None,
            closed: false,
            last_activity_at: None,
            mcp_servers: Vec::new(),
            kind: params.kind.unwrap_or_default(),
            source_chat: source_chat.as_ref().map(|source| source.id.clone()),
            // Written in the background from the source chat; a first message sent with the
            // creation goes instead.
            brief: (source_chat.is_some() && prompt.is_none()).then_some(DesignBrief {
                state: BriefState::Writing,
                text: None,
                error: None,
            }),
            parent_tool_use: parent.as_ref().and_then(|p| p.tool_use.clone()),
            parent_chat: parent.map(|p| p.chat),
            forked_from_agent: None,
        };
        let chat = ChatInfo {
            last_activity_at: Some(chat.created_at.clone()),
            ..chat
        };
        if !d.store.insert_chat(&chat)? {
            // Its creation failed since the workspace was read, and took it away.
            return Err(Error::not_found(format!("workspace {}", workspace.slug)));
        }
        d.events
            .registry(RegistryEvent::ChatUpserted { chat: chat.clone() });
        d.workspaces.publish(&workspace.id);
        d.events.report(Activity::ChatCreated);
        if chat.brief.is_some() {
            self.spawn_brief(&chat.id);
        }
        let images = params.images.unwrap_or_default();
        if creating {
            if prompt.is_some() || !images.is_empty() {
                self.hold_first_message(&chat, &workspace, prompt.unwrap_or_default(), &images)?;
            }
            return Ok(chat);
        }
        if prompt.is_some() || !images.is_empty() {
            let prompt = prompt.unwrap_or_default();
            // A first message the agent cannot take leaves the chat in error with a notice:
            // the chat itself exists and can be retried.
            return match self
                .send_with(&chat.id, &prompt, &images, SendMode::Steer)
                .await
            {
                Ok(chat) => Ok(chat),
                Err(error) => {
                    tracing::warn!(%error, chat = chat.id, "the first message could not start the agent");
                    self.current(&chat.id)
                }
            };
        }
        Ok(chat)
    }

    /// Sends a user message, starting or resuming the session first if needed. A workspace with
    /// no chat yet gets one.
    pub async fn send(&self, key: &str, text: &str) -> Result<ChatInfo> {
        self.send_with(key, text, &[], SendMode::Steer).await
    }

    /// Sends a user message with images, saved in the workspace's notes folder first.
    pub async fn send_with(
        &self,
        key: &str,
        text: &str,
        images: &[ImageInput],
        mode: SendMode,
    ) -> Result<ChatInfo> {
        self.send_from(key, text, images, mode, Sender::User).await
    }

    /// As `send_with`, for a message the user did not write (`Sender`). A guest's suggestion
    /// (story 53): the chat shows who suggested it, the agent reads the text alone; it never
    /// waits in the queue, it steers a running turn.
    pub async fn send_from(
        &self,
        key: &str,
        text: &str,
        images: &[ImageInput],
        mode: SendMode,
        from: Sender,
    ) -> Result<ChatInfo> {
        let mode = if matches!(from, Sender::Guest(_)) {
            SendMode::Steer
        } else {
            mode
        };
        if text.trim().is_empty() && images.is_empty() {
            return Err(Error::BadRequest("the message is empty".into()));
        }
        let d = self.d();
        let chat = match self.resolve(key) {
            Ok(chat) => chat,
            Err(Error::NotFound(_)) if d.store.chat(key)?.is_none() => {
                let workspace = d.workspaces.row(key)?;
                return Box::pin(self.create(CreateChatParams {
                    workspace: workspace.id,
                    agent: None,
                    model: None,
                    effort: None,
                    fast: None,
                    permission_mode: None,
                    prompt: Some(text.to_string()),
                    images: Some(images.to_vec()),
                    ..Default::default()
                }))
                .await;
            }
            Err(error) => return Err(error),
        };
        let _admission = d.execution.admit(&chat.workspace_id)?;
        self.require_native_context(&chat.id)?;
        if chat.status == ChatStatus::Closed {
            return Err(Error::Conflict("this chat's workspace is archived".into()));
        }
        // A message to a chat the user closed puts it back among the tabs.
        if chat.closed {
            self.set_closed(&chat.id, false)?;
        }
        let workspace = d.workspaces.row(&chat.workspace_id)?;
        let attachments = if images.is_empty() {
            Vec::new()
        } else {
            attachments::save(&workspace.path, images, attachments::Accept::Files)?
        };
        let message = || QueuedMessage {
            id: uuid::Uuid::now_v7().to_string(),
            text: text.to_string(),
            attachments: attachments.clone(),
            created_at: now(),
        };
        let mut chat = chat;
        // While the workspace is being created, messages wait for the agent to start. Its state
        // is checked again with the write: the creation may have ended since it was read.
        if workspace.state == WorkspaceState::Creating {
            if d.store.queue_push_while_creating(&message(), &chat.id)? {
                self.publish_queue(&chat)?;
                return self.current(&chat.id);
            }
            // Failed, it took the chat away (not found); done, the message goes as to a ready
            // workspace.
            chat = self.current(&chat.id)?;
        }
        let busy = matches!(
            chat.status,
            ChatStatus::Working | ChatStatus::NeedsInput | ChatStatus::Starting
        );
        if mode == SendMode::Queue && busy {
            if !d.store.queue_push(&message(), &chat.id)? {
                return Err(Error::not_found(format!("chat {}", chat.id)));
            }
            self.publish_queue(&chat)?;
            return self.current(&chat.id);
        }
        self.deliver_admitted(&chat, text, attachments, from, _admission)
            .await
    }

    /// Hands a message to the agent: it starts a turn, or steers the running one. The chat shows
    /// it at once; while shell commands of the user's run in the chat, or an earlier message waits
    /// for some, the agent gets it once they ended, with their output, and the call does not wait.
    async fn deliver_admitted(
        &self,
        chat: &ChatInfo,
        text: &str,
        attachments: Vec<Attachment>,
        from: Sender,
        admission: crate::workspace_execution::Admission,
    ) -> Result<ChatInfo> {
        let d = self.d();
        let chat = self.current(&chat.id)?;
        // Saved while the workspace was created, maybe under the folder name it had then.
        let attachments =
            attachments::moved_to(&d.workspaces.row(&chat.workspace_id)?.path, attachments);
        self.touch(&chat.id);
        let queued = matches!(chat.status, ChatStatus::Working | ChatStatus::NeedsInput);
        let message_id = uuid::Uuid::now_v7().to_string();
        self.name_workspace_from(&chat.workspace_id, text);
        if let Err(error) = d.store.count_turn(&chat.workspace_id) {
            tracing::warn!(%error, workspace = chat.workspace_id, "could not count the message");
        }
        if chat.brief.is_some() {
            self.drop_brief(&chat.id)?;
        }
        // Right before the message is logged: the commands running now end as its own.
        let since = self.last_seq(&chat.id, "user_message")?;
        let waits = self.claim_shells(&chat.id, &message_id);
        let sent = d
            .events
            .chat(
                &chat.workspace_id,
                &chat.id,
                ChatEvent::UserMessage {
                    text: text.to_string(),
                    queued,
                    attachments: (!attachments.is_empty()).then(|| attachments.clone()),
                    message_id: Some(message_id.clone()),
                    suggested_by: match &from {
                        Sender::Guest(author) => Some(author.clone()),
                        _ => None,
                    },
                    delegated_answer: match from {
                        Sender::DelegatedChat(answer) => Some(answer),
                        _ => None,
                    },
                },
            )
            .seq
            .unwrap_or(i64::MAX);
        let message = Delivery {
            text: text.to_string(),
            attachments,
            message_id,
            since,
            sent,
        };
        // Its place in the line, taken before anything waits: a message sent right after it comes
        // after it.
        let (handed, after) = tokio::sync::oneshot::channel::<()>();
        let mut before = self
            .deliveries
            .lock()
            .unwrap()
            .insert(chat.id.clone(), after);
        let before_handed = before.as_mut().is_none_or(|before| {
            !matches!(
                before.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            )
        });
        if !waits && before_handed {
            let _handed = handed;
            return self.hand_over(&chat.id, message, admission).await;
        }
        let chat_id = chat.id.clone();
        tokio::spawn(async move {
            let _handed = handed;
            if let Some(before) = before {
                let _ = before.await;
            }
            d.chats.shells_ended(&message.message_id).await;
            if let Err(error) = d.chats.hand_over(&chat.id, message, admission).await {
                tracing::warn!(%error, chat = chat.id, "could not hand a message to the agent");
                d.chats.notice(
                    &chat,
                    NoticeLevel::Error,
                    format!("Could not send your message: {error}"),
                );
            }
        });
        self.current(&chat_id)
    }

    /// Gives a logged message to the agent, with the shell commands it hands over.
    async fn hand_over(
        &self,
        chat_id: &str,
        message: Delivery,
        admission: crate::workspace_execution::Admission,
    ) -> Result<ChatInfo> {
        let d = self.d();
        let lock = self.agent_lock(chat_id);
        let _guard = lock.lock().await;
        let chat = self.current(chat_id)?;
        let Delivery {
            text,
            attachments,
            message_id,
            ..
        } = &message;
        let commands = self.shell_commands_of(&chat.id, &message)?;
        if self.is_live(&chat.id)
            && chat.status == ChatStatus::Idle
            && self.stale_briefing.lock().unwrap().contains(&chat.id)
        {
            self.close_for_briefing(&chat.id, true).await;
        }
        // What the agent reads: a fork's conversation its agent never had, the user's shell
        // commands the message hands over, the message, and a design chat's invalid canvas.json.
        let text = self.with_handoff(
            &chat,
            crate::chat_shell::with_commands(&commands, &self.with_canvas_note(&chat, text)),
            message.sent,
        )?;
        let text = &text;
        self.ensure_ollama(&chat).await?;
        if self.is_live(&chat.id) {
            let sent = d
                .chats
                .sidecar
                .call(SidecarCall::SessionSend(SessionSendParams {
                    chat_id: chat.id.clone(),
                    text: text.to_string(),
                    attachments: attachments.clone(),
                    message_id: Some(message_id.clone()),
                }))
                .await;
            match sent {
                Ok(_) => return self.current(&chat.id),
                // The session died since: start it again below.
                Err(_) => {
                    self.live.lock().unwrap().remove(&chat.id);
                }
            }
        }
        self.start_session_admitted(
            &chat,
            Some(text.to_string()),
            attachments.clone(),
            admission,
        )
        .await?;
        self.current(&chat.id)
    }

    /// A message after the conversation a fork's agent never had (what the chat held before the
    /// message's event, `sent`), until that agent has a session of its own.
    fn with_handoff(&self, chat: &ChatInfo, mut text: String, sent: i64) -> Result<String> {
        let Some(from) = chat.forked_from_agent else {
            return Ok(text);
        };
        if chat.agent_session_id.is_some() {
            return Ok(text);
        }
        let before: Vec<ChatEvent> = self
            .d()
            .store
            .events_of_chat(&chat.id)?
            .into_iter()
            .filter(|envelope| envelope.seq.is_some_and(|seq| seq < sent))
            .filter_map(|envelope| match envelope.event {
                Event::Chat { event, .. } => Some(event),
                _ => None,
            })
            .collect();
        if let Some(handoff) = handoff(from, chat.agent, &before) {
            text.insert_str(0, &handoff);
        }
        Ok(text)
    }

    fn publish_queue(&self, chat: &ChatInfo) -> Result<Vec<QueuedMessage>> {
        let d = self.d();
        let messages = d.store.queue_of(&chat.id)?;
        d.events.chat(
            &chat.workspace_id,
            &chat.id,
            ChatEvent::QueueChanged {
                messages: messages.clone(),
            },
        );
        Ok(messages)
    }

    /// Reorders, edits or removes queued messages.
    pub fn queue_set(&self, key: &str, items: &[QueueItem]) -> Result<Vec<QueuedMessage>> {
        let chat = self.resolve(key)?;
        // A workspace fenced for a transfer keeps the chats it is packaging as they are.
        let _admission = self.d().execution.admit(&chat.workspace_id)?;
        self.d().store.queue_set(&chat.id, items)?;
        self.publish_queue(&chat)
    }

    /// Sends one queued message now, out of turn.
    pub async fn queue_send(&self, key: &str, id: &str) -> Result<ChatInfo> {
        let chat = self.resolve(key)?;
        let admission = self.d().execution.admit(&chat.workspace_id)?;
        self.require_native_context(&chat.id)?;
        let message = self
            .d()
            .store
            .queue_take(&chat.id, id)?
            .ok_or_else(|| Error::not_found(format!("queued message {id}")))?;
        self.publish_queue(&chat)?;
        self.deliver_admitted(
            &chat,
            &message.text,
            message.attachments,
            Sender::User,
            admission,
        )
        .await
    }

    /// After a turn that completed, the first queued message runs as the next turn. A short
    /// pause lets a turn the agent starts on its own (messages it had not folded in) begin first.
    fn send_next_queued(&self, chat_id: String) {
        let d = self.d();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            let Ok(Some(chat)) = d.store.chat(&chat_id) else {
                return;
            };
            if chat.status != ChatStatus::Idle {
                return;
            }
            let Ok(admission) = d.execution.admit(&chat.workspace_id) else {
                return;
            };
            if d.chats.require_native_context(&chat.id).is_err() {
                return;
            }
            let Ok(Some(message)) = d.store.queue_pop(&chat.id) else {
                return;
            };
            if let Err(error) = d.chats.publish_queue(&chat) {
                tracing::warn!(%error, "could not publish a chat's queue");
            }
            if let Err(error) = d
                .chats
                .deliver_admitted(
                    &chat,
                    &message.text,
                    message.attachments,
                    Sender::User,
                    admission,
                )
                .await
            {
                tracing::warn!(%error, chat = chat.id, "could not send a queued message");
            }
        });
    }

    fn current(&self, id: &str) -> Result<ChatInfo> {
        self.d()
            .store
            .chat(id)?
            .map(|chat| self.as_seen(chat))
            .ok_or_else(|| Error::not_found(format!("chat {id}")))
    }

    pub(crate) fn is_live(&self, id: &str) -> bool {
        self.live.lock().unwrap().contains(id)
    }

    fn require_native_context(&self, chat: &str) -> Result<()> {
        let reason: Option<String> = self.d().store.with(|c| {
            use rusqlite::OptionalExtension;
            c.query_row(
                "SELECT reason FROM restored_chat_blocks WHERE chat_id=?1",
                [chat],
                |r| r.get(0),
            )
            .optional()
        })?;
        if let Some(reason) = reason {
            return Err(Error::Conflict(format!(
                "This conversation cannot resume: {reason}; preserve the source, or explicitly create a new chat"
            )));
        }
        Ok(())
    }

    /// Brings Ollama back before a message reaches a local model's agent: the Ollama the daemon
    /// started stops once unused, a live session included.
    async fn ensure_ollama(&self, chat: &ChatInfo) -> Result<()> {
        if chat.agent != AgentKind::Codex
            || !chat.model.as_deref().is_some_and(crate::ollama::is_local)
        {
            return Ok(());
        }
        let Err(error) = self.d().ollama.ensure().await else {
            return Ok(());
        };
        self.notice(chat, NoticeLevel::Error, error.to_string());
        if !self.is_live(&chat.id) {
            self.set_status(chat, ChatStatus::Error);
        }
        Err(error)
    }

    async fn start_session(
        &self,
        chat: &ChatInfo,
        prompt: Option<String>,
        attachments: Vec<Attachment>,
    ) -> Result<()> {
        let admission = self.d().execution.admit(&chat.workspace_id)?;
        self.start_session_admitted(chat, prompt, attachments, admission)
            .await
    }

    async fn start_session_admitted(
        &self,
        chat: &ChatInfo,
        prompt: Option<String>,
        attachments: Vec<Attachment>,
        _admission: crate::workspace_execution::Admission,
    ) -> Result<()> {
        self.require_native_context(&chat.id)?;
        let d = self.d();
        let lock = self
            .start_locks
            .lock()
            .unwrap()
            .entry(chat.id.clone())
            .or_default()
            .clone();
        let _guard = lock.lock().await;
        self.ensure_ollama(chat).await?;
        if self.is_live(&chat.id) {
            if let Some(text) = prompt {
                self.sidecar
                    .call(SidecarCall::SessionSend(SessionSendParams {
                        chat_id: chat.id.clone(),
                        text,
                        attachments,
                        message_id: None,
                    }))
                    .await?;
            }
            return Ok(());
        }
        let workspace = d.workspaces.row(&chat.workspace_id)?;
        let chat = self.current(&chat.id)?;
        self.set_status(&chat, ChatStatus::Starting);
        // Tickets never read from Linear (it failed, or was off, when they were attached): read
        // now, or the agent is told why it starts without them.
        if let Some(unread) = d.workspaces.ensure_briefing(&chat.workspace_id).await
            && self.unread_told.lock().unwrap().insert(chat.id.clone())
        {
            let (noun, pronoun) = match unread.tickets.len() {
                1 => ("ticket", "its"),
                _ => ("tickets", "their"),
            };
            self.notice(
                &chat,
                NoticeLevel::Warning,
                format!(
                    "Could not read the Linear {noun} {} ({}): the agent starts without {pronoun} content. Refresh the tickets in the workspace header, or run `chauffeur ticket refresh`, to try again.",
                    unread.tickets.join(", "),
                    unread.reason
                ),
            );
        }
        // Read again: its tickets may have just been read from Linear.
        let workspace = d.workspaces.row(&workspace.id).unwrap_or(workspace);
        let repos = repo_dirs(&d.workspaces.info(&workspace)?);
        self.stale_briefing.lock().unwrap().remove(&chat.id);
        // A design chat's agent starts in its own folder, the one place it may write.
        let design_dir = match chat.kind {
            ChatKind::Design => match design::design_dir(&workspace.path, &chat.id) {
                Ok(dir) => Some(dir),
                Err(error) => {
                    let message = format!("could not make the chat's design folder: {error}");
                    self.notice(&chat, NoticeLevel::Error, message.clone());
                    self.set_status(&chat, ChatStatus::Error);
                    return Err(Error::Internal(anyhow::anyhow!(message)));
                }
            },
            ChatKind::Chat => None,
        };
        let mut env = d.workspaces.agent_env(&workspace, design_dir.is_some())?;
        // An exploration's Claude Code keeps its temporary files, and its sandboxed commands
        // theirs, in a folder of the exploration's own, removed at archive and promote.
        if workspace.explore && chat.agent == AgentKind::Claude {
            let tmp = match crate::explore_tmp::ensure(&d.paths.explore_tmp_root, &workspace.id) {
                Ok(tmp) => tmp,
                Err(error) => {
                    let message =
                        format!("could not make the exploration's temporary folder: {error}");
                    self.notice(&chat, NoticeLevel::Error, message.clone());
                    self.set_status(&chat, ChatStatus::Error);
                    return Err(Error::Internal(anyhow::anyhow!(message)));
                }
            };
            env.insert(
                "CLAUDE_CODE_TMPDIR".into(),
                tmp.to_string_lossy().into_owned(),
            );
        }
        crate::ollama::codex_env(&d.paths.home, chat.agent, chat.model.as_deref(), &mut env)?;
        let params = SessionStartParams {
            launch_id: None,
            chat_id: chat.id.clone(),
            workspace_id: workspace.id.clone(),
            agent: chat.agent,
            cwd: design_dir.clone().unwrap_or_else(|| workspace.path.clone()),
            additional_directories: repos,
            model: chat.model.clone(),
            effort: chat.effort.clone(),
            fast: chat.fast,
            permission_mode: chat.permission_mode,
            read_only: workspace.explore,
            plans_dir: match chat.agent {
                AgentKind::Claude if design_dir.is_none() => plans_dir(&workspace),
                _ => None,
            },
            notes_dir: (workspace.explore || design_dir.is_some()).then(|| {
                std::path::Path::new(&workspace.path)
                    .join(crate::paths::NOTES_DIR)
                    .to_string_lossy()
                    .into_owned()
            }),
            resume_session_id: chat.agent_session_id.clone(),
            env,
            append_system_prompt: system_prompt(
                &workspace,
                &chat,
                design_dir.as_deref().map(|dir| (dir, chat.agent)),
                self.subagent_notice(&chat).as_deref(),
                // Codex keeps its plan natively; Claude, with Chauffeur's tool.
                (chat.agent == AgentKind::Claude && design_dir.is_none()).then_some(PLAN_NOTICE),
            ),
            design_dir,
            prompt,
            prompt_attachments: attachments,
            jev: d.jev.session_params(&d.store),
            prompt_suggestions: d.settings.get().prompt_suggestions,
            always_allow: always_allowed(&d.store, &chat.id),
            // One level of delegation: a delegated chat answers its parent, it delegates nothing.
            delegate: chat.kind == ChatKind::Chat && chat.parent_chat.is_none(),
        };
        // A new session runs in the mode asked until its agent says otherwise, and reports its
        // own MCP servers.
        self.effective_modes.lock().unwrap().remove(&chat.id);
        self.mcp_servers.lock().unwrap().remove(&chat.id);
        match self
            .sidecar
            .call(SidecarCall::SessionStart(Box::new(params)))
            .await
        {
            Ok(_) => {
                self.live.lock().unwrap().insert(chat.id.clone());
                // The first turn's `turn_started` moves the status on; without a prompt the
                // session simply waits.
                let current = self.current(&chat.id)?;
                if current.status == ChatStatus::Starting {
                    self.set_status(&current, ChatStatus::Idle);
                }
                Ok(())
            }
            // The daemon stops under the start: the chat stays starting, for the next daemon to
            // see the cut turn, as the sidecar's exit leaves a working one.
            Err(error) if d.shutdown.is_cancelled() => Err(error),
            Err(error) => {
                self.notice(
                    &chat,
                    NoticeLevel::Error,
                    format!("could not start the agent: {error}"),
                );
                self.set_status(&chat, ChatStatus::Error);
                Err(error)
            }
        }
    }

    pub async fn interrupt(&self, key: &str) -> Result<ChatInfo> {
        let chat = self.resolve(key)?;
        if self.is_live(&chat.id) {
            self.sidecar
                .call(SidecarCall::SessionInterrupt(SessionParams {
                    chat_id: chat.id.clone(),
                }))
                .await?;
        }
        self.current(&chat.id)
    }

    /// Stops one task of the agent (a background command, a subagent): its `task_ended` follows
    /// from the agent.
    pub async fn stop_task(&self, key: &str, task_id: &str) -> Result<ChatInfo> {
        let chat = self.resolve(key)?;
        if chat.agent == AgentKind::Codex {
            return Err(Error::BadRequest(
                "Codex runs no task Chauffeur can stop: interrupt the chat instead".into(),
            ));
        }
        let running = self
            .tasks
            .lock()
            .unwrap()
            .get(&chat.id)
            .is_some_and(|tasks| tasks.contains(task_id));
        if !running || !self.is_live(&chat.id) {
            return Err(Error::NotFound(format!(
                "no running task {task_id} in this chat"
            )));
        }
        self.sidecar
            .call(SidecarCall::SessionStopTask(SessionTaskParams {
                chat_id: chat.id.clone(),
                task_id: task_id.to_string(),
            }))
            .await?;
        self.current(&chat.id)
    }

    /// Runs a command of the user's in the chat's workspace folder (`chat_shell`), shown in the
    /// chat; the agent reads it with the next message. Refused while the workspace is not ready,
    /// in an exploration (its chats only read) and in a design chat (it never touches code).
    pub async fn shell(&self, key: &str, command: &str) -> Result<ShellResult> {
        if command.trim().is_empty() {
            return Err(Error::BadRequest("the command is empty".into()));
        }
        let d = self.d();
        let chat = self.resolve(key)?;
        if chat.kind == ChatKind::Design {
            return Err(Error::BadRequest(
                "a design chat runs no shell command: run it from a chat of the workspace".into(),
            ));
        }
        let workspace = self.editable_workspace(&chat, false)?;
        crate::workspaces::read_only(
            &workspace,
            "its chats run no shell command (its Terminal tab does)",
        )?;
        let mut env = d.shell_env.vars();
        env.extend(d.workspaces.agent_env(&workspace, false)?);
        let running = {
            let _admission = d.execution.admit(&workspace.id)?;
            crate::chat_shell::spawn(command, std::path::Path::new(&workspace.path), &env)
                .context("starting the shell")?
        };
        let shell_id = uuid::Uuid::now_v7().to_string();
        self.shells.lock().unwrap().insert(
            shell_id.clone(),
            RunningShell {
                chat_id: chat.id.clone(),
                message_id: None,
            },
        );
        d.events.chat(
            &chat.workspace_id,
            &chat.id,
            ChatEvent::ShellStarted {
                shell_id: shell_id.clone(),
                command: command.to_string(),
            },
        );
        // Not waited for: how it ends comes in `shell_ended`, which a dropped connection does not
        // lose.
        let (id, command) = (shell_id.clone(), command.to_string());
        tokio::spawn(async move {
            let outcome = running.wait(crate::chat_shell::TIMEOUT).await;
            let outcome = outcome.unwrap_or_else(|error| crate::chat_shell::Outcome {
                exit_code: None,
                output: format!("Chauffeur lost track of the command: {error}"),
                timed_out: false,
            });
            d.chats.end_shell(&chat, id, command, outcome);
        });
        Ok(ShellResult { shell_id })
    }

    /// Logs how a command ended, handed over with the message that waits for it, if one does.
    fn end_shell(
        &self,
        chat: &ChatInfo,
        shell_id: String,
        command: String,
        outcome: crate::chat_shell::Outcome,
    ) {
        // Logged under the lock: a message sent meanwhile either claimed the command before, or
        // is logged after it and finds it among those ended before it.
        let mut shells = self.shells.lock().unwrap();
        let message_id = shells.remove(&shell_id).and_then(|shell| shell.message_id);
        self.d().events.chat(
            &chat.workspace_id,
            &chat.id,
            ChatEvent::ShellEnded {
                shell_id,
                command,
                exit_code: outcome.exit_code,
                output: outcome.output,
                timed_out: outcome.timed_out,
                message_id,
            },
        );
        drop(shells);
        self.shell_ended.send_modify(|ended| *ended += 1);
    }

    /// Gives the chat's shell commands running now to the message `message_id`: the agent gets it
    /// once they ended. Whether there were any.
    fn claim_shells(&self, chat_id: &str, message_id: &str) -> bool {
        let mut claimed = false;
        for shell in self.shells.lock().unwrap().values_mut() {
            if shell.chat_id == chat_id && shell.message_id.is_none() {
                shell.message_id = Some(message_id.to_string());
                claimed = true;
            }
        }
        claimed
    }

    /// Waits for the shell commands the message `message_id` claimed to end.
    async fn shells_ended(&self, message_id: &str) {
        let mut ended = self.shell_ended.subscribe();
        while self
            .shells
            .lock()
            .unwrap()
            .values()
            .any(|shell| shell.message_id.as_deref() == Some(message_id))
        {
            if ended.changed().await.is_err() {
                return;
            }
        }
    }

    /// The user's shell commands a message hands to the agent, oldest first: those that ended
    /// since the user's last message and before it, and those it waited for.
    fn shell_commands_of(&self, chat_id: &str, message: &Delivery) -> Result<Vec<ChatEvent>> {
        Ok(self
            .d()
            .store
            .events_of_type_after(chat_id, "shell_ended", message.since)?
            .into_iter()
            .filter_map(|envelope| {
                let Event::Chat { event, .. } = envelope.event else {
                    return None;
                };
                let ChatEvent::ShellEnded { message_id, .. } = &event else {
                    return None;
                };
                let mine = match message_id {
                    Some(handed) => *handed == message.message_id,
                    None => envelope.seq.is_some_and(|seq| seq < message.sent),
                };
                mine.then_some(event)
            })
            .collect())
    }

    /// How full a chat's context window is: read from its running Claude session, else the last
    /// reading of the log (an idle-closed or unstarted session is not started for this; Codex
    /// reads its window only after a turn).
    pub async fn context_usage(
        &self,
        key: &str,
        detail: ContextDetail,
    ) -> Result<ContextUsageResult> {
        let d = self.d();
        let chat = self.resolve(key)?;
        if chat.agent == AgentKind::Claude && self.is_live(&chat.id) {
            let read = self
                .sidecar
                .call(SidecarCall::SessionContextUsage(
                    SessionContextUsageParams {
                        chat_id: chat.id.clone(),
                        detail,
                    },
                ))
                .await
                .and_then(|value| {
                    Ok(serde_json::from_value::<SessionContextUsageResult>(value)?.usage)
                });
            match read {
                Ok(usage) => {
                    return Ok(ContextUsageResult {
                        usage: Some(usage),
                        read_at: Some(now()),
                        live: true,
                    });
                }
                Err(error) => {
                    tracing::warn!(%error, chat = %chat.id, "could not read the context usage")
                }
            }
        }
        let last = d
            .store
            .last_events_of_type(&chat.id, "context_usage", 1)?
            .into_iter()
            .find_map(|envelope| match envelope.event {
                Event::Chat {
                    event: ChatEvent::ContextUsage { usage },
                    ..
                } => Some((usage, envelope.ts)),
                _ => None,
            });
        let jev = d
            .jev
            .session_params(&d.store)
            .map(|jev| jev.threshold_percent);
        Ok(match last {
            Some((usage, ts)) => ContextUsageResult {
                usage: Some(stored_reading(usage, chat.agent, jev)),
                read_at: Some(ts),
                live: false,
            },
            None => ContextUsageResult {
                usage: None,
                read_at: None,
                live: false,
            },
        })
    }

    /// A subagent's conversation, from its transcript (see `chat.subagent_messages`). The log
    /// tells which subagent a tool call started (its `task_started`), the prompt it was given and
    /// its result; the sidecar reads the transcript, with or without a running session.
    pub async fn subagent_messages(
        &self,
        params: SubagentMessagesParams,
    ) -> Result<SubagentMessagesResult> {
        let d = self.d();
        let chat = self.resolve(&params.chat)?;
        if chat.agent == AgentKind::Codex {
            return Err(Error::BadRequest(
                "Codex runs no subagents with a conversation of their own".into(),
            ));
        }
        let tool_use_id = params.tool_use_id.filter(|id| !id.is_empty());
        let mut agent_id = params.agent_id.filter(|id| !id.is_empty());
        if tool_use_id.is_none() && agent_id.is_none() {
            return Err(Error::BadRequest(
                "name the subagent: tool_use_id or agent_id".into(),
            ));
        }
        let not_found = |agent_id: Option<String>| SubagentMessagesResult {
            agent_id,
            found: false,
            events: Vec::new(),
        };
        // The chat's own session, then those its conversation was forked from, newest first: a
        // subagent run before the fork keeps its transcript in the session it ran in.
        let mut sessions: Vec<String> = chat.agent_session_id.clone().into_iter().collect();
        sessions.extend(d.store.chat_fork_sessions(&chat.id)?.into_iter().rev());
        // No session yet: no transcript either.
        if sessions.is_empty() {
            return Ok(not_found(agent_id));
        }
        let mut prompt = None;
        let mut result = None;
        if let Some(tool_use_id) = &tool_use_id {
            let mut known = false;
            for envelope in d.store.chat_events_mentioning(&chat.id, tool_use_id, 200)? {
                let Event::Chat { event, .. } = envelope.event else {
                    continue;
                };
                match event {
                    ChatEvent::TaskStarted {
                        task_id,
                        tool_use_id: Some(started_by),
                        ..
                    } if &started_by == tool_use_id => {
                        known = true;
                        agent_id.get_or_insert(task_id);
                    }
                    ChatEvent::ToolCall {
                        tool_use_id: id,
                        input,
                        ..
                    } if &id == tool_use_id => {
                        known = true;
                        prompt = input
                            .get("prompt")
                            .and_then(|p| p.as_str())
                            .map(str::to_string);
                    }
                    ChatEvent::ToolResult {
                        tool_use_id: id,
                        output,
                        ..
                    } if &id == tool_use_id => {
                        known = true;
                        result = Some(output);
                    }
                    _ => {}
                }
            }
            if !known && agent_id.is_none() {
                return Err(Error::NotFound(format!(
                    "no tool call {tool_use_id} in this chat"
                )));
            }
        }
        let workspace = d.workspaces.get(&chat.workspace_id)?;
        let mut read: Option<AgentSubagentMessagesResult> = None;
        for session_id in sessions {
            let value = self
                .sidecar
                .call(SidecarCall::AgentSubagentMessages(
                    AgentSubagentMessagesParams {
                        agent: chat.agent,
                        session_id,
                        cwd: workspace.path.clone(),
                        agent_id: agent_id.clone(),
                        prompt: prompt.clone(),
                        result: result.clone(),
                    },
                ))
                .await?;
            let this: AgentSubagentMessagesResult = serde_json::from_value(value)?;
            if agent_id.is_none() {
                agent_id.clone_from(&this.agent_id);
            }
            let found = this.found;
            read = Some(this);
            if found {
                break;
            }
        }
        let read = read.expect("one session read at least");
        Ok(SubagentMessagesResult {
            agent_id: read.agent_id.or(agent_id),
            found: read.found,
            events: read.events,
        })
    }

    pub async fn respond(
        &self,
        key: &str,
        request_id: &str,
        response: RequestResponse,
    ) -> Result<ChatInfo> {
        let chat = self.resolve(key)?;
        let _admission = self.d().execution.admit(&chat.workspace_id)?;
        let open = self
            .pending
            .lock()
            .unwrap()
            .get(&chat.id)
            .is_some_and(|ids| ids.contains(request_id));
        if !open || !self.is_live(&chat.id) {
            return Err(Error::NotFound(format!(
                "no open request {request_id} in this chat"
            )));
        }
        self.sidecar
            .call(SidecarCall::SessionRespond(SessionRespondParams {
                chat_id: chat.id.clone(),
                request_id: request_id.to_string(),
                response,
            }))
            .await?;
        self.current(&chat.id)
    }

    pub async fn configure(&self, params: ConfigureChatParams) -> Result<ChatInfo> {
        let d = self.d();
        let mut chat = self.resolve(&params.chat)?;
        let _admission = d.execution.admit(&chat.workspace_id)?;
        let lock = self.agent_lock(&chat.id);
        let _guard = lock.lock().await;
        let start = self.start_lock(&chat.id);
        let _start = start.lock().await;
        chat = self.current(&chat.id)?;
        let previous = chat.agent;
        let switching = params.agent.is_some_and(|agent| agent != previous);
        if chat.kind == ChatKind::Design
            && params
                .permission_mode
                .is_some_and(|mode| mode != chat.permission_mode)
        {
            return Err(Error::BadRequest(
                "a design chat's permission mode is fixed: it writes its own folder only".into(),
            ));
        }
        if let Some(agent) = params.agent.filter(|agent| *agent != chat.agent) {
            self.editable_workspace(&chat, false)?;
            if self.agent_busy(&chat)?
                || self
                    .shells
                    .lock()
                    .unwrap()
                    .values()
                    .any(|shell| shell.chat_id == chat.id)
            {
                return Err(Error::Conflict(
                    "this chat has work or requests pending: finish or stop them before changing its provider".into(),
                ));
            }
            if (chat.agent_session_id.is_some()
                || chat.forked_from_agent.is_some()
                || self.is_live(&chat.id))
                && params.confirm_agent != Some(previous)
            {
                return Err(Error::Conflict(
                    "changing this chat's provider hands over its conversation as text: confirm the current agent first".into(),
                ));
            }
            if params.confirm_agent.is_some_and(|agent| agent != previous) {
                return Err(Error::Conflict(
                    "this chat's provider changed: choose it again and confirm".into(),
                ));
            }
            chat.agent = agent;
            // The other agent's model and effort mean nothing here.
            chat.model = None;
            chat.effort = None;
            chat.fast = false;
        }
        if let Some(model) = &params.model {
            // Codex takes a thread's provider only when the thread starts or resumes.
            let local = crate::ollama::is_local;
            if !switching
                && (chat.agent_session_id.is_some() || self.is_live(&chat.id))
                && local(model) != chat.model.as_deref().is_some_and(local)
            {
                let (now, then) = if local(model) {
                    ("a hosted model", "a local one")
                } else {
                    ("a local model", "a hosted one")
                };
                return Err(Error::Conflict(format!(
                    "this chat already runs on {now}: start a new chat for {then}"
                )));
            }
            chat.model = Some(model.clone());
        }
        if chat.agent != AgentKind::Codex
            && chat.model.as_deref().is_some_and(crate::ollama::is_local)
        {
            return Err(Error::BadRequest(LOCAL_MODELS_RUN_IN_CODEX.into()));
        }
        if let Some(effort) = &params.effort {
            chat.effort = Some(effort.clone());
        }
        if let Some(fast) = params.fast {
            chat.fast = fast;
        }
        if let Some(mode) = params.permission_mode {
            if mode != PermissionMode::Plan && d.workspaces.row(&chat.workspace_id)?.explore {
                return Err(Error::BadRequest(
                    "an exploration's chats are read-only: create a workspace to change code"
                        .into(),
                ));
            }
            chat.permission_mode = mode;
        }
        if switching {
            if self.is_live(&chat.id) {
                let result = self
                    .sidecar
                    .call(SidecarCall::SessionCloseIdle(SessionParams {
                        chat_id: chat.id.clone(),
                    }))
                    .await?;
                if result["closed"].as_bool() != Some(true) {
                    return Err(Error::Conflict("this chat's agent is working: wait until it is idle before changing its provider".into()));
                }
                self.live.lock().unwrap().remove(&chat.id);
            }
            chat.forked_from_agent = Some(previous);
            chat.agent_session_id = None;
            chat.status = ChatStatus::Idle;
            self.stale_briefing.lock().unwrap().remove(&chat.id);
            self.effective_modes.lock().unwrap().remove(&chat.id);
            self.mcp_servers.lock().unwrap().remove(&chat.id);
            self.plan_tasks.lock().unwrap().remove(&chat.id);
            self.progress.lock().unwrap().remove(&chat.id);
            d.store.kv_delete(&format!("{PROGRESS_KEY}{}", chat.id))?;
        }
        // A session not running cannot tell how the new model or mode works out: as asked until
        // its next start says otherwise. A running one reports it while being configured.
        if (params.model.is_some() || params.permission_mode.is_some()) && !self.is_live(&chat.id) {
            self.effective_modes.lock().unwrap().remove(&chat.id);
        }
        d.store.update_chat(&chat)?;
        if switching {
            d.events.chat(
                &chat.workspace_id,
                &chat.id,
                ChatEvent::AgentChanged {
                    from: previous,
                    to: chat.agent,
                },
            );
        }
        let chat = self.as_seen(chat);
        d.events
            .registry(RegistryEvent::ChatUpserted { chat: chat.clone() });
        if self.is_live(&chat.id) {
            self.sidecar
                .call(SidecarCall::SessionConfigure(SessionConfigureParams {
                    chat_id: chat.id.clone(),
                    model: params.model,
                    effort: params.effort,
                    fast: params.fast,
                    permission_mode: params.permission_mode,
                }))
                .await?;
            // With the permission mode the agent reported while being configured.
            return self.current(&chat.id);
        }
        Ok(chat)
    }

    /// Ingests an event the sidecar reported for a chat.
    pub fn on_event(&self, chat_id: &str, event: ChatEvent) {
        let d = self.d();
        let Ok(Some(mut chat)) = d.store.chat(chat_id) else {
            tracing::warn!(chat_id, "event for an unknown chat");
            return;
        };
        match event {
            ChatEvent::PermissionModeEffective { mode } => return self.effective_mode(chat, mode),
            ChatEvent::McpStatus { servers } => return self.mcp_status(chat, servers),
            ChatEvent::AccountLost { message } => {
                return d.usage.account_lost(&d, chat.agent, message);
            }
            _ => {}
        }
        self.touch(&chat.id);
        let event = self.save_tool_images(&chat.workspace_id, event);
        let turn_done = matches!(
            event,
            ChatEvent::TurnEnded {
                outcome: TurnOutcome::Success,
                ..
            }
        );
        let turn_ended = matches!(event, ChatEvent::TurnEnded { .. });
        let next = {
            let mut pending = self.pending.lock().unwrap();
            let open = pending.entry(chat.id.clone()).or_default();
            let mut in_turn = self.in_turn.lock().unwrap();
            // Working while the agent itself runs a turn, waiting for the user while a request is
            // open (the agent's, or a background subagent's), idle otherwise: a background
            // subagent working alone shows as a running task, not as the chat working.
            let settled = |open: &HashSet<String>, turn: bool| {
                if !open.is_empty() {
                    ChatStatus::NeedsInput
                } else if turn {
                    ChatStatus::Working
                } else {
                    ChatStatus::Idle
                }
            };
            match &event {
                ChatEvent::SessionStarted {
                    agent_session_id,
                    model,
                } => {
                    chat.agent_session_id = Some(agent_session_id.clone());
                    if chat.model.is_none() {
                        chat.model = model.clone();
                    }
                    let _ = d.store.update_chat(&chat);
                    None
                }
                ChatEvent::TurnStarted => {
                    in_turn.insert(chat.id.clone());
                    Some(settled(open, true))
                }
                ChatEvent::PermissionRequest { request_id, .. }
                | ChatEvent::Question { request_id, .. }
                | ChatEvent::McpElicitation { request_id, .. } => {
                    open.insert(request_id.clone());
                    Some(ChatStatus::NeedsInput)
                }
                ChatEvent::RequestResolved { request_id, .. } => {
                    open.remove(request_id);
                    let turn = in_turn.contains(&chat.id);
                    if open.is_empty() && !turn {
                        // A background subagent's request answered while the agent rested.
                        self.send_next_queued(chat.id.clone());
                    }
                    open.is_empty().then(|| settled(open, turn))
                }
                ChatEvent::TurnEnded { outcome, .. } => {
                    in_turn.remove(&chat.id);
                    d.usage.turn_ended(chat.agent);
                    if *outcome == TurnOutcome::Success {
                        self.send_next_queued(chat.id.clone());
                    }
                    Some(settled(open, false))
                }
                _ => None,
            }
        };
        let tasks_changed = self.track_task(&chat.id, &event);
        let progress = crate::plan_progress::after(
            &event,
            self.plan_tasks
                .lock()
                .unwrap()
                .entry(chat.id.clone())
                .or_default(),
        );
        d.events.chat(&chat.workspace_id, &chat.id, event);
        if let Some(progress) = progress {
            self.set_progress(&chat, progress);
        }
        if tasks_changed {
            self.publish_tasks(&chat.id);
        }
        if turn_done {
            self.name_workspace_again(&chat.workspace_id);
        }
        if turn_ended {
            self.forget_auto_resume(&chat.id);
        }
        if let Some(status) = next
            && status != chat.status
            && chat.status != ChatStatus::Closed
        {
            self.set_status(&chat, status);
        }
    }

    /// The agent said which permission mode it runs in: clients see it on the chat.
    fn effective_mode(&self, chat: ChatInfo, mode: PermissionMode) {
        let before = self
            .effective_modes
            .lock()
            .unwrap()
            .insert(chat.id.clone(), mode);
        if before != Some(mode) {
            self.d().events.registry(RegistryEvent::ChatUpserted {
                chat: self.as_seen(chat),
            });
        }
    }

    fn mcp_status(&self, chat: ChatInfo, servers: Vec<McpServer>) {
        let before = self
            .mcp_servers
            .lock()
            .unwrap()
            .insert(chat.id.clone(), servers.clone());
        if before.as_ref() != Some(&servers) {
            self.d().events.registry(RegistryEvent::ChatUpserted {
                chat: self.as_seen(chat),
            });
        }
    }

    /// Connects one of a chat's MCP servers again: its login (the URL for the user to open) when
    /// it needs one, a reconnection otherwise. The session reports the outcome with `mcp_status`.
    pub async fn mcp_connect(&self, key: &str, server: &str) -> Result<McpConnectResult> {
        let chat = self.resolve(key)?;
        let _admission = self.d().execution.admit(&chat.workspace_id)?;
        if !self.is_live(&chat.id) {
            return Err(Error::Conflict(
                "the chat's agent is not running: send it a message first".into(),
            ));
        }
        let known = self
            .mcp_servers
            .lock()
            .unwrap()
            .get(&chat.id)
            .is_some_and(|servers| servers.iter().any(|s| s.name == server));
        if !known {
            return Err(Error::NotFound(format!(
                "no MCP server {server} in this chat"
            )));
        }
        let value = self
            .sidecar
            .call(SidecarCall::SessionMcpConnect(SessionMcpParams {
                chat_id: chat.id.clone(),
                server: server.to_string(),
            }))
            .await?;
        Ok(serde_json::from_value(value)?)
    }

    /// Follows the tasks a chat's agent runs; true when their number changed.
    fn track_task(&self, chat_id: &str, event: &ChatEvent) -> bool {
        let changed = {
            let mut tasks = self.tasks.lock().unwrap();
            match event {
                ChatEvent::TaskStarted { task_id, .. } => tasks
                    .entry(chat_id.to_string())
                    .or_default()
                    .insert(task_id.clone()),
                ChatEvent::TaskEnded { task_id, .. } => {
                    let removed = tasks
                        .get_mut(chat_id)
                        .is_some_and(|running| running.remove(task_id));
                    if tasks.get(chat_id).is_some_and(HashSet::is_empty) {
                        tasks.remove(chat_id);
                    }
                    removed
                }
                _ => false,
            }
        };
        if changed {
            self.save_tasks();
        }
        changed
    }

    /// Keeps the running tasks in the store, for a daemon restart to report them stopped.
    fn save_tasks(&self) {
        let json = serde_json::to_string(&*self.tasks.lock().unwrap()).unwrap_or_default();
        if let Err(error) = self.d().store.kv_set(RUNNING_TASKS_KEY, &json) {
            tracing::warn!(%error, "could not keep the running tasks");
        }
    }

    /// A chat's number of running tasks changed: the chat and its workspace are published again,
    /// for the sidebar of clients that do not follow the chat.
    fn publish_tasks(&self, chat_id: &str) {
        let d = self.d();
        let Ok(chat) = self.current(chat_id) else {
            return;
        };
        d.events
            .registry(RegistryEvent::ChatUpserted { chat: chat.clone() });
        d.workspaces.publish(&chat.workspace_id);
    }

    /// The agent session of a chat is gone (the sidecar exited, the daemon restarted): its tasks
    /// went with it.
    fn end_tasks(&self, chat_id: &str, why: &str) {
        let ids = self.tasks.lock().unwrap().remove(chat_id);
        let Some(ids) = ids else {
            return;
        };
        self.save_tasks();
        let d = self.d();
        let Ok(chat) = self.current(chat_id) else {
            return;
        };
        let mut ids: Vec<String> = ids.into_iter().collect();
        ids.sort();
        for task_id in ids {
            d.events.chat(
                &chat.workspace_id,
                &chat.id,
                ChatEvent::TaskEnded {
                    task_id,
                    status: TaskStatus::Stopped,
                    summary: Some(why.to_string()),
                },
            );
        }
        self.publish_tasks(chat_id);
    }

    /// A tool result's images (a screenshot, an image the agent read), saved in the workspace's
    /// notes folder like a message's: the event keeps where they are, not their bytes.
    fn save_tool_images(&self, workspace_id: &str, event: ChatEvent) -> ChatEvent {
        let ChatEvent::ToolResult {
            tool_use_id,
            output,
            is_error,
            images: Some(images),
            ..
        } = event
        else {
            return event;
        };
        let saved: Vec<_> = match self.d().workspaces.row(workspace_id) {
            Ok(row) => images
                .iter()
                .take(attachments::MAX_ATTACHMENTS)
                .filter_map(|image| {
                    match attachments::save(
                        &row.path,
                        std::slice::from_ref(image),
                        attachments::Accept::Images,
                    ) {
                        Ok(mut saved) => saved.pop(),
                        Err(error) => {
                            tracing::warn!(%error, "could not keep an image of a tool result");
                            None
                        }
                    }
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        ChatEvent::ToolResult {
            tool_use_id,
            output,
            is_error,
            images: None,
            attachments: (!saved.is_empty()).then_some(saved),
        }
    }

    fn set_status(&self, chat: &ChatInfo, status: ChatStatus) {
        let d = self.d();
        let mut chat = match d.store.chat(&chat.id) {
            Ok(Some(chat)) => self.as_seen(chat),
            _ => chat.clone(),
        };
        if chat.status == status {
            return;
        }
        let before = chat.status;
        chat.status = status;
        if let Err(error) = d.store.update_chat(&chat) {
            tracing::error!(%error, "could not store a chat status");
        }
        if needs_attention(before, status)
            && let Err(error) = d.store.set_workspace_attention(&chat.workspace_id)
        {
            tracing::error!(%error, "could not mark a workspace unread");
        }
        d.events.chat(
            &chat.workspace_id,
            &chat.id,
            ChatEvent::StatusChanged { status },
        );
        d.events
            .registry(RegistryEvent::ChatUpserted { chat: chat.clone() });
        d.workspaces.publish(&chat.workspace_id);
        if before == ChatStatus::Working && status == ChatStatus::Idle {
            // Its agent may have committed, pushed or opened a pull request.
            crate::prs::forget_status(&chat.workspace_id);
            d.events.report(Activity::TurnEnded);
        }
    }

    /// An exploration became a workspace: its chats leave plan mode, and their sessions close so
    /// that the next message resumes them able to write, the conversation kept.
    pub async fn workspace_promoted(&self, workspace_id: &str, branch: &str) {
        let d = self.d();
        for mut chat in d.store.chats_of(workspace_id).unwrap_or_default() {
            if chat.permission_mode == PermissionMode::Plan {
                chat.permission_mode = PermissionMode::default();
                let _ = d.store.update_chat(&chat);
                d.events.registry(RegistryEvent::ChatUpserted {
                    chat: self.as_seen(chat.clone()),
                });
            }
            if self.is_live(&chat.id) {
                let _ = self
                    .sidecar
                    .call(SidecarCall::SessionStop(SessionParams {
                        chat_id: chat.id.clone(),
                    }))
                    .await;
                self.live.lock().unwrap().remove(&chat.id);
            }
            self.notice(
                &chat,
                NoticeLevel::Info,
                format!(
                    "This exploration is now a workspace on branch {branch}: from the next message the agent can edit files, run them and commit"
                ),
            );
        }
    }

    fn notice(&self, chat: &ChatInfo, level: NoticeLevel, message: String) {
        self.d().events.chat(
            &chat.workspace_id,
            &chat.id,
            ChatEvent::Notice {
                level,
                message,
                badge: None,
            },
        );
    }

    /// The sidecar died: every session is gone. While the daemon stops, chats keep their status,
    /// for its next start to resume their turns.
    pub fn on_sidecar_exit(&self) {
        let d = self.d();
        if d.shutdown.is_cancelled() {
            return;
        }
        self.live.lock().unwrap().clear();
        self.pending.lock().unwrap().clear();
        self.in_turn.lock().unwrap().clear();
        let with_tasks: Vec<String> = self.tasks.lock().unwrap().keys().cloned().collect();
        for chat_id in with_tasks {
            self.end_tasks(&chat_id, "The agent process stopped");
        }
        self.turns_cut("The agent process stopped", NoticeLevel::Error);
    }

    /// At daemon start: sessions of the previous daemon are gone.
    pub fn recover_after_restart(&self) {
        let d = self.d();
        // Tasks running when the previous daemon stopped went with its sidecar.
        let left: HashMap<String, HashSet<String>> = d
            .store
            .kv_get(RUNNING_TASKS_KEY)
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        if !left.is_empty() {
            *self.tasks.lock().unwrap() = left.clone();
            for chat_id in left.keys() {
                self.end_tasks(chat_id, "Chauffeur restarted");
            }
        }
        *self.auto_resumed.lock().unwrap() = d
            .store
            .kv_get(AUTO_RESUMED_KEY)
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        self.turns_cut(
            "Chauffeur restarted and interrupted the agent",
            NoticeLevel::Warning,
        );
    }

    /// The agents' sessions went away under their chats (`why` says how, to the user). A chat cut
    /// while working resumes its turn on its own (`resume_interrupted_turns`), unless it was
    /// resumed so already since its last turn; one waiting for the user waits for a message.
    fn turns_cut(&self, why: &str, level: NoticeLevel) {
        let d = self.d();
        let resume = d.settings.get().resume_interrupted_turns;
        for chat in d.store.all_chats().unwrap_or_default() {
            if !matches!(
                chat.status,
                ChatStatus::Working | ChatStatus::Starting | ChatStatus::NeedsInput
            ) {
                continue;
            }
            let again = self.forget_auto_resume(&chat.id);
            let resumes = resume
                && !again
                && chat.status != ChatStatus::NeedsInput
                && chat.agent_session_id.is_some();
            let message = if resumes {
                format!("{why}: the agent resumes its turn")
            } else if again {
                format!(
                    "{why} again before the agent's resumed turn ended; send a message to resume"
                )
            } else {
                format!("{why}; send a message to resume")
            };
            self.cancel_open_requests(&chat);
            self.notice(&chat, level, message);
            self.set_status(&chat, ChatStatus::Idle);
            if resumes {
                self.auto_resumed.lock().unwrap().insert(chat.id.clone());
                self.save_auto_resumed();
                self.resume_cut_turn(chat.id);
            }
        }
    }

    /// Closes the permission requests, questions and elicitations a cut session left open: their
    /// cards would stay answerable on screen, and no answer could reach a session that is gone.
    fn cancel_open_requests(&self, chat: &ChatInfo) {
        let d = self.d();
        for request_id in open_requests(&d.store, &chat.id) {
            d.events.chat(
                &chat.workspace_id,
                &chat.id,
                ChatEvent::RequestResolved {
                    request_id,
                    outcome: "cancelled".into(),
                    rules: None,
                },
            );
        }
    }

    /// Starts the session of a chat whose turn was cut, telling the agent what happened.
    fn resume_cut_turn(&self, chat_id: String) {
        let d = self.d();
        tokio::spawn(async move {
            let resumed = async {
                let chat = d.chats.current(&chat_id)?;
                let admission = d.execution.admit(&chat.workspace_id)?;
                d.chats
                    .start_session_admitted(
                        &chat,
                        Some(RESUME_PROMPT.to_string()),
                        Vec::new(),
                        admission,
                    )
                    .await
            }
            .await;
            let Err(error) = resumed else { return };
            tracing::warn!(%error, chat = chat_id, "could not resume an interrupted turn");
            d.chats.forget_auto_resume(&chat_id);
            // The daemon stops before the turn could resume: the next one resumes it, its one
            // resume not spent.
            if d.shutdown.is_cancelled() {
                if let Ok(chat) = d.chats.current(&chat_id) {
                    d.chats.set_status(&chat, ChatStatus::Starting);
                }
                return;
            }
            if let Ok(chat) = d.chats.current(&chat_id) {
                d.chats.notice(
                    &chat,
                    NoticeLevel::Error,
                    format!("Could not resume the agent's turn: {error}; send a message to resume"),
                );
            }
        });
    }

    /// The user is back: each idle chat whose last turn failed on an error that trying again could
    /// cure (no network, DNS, an overloaded API) resumes it, once per message of the user's, even
    /// hours later. Returns how many chats resumed.
    pub fn resume_failed_turns(&self) -> Result<usize> {
        let d = self.d();
        if !d.settings.get().resume_interrupted_turns {
            return Ok(0);
        }
        let mut resumed = 0;
        for chat in d.store.all_chats()? {
            if chat.status != ChatStatus::Idle || chat.closed || chat.agent_session_id.is_none() {
                continue;
            }
            let Some(failed) = d
                .store
                .last_events_of_type(&chat.id, "turn_ended", 1)?
                .pop()
            else {
                continue;
            };
            let Event::Chat {
                event:
                    ChatEvent::TurnEnded {
                        outcome: TurnOutcome::Error,
                        error: Some(error),
                        ..
                    },
                ..
            } = &failed.event
            else {
                continue;
            };
            let asked = self.last_seq(&chat.id, "user_message")?;
            let key = format!("{FAILED_RESUME_KEY}{}", chat.id);
            if !is_retryable(error)
                || asked > failed.seq.unwrap_or(0)
                || d.store.kv_get(&key)?.as_deref() == Some(&asked.to_string())
            {
                continue;
            }
            d.store.kv_set(&key, &asked.to_string())?;
            self.notice(
                &chat,
                NoticeLevel::Warning,
                format!("The last turn failed ({error}): the agent resumes it"),
            );
            self.resume_failed_turn(chat.id, retry_prompt(error, &failed.ts));
            resumed += 1;
        }
        Ok(resumed)
    }

    /// Hands the agent of a chat whose turn failed the prompt to carry on.
    fn resume_failed_turn(&self, chat_id: String, prompt: String) {
        let d = self.d();
        tokio::spawn(async move {
            let resumed = async {
                let chat = d.chats.current(&chat_id)?;
                let admission = d.execution.admit(&chat.workspace_id)?;
                d.chats
                    .start_session_admitted(&chat, Some(prompt), Vec::new(), admission)
                    .await
            }
            .await;
            let Err(error) = resumed else { return };
            tracing::warn!(%error, chat = chat_id, "could not resume a failed turn");
            if let Ok(chat) = d.chats.current(&chat_id) {
                d.chats.notice(
                    &chat,
                    NoticeLevel::Error,
                    format!("Could not resume the agent's turn: {error}; send a message to resume"),
                );
            }
        });
    }

    /// The chat ended a turn, or was cut again: Chauffeur may resume it again. Returns whether it
    /// had been resumed on its own.
    fn forget_auto_resume(&self, chat_id: &str) -> bool {
        let removed = self.auto_resumed.lock().unwrap().remove(chat_id);
        if removed {
            self.save_auto_resumed();
        }
        removed
    }

    fn save_auto_resumed(&self) {
        let json = serde_json::to_string(&*self.auto_resumed.lock().unwrap()).unwrap_or_default();
        if let Err(error) = self.d().store.kv_set(AUTO_RESUMED_KEY, &json) {
            tracing::warn!(%error, "could not keep the chats resumed after a restart");
        }
    }

    /// Checked handoff stop for registered sessions only. The caller must install/drain the
    /// admission fence and hold its stop-attempt guard. Preserve tabs, conversations and queues.
    /// Old generations and missing/legacy receipts remain uncertain, even after a successful RPC.
    pub(crate) async fn stop_workspace_and_wait(&self, workspace: &str) -> Result<()> {
        let d = self.d();
        let mut ids: HashSet<String> = d
            .store
            .chats_of(workspace)?
            .into_iter()
            .filter(|chat| self.is_live(&chat.id))
            .map(|chat| chat.id)
            .collect();
        ids.extend(
            crate::workspace_launches::unresolved(&d.store, workspace)?
                .into_iter()
                .map(|launch| launch.chat),
        );
        let results =
            futures_util::future::join_all(ids.iter().map(|id| self.stop_agent_checked(id))).await;
        for result in results {
            result?;
        }
        Ok(())
    }

    async fn stop_agent_checked(&self, id: &str) -> Result<()> {
        let lock = self.start_lock(id);
        let _guard = lock.lock().await;
        let result = self
            .sidecar
            .call(SidecarCall::SessionStop(SessionParams {
                chat_id: id.to_owned(),
            }))
            .await?;
        if !result
            .get("confirmed_launch_ids")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|ids| !ids.is_empty())
        {
            return Err(Error::Unavailable(
                "Native session stop has no exact launch receipt; retain the workspace fence"
                    .into(),
            ));
        }
        self.live.lock().unwrap().remove(id);
        self.pending.lock().unwrap().remove(id);
        self.stale_briefing.lock().unwrap().remove(id);
        self.in_turn.lock().unwrap().remove(id);
        self.end_tasks(id, "The workspace is stopping for handoff");
        if let Ok(chat) = self.current(id)
            && matches!(
                chat.status,
                ChatStatus::Working | ChatStatus::Starting | ChatStatus::NeedsInput
            )
        {
            self.set_status(&chat, ChatStatus::Idle);
        }
        Ok(())
    }

    /// Stops the sessions of an archived workspace; its chats become read-only.
    pub async fn close_workspace(&self, workspace_id: &str) {
        let d = self.d();
        for chat in d.store.chats_of(workspace_id).unwrap_or_default() {
            if self.is_live(&chat.id) {
                let _ = self
                    .sidecar
                    .call(SidecarCall::SessionStop(SessionParams {
                        chat_id: chat.id.clone(),
                    }))
                    .await;
                self.live.lock().unwrap().remove(&chat.id);
            }
            self.pending.lock().unwrap().remove(&chat.id);
            self.in_turn.lock().unwrap().remove(&chat.id);
            self.end_tasks(&chat.id, "The workspace was archived");
            self.set_status(&chat, ChatStatus::Closed);
        }
    }

    /// Shows the first message of a chat whose workspace is being created, and keeps it for the
    /// agent until the workspace is ready.
    /// The first message of a chat made while its workspace is being created, given once the
    /// chat exists (an imported design's chat, whose folder the message names): the agent gets
    /// it once the workspace is ready, as `create`'s prompt.
    pub(crate) fn hold_first(
        &self,
        chat: &ChatInfo,
        text: String,
        images: &[ImageInput],
    ) -> Result<()> {
        let workspace = self.d().workspaces.row(&chat.workspace_id)?;
        self.hold_first_message(chat, &workspace, text, images)
    }

    fn hold_first_message(
        &self,
        chat: &ChatInfo,
        workspace: &WorkspaceRow,
        text: String,
        images: &[ImageInput],
    ) -> Result<()> {
        let d = self.d();
        let attachments = attachments::save(&workspace.path, images, attachments::Accept::Files)?;
        if let Err(error) = d.store.count_turn(&chat.workspace_id) {
            tracing::warn!(%error, workspace = chat.workspace_id, "could not count the message");
        }
        let sent = d
            .events
            .chat(
                &chat.workspace_id,
                &chat.id,
                ChatEvent::UserMessage {
                    text: text.clone(),
                    queued: false,
                    attachments: (!attachments.is_empty()).then(|| attachments.clone()),
                    message_id: Some(uuid::Uuid::now_v7().to_string()),
                    suggested_by: None,
                    delegated_answer: None,
                },
            )
            .seq
            .unwrap_or(i64::MAX);
        self.first_messages
            .lock()
            .unwrap()
            .entry(workspace.id.clone())
            .or_default()
            .push(FirstMessage {
                chat_id: chat.id.clone(),
                text,
                attachments,
                sent,
            });
        // A creation that failed meanwhile took the chat away, maybe after it forgot the first
        // messages: this one would be kept for nothing.
        if d.store.chat(&chat.id)?.is_none() {
            let mut all = self.first_messages.lock().unwrap();
            if let Some(firsts) = all.get_mut(&workspace.id) {
                firsts.retain(|first| first.chat_id != chat.id);
                if firsts.is_empty() {
                    all.remove(&workspace.id);
                }
            }
            return Err(Error::not_found(format!("workspace {}", workspace.slug)));
        }
        Ok(())
    }

    /// The workspace is ready: the agents of the chats opened while it was created start, on
    /// their first message, then on the messages sent meanwhile.
    pub async fn workspace_ready(&self, workspace_id: &str) {
        let d = self.d();
        let firsts = self
            .first_messages
            .lock()
            .unwrap()
            .remove(workspace_id)
            .unwrap_or_default();
        let Ok(workspace) = d.workspaces.row(workspace_id) else {
            return;
        };
        for first in firsts {
            let Ok(chat) = self.current(&first.chat_id) else {
                continue;
            };
            self.name_workspace_from(workspace_id, &first.text);
            let attachments = attachments::moved_to(&workspace.path, first.attachments);
            let text = self
                .with_handoff(&chat, first.text.clone(), first.sent)
                .unwrap_or_else(|error| {
                    tracing::warn!(%error, chat = chat.id, "could not read the conversation to hand over");
                    first.text
                });
            if let Err(error) = self.start_session(&chat, Some(text), attachments).await {
                tracing::warn!(%error, chat = chat.id, "the first message could not start the agent");
            }
        }
        for chat in d.store.chats_of(workspace_id).unwrap_or_default() {
            if chat.status == ChatStatus::Starting && !self.is_live(&chat.id) {
                self.set_status(&chat, ChatStatus::Idle);
                self.send_next_queued(chat.id.clone());
            }
        }
    }

    /// A creation that failed: its chats' first messages go with it.
    pub fn forget_creation(&self, workspace_id: &str) {
        self.first_messages.lock().unwrap().remove(workspace_id);
    }

    fn touch(&self, chat_id: &str) {
        self.activity
            .lock()
            .unwrap()
            .insert(chat_id.to_string(), std::time::Instant::now());
    }

    /// How long a chat has done nothing (since the daemon started, for one silent since).
    fn quiet_for(&self, chat_id: &str) -> std::time::Duration {
        self.activity
            .lock()
            .unwrap()
            .get(chat_id)
            .copied()
            .unwrap_or(self.started)
            .elapsed()
    }

    /// Closes the sessions of the chats idle for `after`, to free their memory; the sidecar keeps
    /// one that is still busy (a background task that will wake the agent). The next message
    /// resumes the chat with its whole conversation.
    pub async fn close_idle_sessions(&self, after: std::time::Duration) {
        let live: Vec<String> = self.live.lock().unwrap().iter().cloned().collect();
        for id in live {
            if self.quiet_for(&id) < after {
                continue;
            }
            let lock = self
                .start_locks
                .lock()
                .unwrap()
                .entry(id.clone())
                .or_default()
                .clone();
            let _guard = lock.lock().await;
            let Ok(chat) = self.current(&id) else {
                continue;
            };
            if chat.status != ChatStatus::Idle || !self.is_live(&id) {
                continue;
            }
            match self
                .sidecar
                .call(SidecarCall::SessionCloseIdle(SessionParams {
                    chat_id: id.clone(),
                }))
                .await
                .map(|value| value["closed"].as_bool() == Some(true))
            {
                Ok(true) => {
                    self.live.lock().unwrap().remove(&id);
                    tracing::info!(chat = %id, "idle session closed");
                }
                Ok(false) => {}
                Err(error) => tracing::warn!(%error, chat = %id, "could not close an idle session"),
            }
        }
    }

    /// Whether a workspace's agents have all been left alone for `after`: none working, waiting
    /// for an answer or holding a session.
    pub fn workspace_quiet(&self, workspace_id: &str, after: std::time::Duration) -> bool {
        let chats = self.d().store.chats_of(workspace_id).unwrap_or_default();
        chats.iter().all(|chat| {
            !self.is_live(&chat.id)
                && !matches!(
                    chat.status,
                    ChatStatus::Working | ChatStatus::NeedsInput | ChatStatus::Starting
                )
                && self.quiet_for(&chat.id) >= after
        })
    }

    /// When a workspace's chats last did something (since the daemon started, for silent ones).
    pub fn workspace_quiet_since(&self, workspace_id: &str) -> std::time::Instant {
        let activity = self.activity.lock().unwrap();
        self.d()
            .store
            .chats_of(workspace_id)
            .unwrap_or_default()
            .iter()
            .filter_map(|chat| activity.get(&chat.id).copied())
            .max()
            .unwrap_or(self.started)
    }

    /// Stops the idle sessions of `agent`'s chats: their next message resumes them with the
    /// settings of now.
    pub async fn restart_idle(&self, agent: AgentKind) {
        let live: Vec<String> = self.live.lock().unwrap().iter().cloned().collect();
        for id in live {
            let Ok(chat) = self.current(&id) else {
                continue;
            };
            if chat.agent != agent || chat.status != ChatStatus::Idle {
                continue;
            }
            let _ = self
                .sidecar
                .call(SidecarCall::SessionStop(SessionParams {
                    chat_id: id.clone(),
                }))
                .await;
            self.live.lock().unwrap().remove(&id);
        }
    }

    /// The variables the broken MCP servers of the running chats lack, by name.
    pub fn missing_env(&self) -> std::collections::BTreeSet<String> {
        let live = self.live.lock().unwrap().clone();
        self.mcp_servers
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| live.contains(*id))
            .flat_map(|(_, servers)| servers.iter())
            .filter(|server| server.status.is_broken())
            .flat_map(|server| server.missing_env.iter().cloned())
            .collect()
    }

    /// The login shell's environment changed: the idle sessions of chats with a broken server
    /// that takes a value from one of `set` (the variables that became set or changed, to a
    /// value) start again with it, the conversation kept; a busy one closes at its next message
    /// once idle, never interrupted. A server broken for another reason restarts nothing.
    /// Returns the chats restarted and those left for later.
    pub async fn restart_for_env(
        &self,
        set: &std::collections::BTreeSet<String>,
    ) -> (Vec<String>, Vec<String>) {
        let waiting: Vec<String> = {
            let live = self.live.lock().unwrap().clone();
            self.mcp_servers
                .lock()
                .unwrap()
                .iter()
                .filter(|(id, servers)| {
                    live.contains(*id)
                        && servers.iter().any(|server| {
                            server.status.is_broken()
                                && server
                                    .uses_env
                                    .iter()
                                    .chain(&server.missing_env)
                                    .any(|name| set.contains(name))
                        })
                })
                .map(|(id, _)| id.clone())
                .collect()
        };
        let (mut restarted, mut deferred) = (Vec::new(), Vec::new());
        for id in waiting {
            let stopped = {
                let lock = self.start_lock(&id);
                let _guard = lock.lock().await;
                let Ok(chat) = self.current(&id) else {
                    continue;
                };
                if !self.is_live(&id) {
                    continue;
                }
                if chat.status != ChatStatus::Idle || self.agent_busy(&chat).unwrap_or(true) {
                    self.stale_briefing.lock().unwrap().insert(id.clone());
                    deferred.push(id.clone());
                    continue;
                }
                if let Err(error) = self
                    .sidecar
                    .call(SidecarCall::SessionStop(SessionParams {
                        chat_id: id.clone(),
                    }))
                    .await
                {
                    tracing::warn!(%error, chat = %id, "could not stop a session to restart it");
                    continue;
                }
                self.live.lock().unwrap().remove(&id);
                chat
            };
            match self.start_session(&stopped, None, Vec::new()).await {
                Ok(()) => restarted.push(id),
                Err(error) => tracing::warn!(%error, chat = %id, "could not restart a session"),
            }
        }
        (restarted, deferred)
    }

    /// A workspace's tickets changed: its agents' sessions, started with the old briefing, close
    /// as soon as they are idle (now, or at their next message), so that their next message
    /// resumes them with the new one, the conversation kept. A working agent is never interrupted.
    pub async fn briefing_changed(&self, workspace_id: &str, tell: bool) {
        let notice = tell.then_some(
            "The Linear tickets of this workspace changed: the agent reads them from your next message",
        );
        self.sessions_outdated(workspace_id, notice).await;
    }

    /// A repo was added to a workspace: its agents' sessions resume with it (its folder,
    /// instructions, hooks, skills) as for a change of tickets.
    pub async fn repo_added(&self, workspace_id: &str, repo: &str) {
        let notice = format!(
            "{repo} was added to this workspace: the agent takes in its instructions, hooks and skills from your next message"
        );
        self.sessions_outdated(workspace_id, Some(&notice)).await;
    }

    /// The workspace's live sessions started from an outdated briefing or layout: each closes as
    /// soon as it is idle (now, or at its next message), and the next message resumes it, the
    /// conversation kept. `notice` is told to the chats that already talked to their agent.
    async fn sessions_outdated(&self, workspace_id: &str, notice: Option<&str>) {
        let d = self.d();
        for chat in d.store.chats_of(workspace_id).unwrap_or_default() {
            if !self.is_live(&chat.id) {
                continue;
            }
            self.stale_briefing.lock().unwrap().insert(chat.id.clone());
            if chat.status == ChatStatus::Idle {
                self.close_for_briefing(&chat.id, false).await;
            }
            if let Some(notice) = notice
                && chat.agent_session_id.is_some()
            {
                self.notice(&chat, NoticeLevel::Info, notice.to_string());
            }
        }
    }

    /// Closes an idle session started with an old briefing, unless the agent still runs a
    /// background task; `wait`: for a session starting meanwhile, else leaves it be.
    async fn close_for_briefing(&self, id: &str, wait: bool) {
        let lock = self
            .start_locks
            .lock()
            .unwrap()
            .entry(id.to_string())
            .or_default()
            .clone();
        let _guard = if wait {
            lock.lock().await
        } else {
            match lock.try_lock() {
                Ok(guard) => guard,
                Err(_) => return,
            }
        };
        let Ok(chat) = self.current(id) else {
            return;
        };
        if chat.status != ChatStatus::Idle || !self.is_live(id) {
            return;
        }
        match self
            .sidecar
            .call(SidecarCall::SessionCloseIdle(SessionParams {
                chat_id: id.to_string(),
            }))
            .await
            .map(|value| value["closed"].as_bool() == Some(true))
        {
            Ok(true) => {
                self.live.lock().unwrap().remove(id);
                self.stale_briefing.lock().unwrap().remove(id);
            }
            Ok(false) => {}
            Err(error) => tracing::warn!(%error, chat = %id, "could not close a session"),
        }
    }

    /// A restored workspace's chat can take messages again.
    pub fn unarchive(&self, chat_id: &str) -> Result<()> {
        let chat = self.current(chat_id)?;
        self.set_status(&chat, ChatStatus::Idle);
        Ok(())
    }

    /// The workspace of a chat the user renames, closes, reopens or deletes: ready (or, for a
    /// rename, being created); an archived workspace's chats are read-only.
    fn editable_workspace(&self, chat: &ChatInfo, creating_too: bool) -> Result<WorkspaceRow> {
        let workspace = self.d().workspaces.row(&chat.workspace_id)?;
        match workspace.state {
            WorkspaceState::Ready => Ok(workspace),
            WorkspaceState::Creating if creating_too => Ok(workspace),
            WorkspaceState::Creating => Err(Error::Conflict(format!(
                "{} is being created: wait until it is ready",
                workspace.slug
            ))),
            state => Err(Error::Conflict(format!(
                "{} is {}: its chats are read-only",
                workspace.slug,
                state.as_str()
            ))),
        }
    }

    /// Whether a chat's agent has something going on that closing it would stop: a turn, a
    /// question or permission request, background tasks, messages waiting in its queue.
    pub fn agent_busy(&self, chat: &ChatInfo) -> Result<bool> {
        Ok(matches!(
            chat.status,
            ChatStatus::Working | ChatStatus::NeedsInput | ChatStatus::Starting
        ) || self.running_tasks(&chat.id) > 0
            || self
                .pending
                .lock()
                .unwrap()
                .get(&chat.id)
                .is_some_and(|open| !open.is_empty())
            || !self.d().store.queue_of(&chat.id)?.is_empty())
    }

    fn refuse_if_busy(&self, chat: &ChatInfo, force: bool, what: &str) -> Result<()> {
        if !force && self.agent_busy(chat)? {
            return Err(Error::Conflict(format!(
                "the agent of this chat is working: stop it first, or {what} with force"
            )));
        }
        Ok(())
    }

    /// Ends a chat's agent session, its turn, background tasks and open requests with it, and
    /// empties its queue: the chat is left idle, its conversation kept for a resume.
    async fn stop_agent(&self, chat_id: &str, why: &str) {
        if self.is_live(chat_id) {
            if let Err(error) = self
                .sidecar
                .call(SidecarCall::SessionStop(SessionParams {
                    chat_id: chat_id.to_string(),
                }))
                .await
            {
                tracing::warn!(%error, chat = chat_id, "could not stop a chat's session");
            }
            self.live.lock().unwrap().remove(chat_id);
        }
        self.pending.lock().unwrap().remove(chat_id);
        self.stale_briefing.lock().unwrap().remove(chat_id);
        self.end_tasks(chat_id, why);
        let Ok(chat) = self.current(chat_id) else {
            return;
        };
        if !self
            .d()
            .store
            .queue_of(chat_id)
            .unwrap_or_default()
            .is_empty()
        {
            match self.d().store.queue_set(chat_id, &[]) {
                Ok(()) => {
                    let _ = self.publish_queue(&chat);
                }
                Err(error) => tracing::warn!(%error, chat = chat_id, "could not empty a queue"),
            }
        }
        if matches!(
            chat.status,
            ChatStatus::Working | ChatStatus::NeedsInput | ChatStatus::Starting
        ) {
            self.set_status(&chat, ChatStatus::Idle);
        }
    }

    fn set_closed(&self, chat_id: &str, closed: bool) -> Result<ChatInfo> {
        let d = self.d();
        d.store.set_chat_closed(chat_id, closed)?;
        let chat = self.current(chat_id)?;
        d.events
            .registry(RegistryEvent::ChatUpserted { chat: chat.clone() });
        d.workspaces.publish(&chat.workspace_id);
        Ok(chat)
    }

    /// A workspace keeps one open chat at least: a new, empty one when the last was closed or
    /// deleted.
    async fn keep_one_open(
        &self,
        workspace_id: &str,
        admission: &crate::workspace_execution::Admission,
    ) -> Result<()> {
        let chats = self.d().store.chats_of(workspace_id)?;
        if chats.iter().all(|chat| chat.closed) {
            self.create_in(
                CreateChatParams {
                    workspace: workspace_id.to_string(),
                    ..Default::default()
                },
                None,
                Some(admission),
            )
            .await?;
        }
        Ok(())
    }

    fn agent_lock(&self, chat_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.agent_locks
            .lock()
            .unwrap()
            .entry(chat_id.to_string())
            .or_default()
            .clone()
    }

    fn start_lock(&self, chat_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.start_locks
            .lock()
            .unwrap()
            .entry(chat_id.to_string())
            .or_default()
            .clone()
    }

    /// A design chat's message, followed by why its canvas.json is invalid, when it is: the user
    /// sees a banner on the canvas, the agent reads it at its next turn.
    fn with_canvas_note(&self, chat: &ChatInfo, text: &str) -> String {
        if chat.kind != ChatKind::Design {
            return text.to_string();
        }
        let Ok(workspace) = self.d().workspaces.row(&chat.workspace_id) else {
            return text.to_string();
        };
        let dir = std::path::Path::new(&workspace.path)
            .join(crate::paths::NOTES_DIR)
            .join(crate::paths::DESIGNS_DIR)
            .join(&chat.id);
        match crate::canvas::manifest_error(&dir) {
            Some(error) => format!(
                "{text}\n\n(Chauffeur: your canvas.json is invalid, so the canvas sorts the screens by itself: {error}. Fix it as you go on.)"
            ),
            None => text.to_string(),
        }
    }

    /// What a chat's tab reads: its title, else its first message.
    fn label_of(&self, chat: &ChatInfo) -> Option<String> {
        if let Some(title) = chat.title.clone().filter(|t| !t.trim().is_empty()) {
            return Some(title);
        }
        let first = self
            .d()
            .store
            .last_events_of_type(&chat.id, "user_message", u32::MAX)
            .ok()?
            .into_iter()
            .last()?;
        match first.event {
            Event::Chat {
                event: ChatEvent::UserMessage { text, .. },
                ..
            } if !text.trim().is_empty() => Some(title_of(&text)),
            _ => None,
        }
    }

    /// Writes a design chat's design brief again from its source chat, until its first message.
    pub fn regenerate_brief(&self, key: &str) -> Result<ChatInfo> {
        let chat = self.resolve(key)?;
        self.editable_workspace(&chat, false)?;
        let _admission = self.d().execution.admit(&chat.workspace_id)?;
        if chat.kind != ChatKind::Design || chat.source_chat.is_none() {
            return Err(Error::BadRequest(
                "only a design chat started from another chat has a brief".into(),
            ));
        }
        if chat.brief.is_none() {
            return Err(Error::Conflict(
                "this design chat had its first message: its brief is gone".into(),
            ));
        }
        self.set_brief(
            &chat.id,
            DesignBrief {
                state: BriefState::Writing,
                text: None,
                error: None,
            },
        )?;
        self.spawn_brief(&chat.id);
        self.current(&chat.id)
    }

    fn set_brief(&self, chat_id: &str, brief: DesignBrief) -> Result<()> {
        let d = self.d();
        d.store.set_chat_brief(chat_id, Some(&brief))?;
        d.events.registry(RegistryEvent::ChatUpserted {
            chat: self.current(chat_id)?,
        });
        Ok(())
    }

    /// The design chat had its first message: its brief is spent, and one still being written is
    /// dropped when it comes.
    fn drop_brief(&self, chat_id: &str) -> Result<()> {
        let d = self.d();
        self.brief_runs.lock().unwrap().remove(chat_id);
        d.store.set_chat_brief(chat_id, None)?;
        d.events.registry(RegistryEvent::ChatUpserted {
            chat: self.current(chat_id)?,
        });
        Ok(())
    }

    /// Writes the design brief in the background; the chat shows it once ready, or why it failed.
    fn spawn_brief(&self, chat_id: &str) {
        let run = {
            let mut runs = self.brief_runs.lock().unwrap();
            let run = runs.get(chat_id).map_or(1, |run| run + 1);
            runs.insert(chat_id.to_string(), run);
            run
        };
        let d = self.d();
        let chat_id = chat_id.to_string();
        tokio::spawn(async move {
            let outcome = d.chats.write_brief(&chat_id).await;
            if d.chats.brief_runs.lock().unwrap().get(&chat_id) != Some(&run) {
                return;
            }
            let brief = match outcome {
                Ok(text) => DesignBrief {
                    state: BriefState::Ready,
                    text: Some(text),
                    error: None,
                },
                Err(error) => {
                    tracing::warn!(%error, chat = chat_id, "no design brief");
                    DesignBrief {
                        state: BriefState::Failed,
                        text: None,
                        error: Some(error),
                    }
                }
            };
            // Gone meanwhile (deleted, or its first message sent): nothing to show.
            if let Ok(Some(chat)) = d.store.chat(&chat_id)
                && chat.brief.is_some()
                && let Err(error) = d.chats.set_brief(&chat_id, brief)
            {
                tracing::warn!(%error, chat = chat_id, "could not keep the design brief");
            }
        });
    }

    /// The brief of a design chat, from its source chat's stored conversation, by the design
    /// chat's model and effort.
    async fn write_brief(&self, chat_id: &str) -> std::result::Result<String, String> {
        let d = self.d();
        let chat = self.current(chat_id).map_err(|error| error.to_string())?;
        let source_id = chat
            .source_chat
            .clone()
            .ok_or("the chat has no source chat")?;
        let source = d
            .store
            .chat(&source_id)
            .map_err(|error| error.to_string())?
            .ok_or("its source chat was deleted")?;
        let events: Vec<ChatEvent> = d
            .store
            .events_of_chat(&source.id)
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter_map(|envelope| match envelope.event {
                Event::Chat { event, .. } => Some(event),
                _ => None,
            })
            .collect();
        let conversation = design::conversation_text(&events, design::CONVERSATION_BUDGET);
        if conversation.trim().is_empty() {
            return Err("the source chat has no messages yet".into());
        }
        let call = self.sidecar.call(SidecarCall::AgentBrief(AgentBriefParams {
            conversation,
            title: source.title.clone(),
            model: chat.model.clone(),
            effort: chat.effort.clone(),
        }));
        match tokio::time::timeout(BRIEF_TIMEOUT, call).await {
            Ok(Ok(value)) => serde_json::from_value::<AgentBriefResult>(value)
                .map_err(|error| error.to_string())
                .map(|result| result.brief.trim().to_string())
                .and_then(|brief| {
                    if brief.is_empty() {
                        Err("Claude wrote an empty brief".to_string())
                    } else {
                        Ok(brief)
                    }
                }),
            Ok(Err(error)) => Err(error.to_string()),
            Err(_) => Err(format!(
                "Claude did not answer within {} s",
                BRIEF_TIMEOUT.as_secs()
            )),
        }
    }

    /// Changes the title a chat's tab shows; empty gives back the automatic one (from its first
    /// message, else none).
    pub fn rename(&self, key: &str, title: &str) -> Result<ChatInfo> {
        let d = self.d();
        let chat = self.resolve(key)?;
        self.editable_workspace(&chat, true)?;
        let _admission = self.d().execution.admit(&chat.workspace_id)?;
        let title: String = title
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(200)
            .collect();
        d.store
            .set_chat_title(&chat.id, (!title.is_empty()).then_some(title.as_str()))?;
        let chat = self.current(&chat.id)?;
        d.events
            .registry(RegistryEvent::ChatUpserted { chat: chat.clone() });
        Ok(chat)
    }

    /// Moves a chat's tab before another chat of its workspace (`before`), or last: every chat
    /// of the workspace, closed ones too, is numbered again in the new order, and each one whose
    /// place changed is published.
    pub fn move_tab(&self, key: &str, before: Option<&str>) -> Result<ChatInfo> {
        let d = self.d();
        let chat = self.resolve(key)?;
        self.editable_workspace(&chat, true)?;
        let _admission = d.execution.admit(&chat.workspace_id)?;
        let chats = d.store.chats_of(&chat.workspace_id)?;
        if let Some(before) = before
            && !chats.iter().any(|c| c.id == before)
        {
            return Err(Error::not_found(format!(
                "chat {before} among the chats of its workspace"
            )));
        }
        let order = moved_before(tab_order(&chats), &chat.id, before);
        d.store.order_chat_tabs(&order)?;
        for (i, id) in order.iter().enumerate() {
            let was = chats
                .iter()
                .find(|c| &c.id == id)
                .and_then(|c| c.tab_position);
            if was != Some(i as u32) {
                let chat = self.current(id)?;
                d.events.registry(RegistryEvent::ChatUpserted { chat });
            }
        }
        self.current(&chat.id)
    }

    /// Takes a chat out of the tabs, its conversation kept; its agent session stops to free its
    /// memory, and resumes at the next message after a reopen.
    pub async fn close(&self, key: &str, force: bool) -> Result<ChatInfo> {
        let chat = self.resolve(key)?;
        self.editable_workspace(&chat, false)?;
        let admission = self.d().execution.admit(&chat.workspace_id)?;
        if chat.closed {
            return Ok(chat);
        }
        self.refuse_if_busy(&chat, force, "close it")?;
        let closed = {
            let lock = self.start_lock(&chat.id);
            let _guard = lock.lock().await;
            self.stop_agent(&chat.id, "The chat was closed").await;
            self.set_closed(&chat.id, true)?
        };
        self.keep_one_open(&chat.workspace_id, &admission).await?;
        Ok(closed)
    }

    /// Puts a closed chat back among the tabs. Its agent starts again with its next message.
    pub fn reopen(&self, key: &str) -> Result<ChatInfo> {
        let chat = self.resolve(key)?;
        self.editable_workspace(&chat, false)?;
        let _admission = self.d().execution.admit(&chat.workspace_id)?;
        if !chat.closed {
            return Ok(chat);
        }
        self.set_closed(&chat.id, false)
    }

    /// Deletes a chat for good: its agent stopped, its row, queue and events gone. Named by its
    /// id only, never through its workspace. The files it saved in the notes folder are the
    /// user's and stay; so do the agents' own transcripts (`~/.claude`, `~/.codex`).
    pub async fn delete(&self, id: &str, force: bool) -> Result<ChatInfo> {
        let d = self.d();
        let chat = d
            .store
            .chat(id)?
            .map(|chat| self.as_seen(chat))
            .ok_or_else(|| Error::not_found(format!("chat {id}")))?;
        self.editable_workspace(&chat, false)?;
        let admission = self.d().execution.admit(&chat.workspace_id)?;
        self.refuse_if_busy(&chat, force, "delete it")?;
        {
            let lock = self.start_lock(&chat.id);
            let _guard = lock.lock().await;
            self.stop_agent(&chat.id, "The chat was deleted").await;
            d.shares.stop_for_chat(&chat.id, &chat.workspace_id);
            d.store.delete_chat(&chat.id)?;
        }
        self.forget_chat(&chat.id);
        d.events.registry(RegistryEvent::ChatRemoved {
            workspace_id: chat.workspace_id.clone(),
            chat_id: chat.id.clone(),
        });
        d.workspaces.publish(&chat.workspace_id);
        self.keep_one_open(&chat.workspace_id, &admission).await?;
        Ok(chat)
    }

    /// Forks a chat into a new tab of its workspace: the same agent, model, effort, speed and
    /// permission mode, a copy of its conversation (its event log here, its agent session in the
    /// agent's own store, which the fork's agent resumes as a session of its own), whole or up to
    /// the end of its turn numbered `turn`, and from there each chat goes its own way. Its queue,
    /// draft and running tasks stay with the original. Into the other `agent`, which cannot resume
    /// this one's session, the event log alone is copied: the fork's agent starts afresh and reads
    /// the conversation as text (`handoff`).
    pub async fn fork(
        &self,
        key: &str,
        turn: Option<u32>,
        agent: Option<AgentKind>,
    ) -> Result<ChatInfo> {
        let d = self.d();
        let source = self.resolve(key)?;
        let _admission = d.execution.admit(&source.workspace_id)?;
        let workspace = self.editable_workspace(&source, false)?;
        if source.kind == ChatKind::Design {
            return Err(Error::BadRequest(
                "a design chat cannot be forked: its screens are its own".into(),
            ));
        }
        // Copied mid-turn, the conversation would end on a step the fork's agent never finishes.
        let lock = self.start_lock(&source.id);
        let _guard = lock.lock().await;
        let source = self.current(&source.id)?;
        // A finished turn no longer changes: it forks while the agent runs a later one.
        let (up_to, from) = match turn {
            Some(turn) => {
                let (seq, point) = self.fork_point(&source.id, turn)?;
                (Some(seq), Some(point))
            }
            None => {
                let waiting = self
                    .pending
                    .lock()
                    .unwrap()
                    .get(&source.id)
                    .is_some_and(|open| !open.is_empty());
                if waiting
                    || matches!(
                        source.status,
                        ChatStatus::Working | ChatStatus::NeedsInput | ChatStatus::Starting
                    )
                {
                    return Err(Error::Conflict(
                        "the agent of this chat is working: fork it once its turn ends, fork from a turn it finished, or interrupt it first"
                            .into(),
                    ));
                }
                (None, None)
            }
        };
        let agent = agent.unwrap_or(source.agent);
        let across = agent != source.agent;
        // The turn's own session, which holds it even if the chat moved to another since; no
        // session yet, or the other agent's: nothing for the agent to carry on, the fork starts
        // afresh.
        let copy = match from {
            _ if across => None,
            Some(point) => Some((point.session_id, Some(point.up_to))),
            None => source.agent_session_id.clone().map(|id| (id, None)),
        };
        let agent_session_id = match copy {
            Some((session_id, up_to)) => {
                let mut env = d.workspaces.agent_env(&workspace, false)?;
                crate::ollama::codex_env(
                    &d.paths.home,
                    source.agent,
                    source.model.as_deref(),
                    &mut env,
                )?;
                let value = self
                    .sidecar
                    .call(SidecarCall::AgentFork(AgentForkParams {
                        agent: source.agent,
                        session_id,
                        cwd: workspace.path.clone(),
                        env,
                        up_to,
                    }))
                    .await?;
                let forked: AgentForkResult = serde_json::from_value(value)?;
                Some(forked.session_id)
            }
            None => None,
        };
        // Sessions of the other agent are none the fork's agent reads.
        let fork_sessions = if across {
            Vec::new()
        } else {
            let mut sessions = d.store.chat_fork_sessions(&source.id)?;
            sessions.extend(source.agent_session_id.clone());
            sessions
        };
        let (model, effort, fast) = if across {
            let choice = agent_choice(&d, agent)?;
            (
                Some(choice.model).filter(|model| !model.is_empty()),
                choice.effort,
                choice.fast,
            )
        } else {
            (source.model.clone(), source.effort.clone(), source.fast)
        };
        let forked_from_agent = if across {
            Some(source.agent)
        } else if agent_session_id.is_none() {
            // A fork of such a fork before its agent had a session: the conversation is still
            // only text.
            source.forked_from_agent
        } else {
            None
        };
        let label = self.label_of(&source);
        let chat = ChatInfo {
            id: uuid::Uuid::now_v7().to_string(),
            progress: None,
            tab_position: None,
            workspace_id: workspace.id.clone(),
            agent,
            model,
            effort,
            fast,
            permission_mode: source.permission_mode,
            title: Some(fork_title(label.as_deref())),
            status: ChatStatus::Idle,
            agent_session_id,
            created_at: now(),
            running_tasks: 0,
            permission_mode_effective: None,
            closed: false,
            last_activity_at: None,
            mcp_servers: Vec::new(),
            kind: ChatKind::Chat,
            source_chat: None,
            brief: None,
            // A fork of a delegated chat is the user's own: it answers no other chat.
            parent_chat: None,
            parent_tool_use: None,
            forked_from_agent,
        };
        let Some(copied) =
            d.store
                .insert_fork(&chat, &source.id, &fork_sessions, up_to, !across)?
        else {
            return Err(Error::not_found(format!("workspace {}", workspace.slug)));
        };
        let (open_tasks, queued) = left_open(&copied);
        // Clients following the workspace live see the conversation come, before the notice.
        d.events.announce(copied);
        // The original's tasks keep running there, or ran on there after the turn forked: the
        // fork shows them ended.
        let running: HashSet<String> = self
            .tasks
            .lock()
            .unwrap()
            .get(&source.id)
            .map(|tasks| tasks.iter().cloned().collect())
            .unwrap_or_default();
        for task_id in open_tasks {
            let summary = if running.contains(&task_id) {
                "It runs on in the original chat"
            } else {
                "It ran on in the original chat"
            };
            d.events.chat(
                &chat.workspace_id,
                &chat.id,
                ChatEvent::TaskEnded {
                    task_id,
                    status: TaskStatus::Stopped,
                    summary: Some(summary.into()),
                },
            );
        }
        // Messages queued then were sent in the original.
        if queued {
            d.events.chat(
                &chat.workspace_id,
                &chat.id,
                ChatEvent::QueueChanged {
                    messages: Vec::new(),
                },
            );
        }
        let from = match &label {
            Some(label) => format!("Forked from “{label}”"),
            None => "Forked from another chat".into(),
        };
        self.notice(
            &chat,
            NoticeLevel::Info,
            if across {
                format!(
                    "{from} into {}, which reads the conversation so far as text with your next message: from here on, each chat goes its own way.",
                    agent_name(agent)
                )
            } else {
                format!("{from}: from here on, each chat goes its own way.")
            },
        );
        let chat = self.current(&chat.id)?;
        d.events
            .registry(RegistryEvent::ChatUpserted { chat: chat.clone() });
        d.workspaces.publish(&workspace.id);
        Ok(chat)
    }

    /// Where a fork from a chat's turn numbered `turn` ends: the seq of its `turn_ended`, and the
    /// point of the agent session it ran in.
    fn fork_point(&self, chat_id: &str, turn: u32) -> Result<(i64, ForkPoint)> {
        let Some((seq, outcome, fork_point)) = self.d().store.turn_end(chat_id, turn)? else {
            return Err(Error::not_found(format!("turn {turn} of this chat")));
        };
        if self
            .d()
            .store
            .events_of_type_after(chat_id, "agent_changed", seq)?
            .iter()
            .any(|envelope| {
                matches!(
                    envelope.event,
                    Event::Chat {
                        event: ChatEvent::AgentChanged { .. },
                        ..
                    }
                )
            })
        {
            return Err(Error::BadRequest(
                "this turn ran before the provider changed: fork the whole chat instead".into(),
            ));
        }
        match (outcome, fork_point) {
            (TurnOutcome::Success, Some(point)) => Ok((seq, point)),
            (TurnOutcome::Success, ..) => Err(Error::BadRequest(format!(
                "turn {turn} ended before Chauffeur kept where to fork it from: fork the whole chat instead"
            ))),
            _ => Err(Error::BadRequest(format!(
                "turn {turn} did not finish: fork from one that did"
            ))),
        }
    }

    /// Drops what the daemon's memory held of a deleted chat.
    fn forget_chat(&self, chat_id: &str) {
        self.live.lock().unwrap().remove(chat_id);
        self.pending.lock().unwrap().remove(chat_id);
        self.start_locks.lock().unwrap().remove(chat_id);
        self.agent_locks.lock().unwrap().remove(chat_id);
        self.activity.lock().unwrap().remove(chat_id);
        self.stale_briefing.lock().unwrap().remove(chat_id);
        self.unread_told.lock().unwrap().remove(chat_id);
        self.brief_runs.lock().unwrap().remove(chat_id);
        for firsts in self.first_messages.lock().unwrap().values_mut() {
            firsts.retain(|first| first.chat_id != chat_id);
        }
        if self.tasks.lock().unwrap().remove(chat_id).is_some() {
            self.save_tasks();
        }
    }

    /// Last assistant text of the main agent (not of its subagents).
    pub fn last_text(&self, chat_id: &str) -> Result<Option<String>> {
        let d = self.d();
        for envelope in d.store.last_events_of_type(chat_id, "assistant_text", 50)? {
            if let Event::Chat {
                event:
                    ChatEvent::AssistantText {
                        text,
                        parent_tool_use_id: None,
                        ..
                    },
                ..
            } = envelope.event
            {
                return Ok(Some(text));
            }
        }
        Ok(None)
    }

    /// Claude's WorktreeCreate hook: a worktree group of every repo of the chat's workspace.
    pub async fn worktree_create(&self, chat_id: &str, name: &str) -> Result<String> {
        let d = self.d();
        let chat = self.current(chat_id)?;
        let result = d
            .workspaces
            .add_agent_worktrees(chauffeur_protocol::client::AddWorktreeParams {
                workspace: chat.workspace_id,
                repos: Vec::new(),
                name: Some(name.to_string()),
                branch: None,
            })
            .await?;
        Ok(result.path)
    }

    /// The agent's `list_repos` tool: the organization's repos (and the workspace's own, should
    /// the organization not list one), each with its folder when already in the workspace.
    pub async fn tool_list_repos(&self, chat_id: &str) -> Result<ToolListReposResult> {
        let d = self.d();
        let chat = self.current(chat_id)?;
        let workspace = d.workspaces.get(&chat.workspace_id)?;
        let listed = d.repos.list(false, &d.shell_env).await?;
        Ok(repos_for_agent(&workspace, listed))
    }

    /// The agent's `add_repo` tool, once the user said yes (the sidecar asked): the repo is added
    /// to the chat's workspace as the app adds it. Refused in a design chat, which has no such tool.
    pub async fn tool_add_repo(&self, chat_id: &str, repo: &str) -> Result<ToolAddRepoResult> {
        let d = self.d();
        let chat = self.current(chat_id)?;
        if chat.kind == ChatKind::Design {
            return Err(Error::Conflict("a design chat adds no repo".into()));
        }
        let (workspace, setup) = d.workspaces.add_repo_to(&chat.workspace_id, repo).await?;
        let worktree = workspace
            .worktrees
            .iter()
            .find(|w| !w.removed && w.origin == WorktreeOrigin::Workspace && w.repo == repo)
            .ok_or_else(|| {
                Error::Internal(anyhow::anyhow!("{repo} was added but has no worktree"))
            })?;
        Ok(ToolAddRepoResult {
            path: worktree.path.clone(),
            branch: (!workspace.explore && !worktree.branch.is_empty())
                .then(|| worktree.branch.clone()),
            setup,
        })
    }

    /// The agent's `delegate` tool (story 49): a delegated chat gets the task, a new one in the
    /// parent's workspace (its agent asked, the parent's permission mode) or one the parent
    /// started earlier, idle. Returns once the task is delivered; `tool_delegate_wait` waits for
    /// the answer.
    pub async fn tool_delegate(&self, params: ToolDelegateParams) -> Result<ToolDelegateStarted> {
        let parent = self.current(&params.chat_id)?;
        if parent.kind == ChatKind::Design {
            return Err(Error::Conflict("a design chat delegates no task".into()));
        }
        if parent.parent_chat.is_some() {
            return Err(Error::Conflict(
                "a delegated chat delegates no task itself: answer the agent that gave you your task"
                    .into(),
            ));
        }
        let prompt = params.prompt.trim();
        if prompt.is_empty() {
            return Err(Error::BadRequest("the task is empty".into()));
        }
        let mut notice = None;
        let child = match params
            .chat
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            Some(id) => {
                let child = self.child_of(&parent.id, id)?;
                if !self.chat_settled(&child, false)? || child.status == ChatStatus::NeedsInput {
                    return Err(Error::Conflict(format!(
                        "the delegated chat {id} is still working (the user may be talking with it): wait for it, or start another"
                    )));
                }
                self.send_with(&child.id, prompt, &[], SendMode::Steer)
                    .await?
            }
            None => {
                let choice = delegate_choice(&self.d(), &parent, &params).await?;
                notice = choice.notice;
                Box::pin(self.create_in(
                    CreateChatParams {
                        workspace: parent.workspace_id.clone(),
                        agent: Some(choice.agent),
                        model: choice.model,
                        effort: choice.effort,
                        fast: Some(choice.fast),
                        permission_mode: Some(parent.permission_mode),
                        prompt: Some(prompt.to_string()),
                        ..Default::default()
                    },
                    Some(Delegation {
                        chat: parent.id.clone(),
                        tool_use: params.tool_use_id.filter(|id| !id.is_empty()),
                    }),
                    None,
                ))
                .await?
            }
        };
        Ok(ToolDelegateStarted {
            chat: child.id,
            agent: child.agent,
            title: child.title,
            model: child.model,
            effort: child.effort,
            notice,
        })
    }

    /// Waits until a delegated chat's turn is over, through its questions to the user, and returns
    /// its answer.
    pub async fn tool_delegate_wait(
        &self,
        params: ToolDelegateChildParams,
    ) -> Result<ToolDelegateResult> {
        let child = self.child_of(&params.chat_id, &params.child)?;
        let (child, _) = self.wait_settled(&child.id, true, None).await?;
        self.delegate_result(&child)
    }

    /// Interrupts a delegated chat: its parent gave up waiting for it.
    pub async fn tool_delegate_stop(&self, params: ToolDelegateChildParams) -> Result<()> {
        let child = self.child_of(&params.chat_id, &params.child)?;
        self.interrupt(&child.id).await?;
        Ok(())
    }

    /// The parent's turn ended before the delegated chat answered, without an interrupt (Codex
    /// may end its turn while its tool calls still run): the delegated chat works on, and once
    /// it is done its answer goes to the parent as a new message, which wakes the parent's agent
    /// or steers its running turn. Lost if the daemon stops first; the delegated chat's own
    /// agent stops with it.
    pub async fn tool_delegate_detach(&self, params: ToolDelegateChildParams) -> Result<()> {
        let child = self.child_of(&params.chat_id, &params.child)?;
        let parent = params.chat_id;
        let d = self.d();
        tokio::spawn(async move {
            let forwarded = async {
                let (child, _) = d.chats.wait_settled(&child.id, true, None).await?;
                let result = d.chats.delegate_result(&child)?;
                let from = Sender::DelegatedChat(DelegatedAnswer {
                    chat: result.chat.clone(),
                    agent: result.agent,
                    title: result.title.clone(),
                });
                d.chats
                    .send_from(&parent, &late_answer(&result), &[], SendMode::Steer, from)
                    .await
            };
            if let Err(error) = forwarded.await {
                tracing::warn!(%error, parent, child = child.id, "could not send a delegated chat's answer to its parent");
            }
        });
        Ok(())
    }

    /// A delegated chat of `parent`, by id.
    fn child_of(&self, parent: &str, id: &str) -> Result<ChatInfo> {
        self.d()
            .store
            .chat(id)?
            .filter(|chat| chat.parent_chat.as_deref() == Some(parent))
            .map(|chat| self.as_seen(chat))
            .ok_or_else(|| Error::not_found(format!("delegated chat {id} of this chat")))
    }

    /// What a delegated chat answered: its last turn's outcome and message, when a turn ended
    /// since its last message.
    fn delegate_result(&self, chat: &ChatInfo) -> Result<ToolDelegateResult> {
        let d = self.d();
        let asked = self.last_seq(&chat.id, "user_message")?;
        let ended = d
            .store
            .last_events_of_type(&chat.id, "turn_ended", 1)?
            .into_iter()
            .next()
            .filter(|envelope| envelope.seq.unwrap_or(0) > asked);
        let (outcome, error) = match ended.map(|envelope| envelope.event) {
            Some(Event::Chat {
                event: ChatEvent::TurnEnded { outcome, error, .. },
                ..
            }) => (Some(outcome), error),
            _ => (
                None,
                Some(match chat.status {
                    ChatStatus::Closed => "its workspace was archived".to_string(),
                    _ => "the agent stopped before answering (its chat says why)".to_string(),
                }),
            ),
        };
        Ok(ToolDelegateResult {
            chat: chat.id.clone(),
            agent: chat.agent,
            title: chat.title.clone(),
            status: chat.status,
            text: match outcome {
                Some(_) => self.last_text(&chat.id)?,
                None => None,
            },
            outcome,
            error,
        })
    }

    /// The sequence number of the chat's last event of a type; 0 for none.
    fn last_seq(&self, chat_id: &str, kind: &str) -> Result<i64> {
        Ok(self
            .d()
            .store
            .last_events_of_type(chat_id, kind, 1)?
            .first()
            .and_then(|envelope| envelope.seq)
            .unwrap_or(0))
    }

    /// Whether a chat's agent is done with what it was given: no turn starting or running, and
    /// no message handed to its session that a turn has not taken up yet (the agent reports the
    /// turn a moment after the message). A turn waiting on the user counts as running only
    /// `through_input`.
    pub fn chat_settled(&self, chat: &ChatInfo, through_input: bool) -> Result<bool> {
        match chat.status {
            ChatStatus::Working | ChatStatus::Starting => return Ok(false),
            ChatStatus::NeedsInput => return Ok(!through_input),
            ChatStatus::Error | ChatStatus::Closed => return Ok(true),
            ChatStatus::Idle => {}
        }
        // No session (it ended, or never started): nothing will take a message up.
        if !self.is_live(&chat.id) {
            return Ok(true);
        }
        Ok(self.last_seq(&chat.id, "user_message")? <= self.last_seq(&chat.id, "turn_ended")?)
    }

    /// Waits until a chat is settled (`chat_settled`), or the timeout; true when it timed out.
    pub async fn wait_settled(
        &self,
        chat_id: &str,
        through_input: bool,
        timeout: Option<std::time::Duration>,
    ) -> Result<(ChatInfo, bool)> {
        let d = self.d();
        // Subscribed before the first look, so that no event in between is missed.
        let mut rx = d.events.subscribe();
        let deadline = timeout.map(|t| tokio::time::Instant::now() + t);
        loop {
            let chat = self.current(chat_id)?;
            if self.chat_settled(&chat, through_input)? {
                return Ok((chat, false));
            }
            let next = async {
                loop {
                    match rx.recv().await {
                        Ok(envelope)
                            if envelope.event.chat_id() == Some(chat_id)
                                && !crate::events::is_ephemeral(&envelope.event) =>
                        {
                            break;
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => break,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            std::future::pending::<()>().await
                        }
                    }
                }
            };
            let recheck = tokio::time::sleep(WAIT_RECHECK);
            match deadline {
                Some(deadline) => tokio::select! {
                    _ = next => {}
                    _ = recheck => {}
                    _ = tokio::time::sleep_until(deadline) => {
                        return Ok((self.current(chat_id)?, true));
                    }
                },
                None => tokio::select! {
                    _ = next => {}
                    _ = recheck => {}
                },
            }
        }
    }

    /// A delegated chat's notice for its agent, naming its parent; None for any other chat.
    fn subagent_notice(&self, chat: &ChatInfo) -> Option<String> {
        let parent_id = chat.parent_chat.as_deref()?;
        let parent = self.d().store.chat(parent_id).ok().flatten();
        Some(subagent_notice(
            parent.as_ref().map(|p| p.agent),
            parent.as_ref().and_then(|p| p.title.as_deref()),
        ))
    }

    /// A short title for a piece of work described by `text`, from Haiku with a Codex fallback; None
    /// when neither provider can answer in time.
    pub async fn title_for(&self, text: &str) -> Option<String> {
        let text: String = text.chars().take(2_000).collect();
        let call = self
            .sidecar
            .call(SidecarCall::AgentTitle(AgentTitleParams { text }));
        match tokio::time::timeout(TITLE_TIMEOUT, call).await {
            Ok(Ok(value)) => serde_json::from_value::<AgentTitleResult>(value)
                .ok()
                .map(|result| result.title.trim().to_string())
                .filter(|title| !title.is_empty()),
            Ok(Err(error)) => {
                tracing::warn!(%error, "no title for the workspace");
                None
            }
            Err(_) => {
                tracing::warn!("no title for the workspace: neither provider answered in time");
                None
            }
        }
    }

    /// A workspace named at random takes its title from its first message, in the background,
    /// unless its tickets gave it one.
    fn name_workspace_from(&self, workspace_id: &str, text: &str) {
        let d = self.d();
        if text.trim().is_empty() || !d.store.take_auto_title(workspace_id).unwrap_or(false) {
            return;
        }
        let titled = d
            .store
            .find_workspace(workspace_id)
            .ok()
            .flatten()
            .is_some_and(|w| w.title.is_some());
        if titled {
            return;
        }
        let (workspace_id, text) = (workspace_id.to_string(), text.to_string());
        tokio::spawn(async move {
            let Some(title) = d.chats.title_for(&text).await else {
                return;
            };
            if d.store
                .set_message_title(&workspace_id, &title)
                .unwrap_or(false)
            {
                d.workspaces.publish(&workspace_id);
            }
        });
    }

    /// After a turn, a workspace whose title Chauffeur chose may be named again from its
    /// conversation, in the background; the model keeps the title while it still fits.
    fn name_workspace_again(&self, workspace_id: &str) {
        let d = self.d();
        match d
            .store
            .take_conversation_title(workspace_id, RETITLE_FIRST, RETITLE_EVERY)
        {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => {
                tracing::warn!(%error, workspace = workspace_id, "could not count the workspace's turns");
                return;
            }
        }
        let workspace_id = workspace_id.to_string();
        tokio::spawn(async move {
            let Ok(Some(workspace)) = d.store.find_workspace(&workspace_id) else {
                return;
            };
            let conversation = match d.chats.workspace_conversation(&workspace_id) {
                Ok(conversation) if !conversation.trim().is_empty() => conversation,
                Ok(_) => return,
                Err(error) => {
                    tracing::warn!(%error, workspace = workspace_id, "could not read the workspace's conversation");
                    return;
                }
            };
            let title = match d.chats.retitle_call(conversation, workspace.title).await {
                Ok(title) => title,
                Err(error) => {
                    tracing::warn!(%error, workspace = workspace_id, "no new title for the workspace");
                    return;
                }
            };
            if !title.is_empty()
                && d.store
                    .set_conversation_title(&workspace_id, &title)
                    .unwrap_or(false)
            {
                d.workspaces.publish(&workspace_id);
            }
        });
    }

    /// Names a workspace again from its conversation (optionally its last N messages): whatever
    /// named it so far, and without `KEEP` (no current title is given to the model). The new
    /// name is the user's: nothing renames the workspace automatically afterwards.
    pub async fn regenerate_title(
        &self,
        key: &str,
        last_messages: Option<usize>,
    ) -> Result<WorkspaceInfo> {
        let d = self.d();
        let workspace = d.workspaces.row(key)?;
        if workspace.state != WorkspaceState::Ready {
            return Err(Error::Conflict(format!(
                "{} is {}: it cannot be named again",
                workspace.slug,
                workspace.state.as_str()
            )));
        }
        let conversation = match last_messages {
            Some(0) => {
                return Err(Error::BadRequest(
                    "Choose at least one recent message".into(),
                ));
            }
            Some(count) => self.recent_workspace_conversation(&workspace.id, count)?,
            None => self.workspace_conversation(&workspace.id)?,
        };
        if conversation.trim().is_empty() {
            return Err(Error::BadRequest(format!(
                "{} has no conversation to name yet: send a message first",
                workspace.slug
            )));
        }
        let title = self.retitle_call(conversation, None).await?;
        d.workspaces.rename(&workspace.id, &title)
    }

    /// Claude's name for a workspace from its conversation: `current` when the model keeps it,
    /// else a new one; with no `current`, always a name.
    async fn retitle_call(&self, conversation: String, current: Option<String>) -> Result<String> {
        let call = self
            .sidecar
            .call(SidecarCall::AgentRetitle(AgentRetitleParams {
                conversation,
                current,
            }));
        let value = tokio::time::timeout(TITLE_TIMEOUT, call)
            .await
            .map_err(|_| {
                Error::Unavailable("Claude did not name the workspace in time".into())
            })??;
        let result: AgentTitleResult = serde_json::from_value(value)?;
        Ok(result.title.trim().to_string())
    }

    /// The last N nonempty developer/assistant messages, ordered across the workspace's chats.
    fn recent_workspace_conversation(&self, workspace_id: &str, count: usize) -> Result<String> {
        let d = self.d();
        let mut messages = Vec::new();
        for chat in d.store.chats_of(workspace_id)? {
            for envelope in d.store.events_of_chat(&chat.id)? {
                let Event::Chat { event, .. } = envelope.event else {
                    continue;
                };
                let text = match event {
                    ChatEvent::UserMessage { text, .. } if !text.trim().is_empty() => {
                        format!("[Developer]\n{}", text.trim())
                    }
                    ChatEvent::AssistantText {
                        text,
                        parent_tool_use_id: None,
                        ..
                    } if !text.trim().is_empty() => format!("[Agent]\n{}", text.trim()),
                    _ => continue,
                };
                messages.push((envelope.seq, text));
            }
        }
        messages.sort_by_key(|(seq, _)| *seq);
        let start = messages.len().saturating_sub(count);
        let text = messages[start..]
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        // Bound the provider context, keeping the most recent text when messages are long.
        let skip = text.chars().count().saturating_sub(RETITLE_BUDGET);
        Ok(text.chars().skip(skip).collect())
    }

    /// The conversation of every chat of a workspace, oldest chat first, each fitted to its share
    /// of the budget (its first message and its end).
    fn workspace_conversation(&self, workspace_id: &str) -> Result<String> {
        let d = self.d();
        let chats = d.store.chats_of(workspace_id)?;
        let budget = RETITLE_BUDGET / chats.len().max(1);
        let mut parts = Vec::new();
        for chat in chats {
            let events: Vec<ChatEvent> = d
                .store
                .events_of_chat(&chat.id)?
                .into_iter()
                .filter_map(|envelope| match envelope.event {
                    Event::Chat { event, .. } => Some(event),
                    _ => None,
                })
                .collect();
            let text = design::conversation_text(&events, budget);
            if !text.trim().is_empty() {
                parts.push(text);
            }
        }
        Ok(parts.join("\n\n[Another chat of the workspace]\n\n"))
    }

    /// What `/` offers in a chat of `agent` in this workspace, as its sessions see it.
    pub async fn commands(&self, workspace: &str, agent: AgentKind) -> Result<Vec<SlashCommand>> {
        let workspace = self.d().workspaces.get(workspace)?;
        if matches!(
            workspace.state,
            WorkspaceState::Archived | WorkspaceState::Archiving
        ) {
            return Err(Error::Conflict(format!("{} is archived", workspace.slug)));
        }
        let value = self
            .sidecar
            .call(SidecarCall::AgentCommands(AgentCommandsParams {
                agent,
                cwd: workspace.path.clone(),
                additional_directories: repo_dirs(&workspace),
            }))
            .await?;
        Ok(serde_json::from_value::<AgentCommandsResult>(value)?.commands)
    }

    /// What `/` offers in a workspace of `repos` that does not exist yet: the sessions will see the
    /// skills of each repo's checkout, so the first one's clone is the folder and the others its
    /// additional directories. Repos with no local clone add nothing; with none, the home folder
    /// stands in, for the user's and the plugins' own commands.
    pub async fn commands_for_repos(
        &self,
        repos: &[String],
        agent: AgentKind,
    ) -> Result<Vec<SlashCommand>> {
        let d = self.d();
        let mut clones = repos
            .iter()
            .filter_map(|reference| d.repos.existing_clone(reference))
            .map(|path| path.to_string_lossy().into_owned());
        let cwd = clones
            .next()
            .unwrap_or_else(|| d.paths.home.to_string_lossy().into_owned());
        let value = self
            .sidecar
            .call(SidecarCall::AgentCommands(AgentCommandsParams {
                agent,
                cwd,
                additional_directories: clones.collect(),
            }))
            .await?;
        Ok(serde_json::from_value::<AgentCommandsResult>(value)?.commands)
    }

    pub async fn worktree_remove(&self, chat_id: &str, path: &str) -> Result<()> {
        let d = self.d();
        let chat = self.current(chat_id)?;
        d.workspaces
            .remove_agent_worktrees(&chat.workspace_id, path)
            .await?;
        Ok(())
    }
}

/// The workspace's own worktree of each repo. Sessions start in the workspace folder, above them,
/// and get these as additional directories: Claude Code then loads each repo's skills and
/// commands, as it would starting in the repo.
pub fn repo_dirs(workspace: &WorkspaceInfo) -> Vec<String> {
    workspace
        .worktrees
        .iter()
        .filter(|w| w.origin == WorktreeOrigin::Workspace && !w.removed)
        .map(|w| w.path.clone())
        .collect()
}

/// Whether a status change deserves the user's attention (an unread workspace): a turn ended or
/// failed, or the agent asks for input. A session that merely started is not news.
/// A reading of the log, with the compaction that applies now: Jev turned on or off since, the
/// session (closed) resumes with it at its next message. Without Jev, Claude Code compacts where
/// its buffer starts, and Codex where it does not say.
pub fn stored_reading(
    mut usage: ContextUsage,
    agent: AgentKind,
    jev_threshold: Option<u8>,
) -> ContextUsage {
    if let Some(threshold) = jev_threshold {
        usage.compaction = Some(ContextCompaction {
            by: CompactionBy::Jev,
            threshold_percent: f64::from(threshold),
        });
    } else if agent == AgentKind::Codex {
        usage.compaction = None;
    } else if usage
        .compaction
        .as_ref()
        .is_some_and(|c| c.by == CompactionBy::Jev)
    {
        let buffer: u64 = usage
            .categories
            .iter()
            .filter(|c| c.kind == ContextCategoryKind::Buffer)
            .map(|c| c.tokens)
            .sum();
        usage.compaction = (buffer > 0 && usage.max_tokens > buffer).then(|| ContextCompaction {
            by: CompactionBy::ClaudeCode,
            threshold_percent: (usage.max_tokens - buffer) as f64 * 100.0 / usage.max_tokens as f64,
        });
    }
    usage
}

pub fn needs_attention(before: ChatStatus, after: ChatStatus) -> bool {
    use ChatStatus::*;
    match after {
        NeedsInput => before != NeedsInput,
        Idle => matches!(before, Working | NeedsInput),
        Error => matches!(before, Working | Starting | NeedsInput),
        _ => false,
    }
}

/// What a chat started without agent or model gets: `default_agent` and `default_model` from
/// settings.toml when set, else the first favorite of the app's model picker, else Claude.
fn default_choice(d: &Daemon) -> Result<LoadoutEntry> {
    let settings = d.settings.get();
    if settings.default_agent.is_some() || settings.default_model.is_some() {
        return Ok(LoadoutEntry {
            agent: settings
                .default_agent
                .as_deref()
                .and_then(AgentKind::parse)
                .unwrap_or_default(),
            model: settings.default_model.clone().unwrap_or_default(),
            effort: None,
            fast: false,
        });
    }
    Ok(crate::models::loadout(d)?
        .into_iter()
        .next()
        .unwrap_or_else(|| LoadoutEntry {
            agent: AgentKind::default(),
            model: String::new(),
            effort: None,
            fast: false,
        }))
}

/// What a chat of `agent` gets when the user picked no model (a design chat, a fork into the other
/// agent): the user's default model when it is that agent's, else its first favorite of the model
/// picker, else the agent's own default.
fn agent_choice(d: &Daemon, agent: AgentKind) -> Result<LoadoutEntry> {
    let default = default_choice(d)?;
    if default.agent == agent {
        return Ok(default);
    }
    Ok(crate::models::loadout(d)?
        .into_iter()
        .find(|entry| entry.agent == agent)
        .unwrap_or_else(|| LoadoutEntry {
            agent,
            model: String::new(),
            effort: None,
            fast: false,
        }))
}

/// What a new delegated chat runs on (story 49), and what Chauffeur chose otherwise than asked.
struct DelegateChoice {
    agent: AgentKind,
    model: Option<String>,
    effort: Option<String>,
    fast: bool,
    notice: Option<String>,
}

/// The parent's agent unless the call names one, or names a model of the other, or asks for a
/// review (the other agent); an agent signed out or out of its plan's limits hands the task to
/// the other. The model is the one named, else the tier's (`default`: the parent's own on the
/// parent's agent, else the user's default for the agent).
async fn delegate_choice(
    d: &Arc<Daemon>,
    parent: &ChatInfo,
    params: &ToolDelegateParams,
) -> Result<DelegateChoice> {
    let named = params
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty());
    let effort = params
        .effort
        .as_deref()
        .map(str::trim)
        .filter(|effort| !effort.is_empty());
    // Listing a catalog may start an agent: only to check a model or an effort the call names.
    let catalogs = if named.is_some() || effort.is_some() {
        d.models.list(d, false).await
    } else {
        Vec::new()
    };
    let listed = |agent: AgentKind| {
        catalogs
            .iter()
            .find(|catalog| catalog.agent == agent && !catalog.models.is_empty())
            .map(|catalog| &catalog.models)
    };
    let owner = named.and_then(|model| model_owner(&catalogs, model));
    let asked = params
        .agent
        .or(owner)
        .unwrap_or(if params.review == Some(true) {
            other_agent(parent.agent)
        } else {
            parent.agent
        });
    let mut agent = asked;
    let mut notice = None;
    if let Some(why) = d.usage.spent(asked)
        && d.usage.spent(other_agent(asked)).is_none()
    {
        agent = other_agent(asked);
        notice = Some(format!(
            "{} {why}: the task went to {} instead.",
            agent_name(asked),
            agent_name(agent)
        ));
    }
    let (model, inherited, fast) = match named.filter(|_| agent == asked) {
        Some(model) => {
            if let Some(models) = listed(agent)
                && !models.iter().any(|known| known.id == model)
            {
                let ids: Vec<&str> = models.iter().map(|known| known.id.as_str()).collect();
                return Err(Error::BadRequest(format!(
                    "{} has no model {model}: pick a tier instead, or one of {}",
                    agent_name(agent),
                    ids.join(", ")
                )));
            }
            (Some(model.to_string()), None, false)
        }
        None => tier_choice(d, parent, agent, params.tier.unwrap_or_default())?,
    };
    // An effort the model does not take is left out: the agent would refuse it.
    let accepts = |effort: &str| {
        listed(agent)
            .and_then(|models| match &model {
                Some(id) => models.iter().find(|known| &known.id == id),
                None => models.iter().find(|known| known.is_default),
            })
            .is_none_or(|known| known.efforts.iter().any(|level| level == effort))
    };
    if let Some(effort) = effort
        && !accepts(effort)
    {
        let model = model.as_deref().unwrap_or("its default model");
        return Err(Error::BadRequest(format!(
            "{} {model} takes no effort {effort}: leave effort out, or pick one it accepts",
            agent_name(agent)
        )));
    }
    Ok(DelegateChoice {
        agent,
        model,
        effort: effort.map(str::to_string).or(inherited),
        fast,
        notice,
    })
}

/// A tier's model of `agent`, with the effort and speed it comes with.
fn tier_choice(
    d: &Daemon,
    parent: &ChatInfo,
    agent: AgentKind,
    tier: DelegateTier,
) -> Result<(Option<String>, Option<String>, bool)> {
    if let Some(model) = crate::models::tier_model(agent, tier) {
        return Ok((Some(model.to_string()), None, false));
    }
    let local = |model: &Option<String>| model.as_deref().is_some_and(crate::ollama::is_local);
    if agent == parent.agent && !local(&parent.model) {
        return Ok((parent.model.clone(), parent.effort.clone(), parent.fast));
    }
    // A local model is too slow for someone else's task: the agent's own default then.
    let choice = agent_choice(d, agent)?;
    let model = Some(choice.model).filter(|model| !model.is_empty());
    if local(&model) {
        return Ok((None, None, false));
    }
    Ok((model, choice.effort, choice.fast))
}

/// The agent whose catalog lists `model`; a local model is Codex's.
fn model_owner(catalogs: &[AgentModels], model: &str) -> Option<AgentKind> {
    if crate::ollama::is_local(model) {
        return Some(AgentKind::Codex);
    }
    catalogs
        .iter()
        .find(|catalog| catalog.models.iter().any(|known| known.id == model))
        .map(|catalog| catalog.agent)
}

fn other_agent(agent: AgentKind) -> AgentKind {
    match agent {
        AgentKind::Claude => AgentKind::Codex,
        AgentKind::Codex => AgentKind::Claude,
    }
}

/// A design chat's tab, named after its source chat.
fn design_title(source_title: Option<&str>) -> String {
    match source_title
        .map(str::trim)
        .filter(|title| !title.is_empty())
    {
        Some(title) => title_of(&format!("Design: {title}")),
        None => "Design".to_string(),
    }
}

/// What a copied conversation leaves open: the tasks it started and never ended, in their order,
/// and whether messages were still queued at its end.
fn left_open(copied: &[EventEnvelope]) -> (Vec<String>, bool) {
    let mut tasks: Vec<String> = Vec::new();
    let mut queued = false;
    for envelope in copied {
        let Event::Chat { event, .. } = &envelope.event else {
            continue;
        };
        match event {
            ChatEvent::TaskStarted { task_id, .. } if !tasks.contains(task_id) => {
                tasks.push(task_id.clone())
            }
            ChatEvent::TaskEnded { task_id, .. } => tasks.retain(|t| t != task_id),
            ChatEvent::QueueChanged { messages } => queued = !messages.is_empty(),
            _ => {}
        }
    }
    (tasks, queued)
}

/// A fork's title: its source's, marked in front where a narrow tab still shows it (a fork of a
/// fork once).
fn fork_title(source_title: Option<&str>) -> String {
    match source_title
        .map(str::trim)
        .filter(|title| !title.is_empty())
    {
        Some(title) if title.starts_with("Fork: ") => title.to_string(),
        Some(title) => title_of(&format!("Fork: {title}")),
        None => "Fork".to_string(),
    }
}

fn title_of(prompt: &str) -> String {
    let line = prompt
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    let mut title: String = line.chars().take(60).collect();
    if line.chars().count() > 60 {
        title.push('…');
    }
    title
}

/// The folder of the workspace's notes where Claude Code writes its plans, created if missing:
/// they show in the Files tab and are kept on archive with the notes. In an exploration it is
/// the one place the agent may write (the sidecar's plan guard). None when it cannot be made: the
/// plans then go to Claude Code's own folder, and an exploration writes nothing.
fn plans_dir(workspace: &WorkspaceRow) -> Option<String> {
    let notes = std::path::Path::new(&workspace.path).join(crate::paths::NOTES_DIR);
    let dir = notes.join(crate::paths::PLANS_DIR);
    // Never through a symlink: the folder would be made wherever it points.
    let made = std::fs::create_dir_all(&notes).and_then(|()| {
        for path in [&notes, &dir] {
            match std::fs::symlink_metadata(path) {
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => {
                    return Err(std::io::Error::other(format!(
                        "{} is not a folder",
                        path.display()
                    )));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    std::fs::create_dir(path)?
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    });
    match made {
        Ok(()) => Some(dir.to_string_lossy().into_owned()),
        Err(error) => {
            tracing::warn!(%error, dir = %dir.display(), "plans folder could not be created");
            None
        }
    }
}

/// What every session of the workspace is told: that a design chat designs, or that an
/// exploration is read-only; that a delegated chat answers its parent; the tickets and reply format.
fn system_prompt(
    workspace: &WorkspaceRow,
    chat: &ChatInfo,
    design: Option<(&str, AgentKind)>,
    subagent: Option<&str>,
    plan: Option<&str>,
) -> Option<String> {
    let notice = match design {
        Some((dir, agent)) => Some(design::system_prompt(dir, agent)),
        None if workspace.explore => Some(format!(
            "{EXPLORE_NOTICE}\n\nFor an interface mockup, direct the user to this chat tab's “Design from this chat” action: this exploration's agent cannot use the Chauffeur CLI."
        )),
        None => Some(format!(
            "When the user asks to see or explore an interface design or mockup, create and start a Chauffeur design chat in this workspace without another confirmation or asking them to repeat the request. \
             Use `chauffeur --json chat {workspace} --design --from {source} --agent {agent}` to create it with a brief from this conversation. \
             Create it once and keep the returned chat id; reuse that chat for follow-ups instead of creating another. \
             This only creates the tab and returns its brief; it does not start the designer. \
             Read the returned brief, then use `chauffeur send <returned chat id> '<design request>'` to send a self-contained request with the relevant context, repo paths and references. \
             Ask the designer to read DESIGN.md and the existing screens, preserve the product's appearance, put the screens in canvas.json and check their renders with headless-browser. \
             Tell the user the design chat's name so they can see its canvas. Do not claim its screens are ready before checking. \
             Keep ordinary explanations and Mermaid diagrams in this chat; a request to implement a page or export a file still follows the user's requested deliverable.",
            workspace = workspace.id,
            source = chat.id,
            agent = chat.agent.as_str(),
        )),
    };
    let parts: Vec<String> = [
        Some("When mentioning a pull request in a reply, including in tables and summaries, use a clickable Markdown link to its actual URL, such as [#351](https://github.com/owner/repo/pull/351), rather than a bare PR number. Resolve the URL from the correct repository or `gh pr view <number> --repo <owner/repo> --json url`; never guess the repository when it is ambiguous.".to_string()),
        Some("You are replying in Chauffeur. It does not render the `visualize` skill's inline content references: never emit a visualize marker or rely on the Codex inline visualization runtime. Interface mockups belong in a Chauffeur design chat's canvas, as self-contained HTML screens, not HTML fragments written into the notes and referenced in a reply.".to_string()),
        notice,
        subagent.map(str::to_string),
        plan.map(str::to_string),
        ticket_briefing(&workspace.path),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

pub(crate) fn agent_name(agent: AgentKind) -> &'static str {
    match agent {
        AgentKind::Claude => "Claude Code",
        AgentKind::Codex => "Codex",
    }
}

/// What a fork into the other agent hands its agent before the user's message, until that agent
/// has a session: the conversation it was forked with, as text. None when there was nothing yet.
fn handoff(from: AgentKind, to: AgentKind, events: &[ChatEvent]) -> Option<String> {
    let conversation = design::conversation_text(events, design::CONVERSATION_BUDGET);
    if conversation.trim().is_empty() {
        return None;
    }
    Some(format!(
        "This chat continues in Chauffeur from a conversation the developer held with {from}; you, {to}, carry it on. You have no record of it but this transcript: the developer's messages, the agent's replies, and one line per tool call, never its output nor a file's content. Read the files again rather than trusting it about them.\n\n<conversation>\n{conversation}\n</conversation>\n\nThe developer's new message:\n\n",
        from = agent_name(from),
        to = agent_name(to),
    ))
}

/// A delegated chat's answer as its parent's agent reads it, once its turn had ended before:
/// as the `delegate` tool's result would have read (the sidecar's `answer`), saying why it comes
/// as a message.
fn late_answer(result: &ToolDelegateResult) -> String {
    let name = match result.agent {
        AgentKind::Codex => "Codex",
        AgentKind::Claude => "Claude",
    };
    let titled = result
        .title
        .as_deref()
        .map(|title| format!(", \"{title}\""))
        .unwrap_or_default();
    let who = format!("{name} (delegated chat {}{titled})", result.chat);
    let message = result
        .text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    let head = "Chauffeur: your turn ended before this delegated chat answered your delegate call, so its answer comes as this message.";
    let body = match result.outcome {
        Some(TurnOutcome::Success) => {
            format!("{who} answered:\n\n{}", message.unwrap_or("(no message)"))
        }
        Some(TurnOutcome::Interrupted) => match message {
            Some(text) => {
                format!("{who} was interrupted before it finished; its last message:\n\n{text}")
            }
            None => format!("{who} was interrupted before it finished."),
        },
        Some(TurnOutcome::Error) => {
            let error = result
                .error
                .as_deref()
                .map(|error| format!(": {error}"))
                .unwrap_or_default();
            match message {
                Some(text) => {
                    format!("{who} stopped on an error{error}\n\nIts last message:\n\n{text}")
                }
                None => format!("{who} stopped on an error{error}"),
            }
        }
        None => format!(
            "{who} could not work on the task: {}.",
            result.error.as_deref().unwrap_or("its agent did not start")
        ),
    };
    format!(
        "{head}\n\n{body}\n\nGive it more work with delegate and chat \"{}\".",
        result.chat
    )
}

/// What a delegated chat's agent is told: who gave it its task, and that its last message of each
/// turn is the answer that agent gets.
fn subagent_notice(parent_agent: Option<AgentKind>, parent_title: Option<&str>) -> String {
    let agent = match parent_agent {
        Some(AgentKind::Claude) => "a Claude Code chat",
        Some(AgentKind::Codex) => "a Codex chat",
        None => "a chat",
    };
    let titled = parent_title
        .map(|title| format!(" (\"{title}\")"))
        .unwrap_or_default();
    format!(
        "This chat is a delegated chat: another agent of this Chauffeur workspace, {agent}{titled}, started it with its delegate tool and gave it its task. You work in the same folders as that agent. The user sees this chat, and may answer your questions or write to you here. At the end of each turn, your last message goes back to that agent as the answer to its task: make it complete and self-contained (what you did, what you found, what is left to do), never a pointer to earlier messages."
    )
}

/// Claude's plan, counted into the chat's progress (plan_progress.rs), which the user follows in
/// the Office.
const PLAN_NOTICE: &str = "When a task takes several steps (three or more), keep your plan with the update_plan tool: call it with every step and its status once you have planned, then again each time a step starts or is done. The user follows your progress from it. Skip it for a quick answer or a single change.";

const EXPLORE_NOTICE: &str = "This Chauffeur workspace is an exploration: each repo is a detached checkout of its base branch (e.g. origin/staging), as just fetched. Read, search, run read-only commands and explain; do not edit files, commit, push, open pull requests or create worktrees, which are refused here. The one file you may write is your plan, in the workspace's .chauffeur/plans/ folder. When the user wants code changed, tell them to create a Chauffeur workspace for it.";

/// The tickets' briefing (see `WorkspaceManager::set_tickets`), given to every session of the
/// workspace.
fn ticket_briefing(workspace_path: &str) -> Option<String> {
    let path = std::path::Path::new(workspace_path)
        .join(crate::paths::NOTES_DIR)
        .join("tickets.md");
    let text = std::fs::read_to_string(path).ok()?;
    let mut text: String = text.chars().take(30_000).collect();
    text.insert_str(
        0,
        "This Chauffeur workspace serves the Linear tickets below (also in .chauffeur/tickets.md).\n\n",
    );
    Some(text)
}

/// What the agent's `list_repos` tool returns: the organization's repos, then the workspace's
/// own that the organization does not list, each with its folder when already in the workspace.
fn repos_for_agent(
    workspace: &WorkspaceInfo,
    listed: Vec<chauffeur_protocol::model::RepoInfo>,
) -> ToolListReposResult {
    let path_of = |name: &str| {
        workspace
            .worktrees
            .iter()
            .find(|w| !w.removed && w.origin == WorktreeOrigin::Workspace && w.repo == name)
            .map(|w| w.path.clone())
    };
    let mut repos: Vec<ToolRepo> = listed
        .into_iter()
        .map(|repo| ToolRepo {
            path: path_of(&repo.name),
            // `owner/name` outside the main organization: what add_repo takes back.
            name: repo.key,
            description: repo.description.filter(|d| !d.trim().is_empty()),
        })
        .collect();
    for worktree in &workspace.worktrees {
        if !worktree.removed
            && worktree.origin == WorktreeOrigin::Workspace
            && !repos
                .iter()
                .any(|r| crate::repos::repo_folder(&r.name) == worktree.repo)
        {
            repos.push(ToolRepo {
                name: worktree.repo.clone(),
                description: None,
                path: Some(worktree.path.clone()),
            });
        }
    }
    ToolListReposResult {
        workspace: workspace.slug.clone(),
        explore: workspace.explore,
        state: workspace.state,
        repos,
    }
}

/// The requests of a chat's last turn no `request_resolved` event closed, oldest first.
fn open_requests(store: &crate::store::Store, chat_id: &str) -> Vec<String> {
    let read = || -> Result<Vec<String>> {
        let turn_end = store
            .last_events_of_type(chat_id, "turn_ended", 1)?
            .first()
            .and_then(|envelope| envelope.seq)
            .unwrap_or(0);
        let mut open: Vec<String> = Vec::new();
        for kind in ["permission_request", "mcp_elicitation", "question"] {
            for envelope in store.events_of_type_after(chat_id, kind, turn_end)? {
                if let Event::Chat {
                    event:
                        ChatEvent::PermissionRequest { request_id, .. }
                        | ChatEvent::McpElicitation { request_id, .. }
                        | ChatEvent::Question { request_id, .. },
                    ..
                } = envelope.event
                {
                    open.push(request_id);
                }
            }
        }
        for envelope in store.events_of_type_after(chat_id, "request_resolved", turn_end)? {
            if let Event::Chat {
                event: ChatEvent::RequestResolved { request_id, .. },
                ..
            } = envelope.event
            {
                open.retain(|id| *id != request_id);
            }
        }
        Ok(open)
    };
    read().unwrap_or_else(|error| {
        tracing::warn!("could not read chat {chat_id}'s open requests: {error:#}");
        Vec::new()
    })
}

/// The rules the user allowed always in a chat, from its `request_resolved` events: a new agent
/// session (an idle one closed, a restart) keeps not asking for them.
fn always_allowed(store: &crate::store::Store, chat_id: &str) -> Vec<String> {
    let events = match store.last_events_of_type(chat_id, "request_resolved", 10_000) {
        Ok(events) => events,
        Err(error) => {
            tracing::warn!("could not read chat {chat_id}'s allowed rules: {error:#}");
            return Vec::new();
        }
    };
    let mut rules: Vec<String> = Vec::new();
    for envelope in events.into_iter().rev() {
        if let Event::Chat {
            event: ChatEvent::RequestResolved {
                rules: Some(found), ..
            },
            ..
        } = envelope.event
        {
            for rule in found {
                if !rules.contains(&rule) {
                    rules.push(rule);
                }
            }
        }
    }
    rules
}

/// Whether a turn's error is one that trying again could cure: the API unreachable (no network,
/// DNS), overloaded or failing on its side. Never a login, a quota or a budget.
fn is_retryable(error: &str) -> bool {
    let error = error.to_lowercase();
    const CAUSES: [&str; 8] = [
        "can't reach the api server",
        "enotfound",
        "econnrefused",
        "econnreset",
        "etimedout",
        "enetunreach",
        "eai_again",
        "overloaded",
    ];
    CAUSES.iter().any(|cause| error.contains(cause)) || error.starts_with("api error: 5")
}

/// What the agent reads when Chauffeur resumes a turn that failed on a retryable error, `failed_at`
/// being when the turn ended (RFC 3339).
fn retry_prompt(error: &str, failed_at: &str) -> String {
    let ago = chrono::DateTime::parse_from_rfc3339(failed_at)
        .ok()
        .map(|at| chrono::Utc::now().signed_duration_since(at))
        .filter(|elapsed| elapsed.num_minutes() > 0)
        .map(|elapsed| format!(" {} minutes ago", elapsed.num_minutes()))
        .unwrap_or_default();
    format!(
        "Your last turn failed{ago} on an error that trying again could cure ({error}), and the \
user is back. Before running anything again, check the state your work was left in (a command \
may have run halfway), then carry on with what you were doing."
    )
}

/// A workspace's chats in the order of their tabs: by `tab_position`, those without one after,
/// by creation (as `chats` comes from the store).
fn tab_order(chats: &[ChatInfo]) -> Vec<String> {
    let mut sorted: Vec<&ChatInfo> = chats.iter().collect();
    sorted.sort_by_key(|c| c.tab_position.unwrap_or(u32::MAX));
    sorted.into_iter().map(|c| c.id.clone()).collect()
}

/// `ids` with `id` moved before `before`, or last when `before` is absent or `id` itself.
pub(crate) fn moved_before(mut ids: Vec<String>, id: &str, before: Option<&str>) -> Vec<String> {
    ids.retain(|other| other != id);
    let at = before
        .and_then(|b| ids.iter().position(|other| other == b))
        .unwrap_or(ids.len());
    ids.insert(at, id.to_string());
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use chauffeur_protocol::events::ContextCategory;

    #[test]
    fn only_errors_that_trying_again_could_cure_are_retried() {
        for error in [
            "API Error: Can't reach the API server \u{2014} check your internet or DNS (ENOTFOUND)",
            "API Error: 529 Overloaded",
            "API Error: 503 Service Unavailable",
            "connect ECONNREFUSED 160.79.104.10:443",
        ] {
            assert!(is_retryable(error), "{error}");
        }
        for error in [
            "API Error: 401 invalid x-api-key",
            "Claude reached the maximum budget",
            "Claude reached the maximum number of turns",
            "Credit balance is too low",
        ] {
            assert!(!is_retryable(error), "{error}");
        }
    }

    #[test]
    fn the_retry_prompt_says_how_long_ago_the_turn_failed() {
        let two_hours_ago = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        assert!(retry_prompt("ENOTFOUND", &two_hours_ago).contains("failed 120 minutes ago"));
        assert!(retry_prompt("ENOTFOUND", "not a date").contains("failed on an error"));
    }

    #[test]
    fn tabs_go_by_their_place_then_new_chats_last_by_creation() {
        let chat = |id: &str, tab_position: Option<u32>| -> ChatInfo {
            let mut c: ChatInfo = serde_json::from_value(serde_json::json!({
                "id": id, "workspace_id": "w", "agent": "claude", "model": null, "effort": null,
                "permission_mode": "auto", "title": null, "status": "idle",
                "agent_session_id": null, "created_at": "2026-10-25T00:00:00Z",
            }))
            .expect("a chat");
            c.tab_position = tab_position;
            c
        };
        let chats = [
            chat("a", Some(2)),
            chat("b", None),
            chat("c", Some(0)),
            chat("d", None),
            chat("e", Some(1)),
        ];
        assert_eq!(tab_order(&chats), ["c", "e", "a", "b", "d"]);
    }

    #[test]
    fn a_tab_moves_before_another_or_last() {
        let ids = || ["a", "b", "c", "d"].map(String::from).to_vec();
        assert_eq!(moved_before(ids(), "d", Some("b")), ["a", "d", "b", "c"]);
        assert_eq!(moved_before(ids(), "a", Some("d")), ["b", "c", "a", "d"]);
        assert_eq!(moved_before(ids(), "b", None), ["a", "c", "d", "b"]);
        // Before itself or an unknown tab: last.
        assert_eq!(moved_before(ids(), "b", Some("b")), ["a", "c", "d", "b"]);
        assert_eq!(moved_before(ids(), "b", Some("z")), ["a", "c", "d", "b"]);
    }

    #[test]
    fn a_new_session_keeps_the_rules_allowed_always_in_the_chat() {
        let store = crate::store::Store::open_in_memory().unwrap();
        let bus = crate::events::EventBus::new(store.clone());
        let resolved = |id: &str, rules: Option<Vec<&str>>| ChatEvent::RequestResolved {
            request_id: id.into(),
            outcome: "allowed".into(),
            rules: rules.map(|r| r.into_iter().map(String::from).collect()),
        };
        bus.chat(
            "w",
            "c",
            resolved("r1", Some(vec!["mcp__linear__save_issue"])),
        );
        bus.chat("w", "c", resolved("r2", None));
        bus.chat(
            "w",
            "c",
            resolved("r3", Some(vec!["mcp__linear__save_issue"])),
        );
        bus.chat("w", "c", resolved("r4", Some(vec!["Bash(npm test:*)"])));
        bus.chat("w", "other", resolved("r5", Some(vec!["Write"])));
        assert_eq!(
            always_allowed(&store, "c"),
            vec![
                "mcp__linear__save_issue".to_string(),
                "Bash(npm test:*)".to_string()
            ]
        );
        assert!(always_allowed(&store, "none").is_empty());
    }

    fn reading(compaction: Option<ContextCompaction>) -> ContextUsage {
        ContextUsage {
            model: None,
            used_tokens: 100_000,
            max_tokens: 200_000,
            percent: 50.0,
            over_limit: false,
            categories: vec![ContextCategory {
                name: "Autocompact buffer".into(),
                tokens: 33_000,
                kind: ContextCategoryKind::Buffer,
            }],
            compaction,
            estimated: false,
            details: None,
        }
    }

    #[test]
    fn a_stored_reading_marks_the_compaction_that_applies_now() {
        let jev = |threshold| {
            Some(ContextCompaction {
                by: CompactionBy::Jev,
                threshold_percent: threshold,
            })
        };
        // Jev turned on since: its threshold.
        assert_eq!(
            stored_reading(reading(None), AgentKind::Claude, Some(55)).compaction,
            jev(55.0)
        );
        // Jev turned off since: where Claude Code's buffer starts.
        let off = stored_reading(reading(jev(60.0)), AgentKind::Claude, None).compaction;
        assert_eq!(off.as_ref().map(|c| c.by), Some(CompactionBy::ClaudeCode));
        assert!((off.unwrap().threshold_percent - 83.5).abs() < 1e-9);
        // Unchanged otherwise.
        let auto = Some(ContextCompaction {
            by: CompactionBy::ClaudeCode,
            threshold_percent: 96.7,
        });
        assert_eq!(
            stored_reading(reading(auto.clone()), AgentKind::Claude, None).compaction,
            auto
        );
        // A Codex chat compacts at Jev's threshold, or where Codex does not say.
        assert_eq!(
            stored_reading(reading(None), AgentKind::Codex, Some(60)).compaction,
            jev(60.0)
        );
        assert_eq!(
            stored_reading(reading(jev(60.0)), AgentKind::Codex, None).compaction,
            None
        );
    }
}
