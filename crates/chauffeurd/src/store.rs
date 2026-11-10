//! SQLite persistence. One connection behind a mutex for writes and short reads; a second,
//! read-only one for the reads that go through a chat's or a workspace's whole history (replays,
//! searches), so they never hold up the writes and the chats' streams meanwhile (WAL lets a reader
//! run beside the writer).

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use chauffeur_protocol::client::{QueueItem, Replay};
use chauffeur_protocol::events::{ChatEvent, Event, EventEnvelope, ForkPoint};
use chauffeur_protocol::model::{
    AgentKind, ChatInfo, ChatKind, ChatStatus, DesignBrief, PermissionMode, QueuedMessage,
    SectionInfo, TicketRef, TurnOutcome, WorkspaceState, WorktreeOrigin,
};
use chauffeur_protocol::workspace_share::{ShareComment, Suggestion, SuggestionState};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::Result;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS workspaces (
    id TEXT PRIMARY KEY,
    slug TEXT NOT NULL UNIQUE,
    path TEXT NOT NULL,
    branch TEXT NOT NULL,
    parent_id TEXT,
    state TEXT NOT NULL,
    tickets TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL,
    archived_at TEXT,
    last_error TEXT
);
CREATE TABLE IF NOT EXISTS worktrees (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    repo TEXT NOT NULL,
    clone_path TEXT NOT NULL,
    path TEXT NOT NULL,
    branch TEXT NOT NULL,
    base_branch TEXT NOT NULL,
    base_commit TEXT,
    origin TEXT NOT NULL,
    grp TEXT,
    removed INTEGER NOT NULL DEFAULT 0,
    snapshot_ref TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS worktrees_by_workspace ON worktrees(workspace_id);
CREATE TABLE IF NOT EXISTS port_blocks (
    owner TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    base INTEGER NOT NULL,
    size INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS chats (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    agent TEXT NOT NULL,
    model TEXT,
    effort TEXT,
    permission_mode TEXT NOT NULL,
    title TEXT,
    status TEXT NOT NULL,
    agent_session_id TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS chats_by_workspace ON chats(workspace_id);
CREATE TABLE IF NOT EXISTS chat_send_receipts (
    chat_id TEXT NOT NULL,
    id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    accepted INTEGER NOT NULL,
    PRIMARY KEY(chat_id, id)
);
CREATE TABLE IF NOT EXISTS events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    ts TEXT NOT NULL,
    workspace_id TEXT,
    chat_id TEXT,
    registry INTEGER NOT NULL DEFAULT 0,
    payload TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS events_by_chat ON events(chat_id, seq);
CREATE INDEX IF NOT EXISTS events_by_workspace ON events(workspace_id, seq);
CREATE TABLE IF NOT EXISTS github_repos (
    full_name TEXT PRIMARY KEY COLLATE NOCASE,
    name TEXT NOT NULL,
    owner_is_org INTEGER NOT NULL DEFAULT 0,
    private INTEGER NOT NULL DEFAULT 0,
    default_branch TEXT,
    description TEXT,
    pushed_at TEXT
);
CREATE TABLE IF NOT EXISTS chat_queue (
    id TEXT PRIMARY KEY,
    chat_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    text TEXT NOT NULL,
    attachments TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS chat_queue_by_chat ON chat_queue(chat_id, position);
CREATE TABLE IF NOT EXISTS database_connections (
    id TEXT PRIMARY KEY,
    config TEXT NOT NULL,
    password_key TEXT
);
CREATE TABLE IF NOT EXISTS database_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    connection TEXT NOT NULL,
    sql TEXT NOT NULL,
    at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS kv (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
-- A retired copy set aside while the workspace comes back to replace it, to put it back if the
-- restoration fails before its import.
-- What a workspace held when it was last captured to move, as a package carries it: a copy
-- retired here is compared with it when the workspace comes back.
CREATE TABLE IF NOT EXISTS workspace_departures (
    workspace_id TEXT PRIMARY KEY,
    contents TEXT NOT NULL
);
-- Transfers this daemon drove, the latest per workspace shown (`cloud.transfer.status`).
CREATE TABLE IF NOT EXISTS workspace_transfers (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    slug TEXT NOT NULL,
    started_at TEXT NOT NULL,
    info TEXT NOT NULL
);
-- GCP workers this daemon created (docs/cloud-workers.md), until deleted.
CREATE TABLE IF NOT EXISTS workers (
    name TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    info TEXT NOT NULL
);
-- A retired copy being deleted after the workspace went back where it came from, retried until
-- it is gone.
CREATE TABLE IF NOT EXISTS workspace_discards (
    workspace_id TEXT PRIMARY KEY,
    fence TEXT NOT NULL,
    attempts INTEGER NOT NULL,
    last_error TEXT
);
CREATE TABLE IF NOT EXISTS workspace_returns (
    operation_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    retired TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS workspace_restores (
    workspace_id TEXT PRIMARY KEY,
    operation_id TEXT NOT NULL UNIQUE,
    snapshot_digest TEXT NOT NULL,
    native_root TEXT NOT NULL,
    receipt TEXT
);
CREATE TABLE IF NOT EXISTS restored_chat_blocks (
    chat_id TEXT PRIMARY KEY,
    reason TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS design_shares (
    chat_id TEXT PRIMARY KEY,
    share_id TEXT NOT NULL,
    url TEXT NOT NULL,
    reviewers TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL,
    stopped_at TEXT
);
CREATE TABLE IF NOT EXISTS workspace_shares (
    workspace_id TEXT PRIMARY KEY,
    share_id TEXT NOT NULL,
    url TEXT NOT NULL,
    guests TEXT NOT NULL DEFAULT '[]',
    team INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    stopped_at TEXT
);
CREATE TABLE IF NOT EXISTS suggestions (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    workspace_id TEXT NOT NULL,
    chat_id TEXT NOT NULL,
    author_name TEXT NOT NULL,
    author_email TEXT NOT NULL,
    text TEXT NOT NULL,
    at INTEGER NOT NULL,
    state TEXT NOT NULL,
    decided_at INTEGER
);
CREATE INDEX IF NOT EXISTS suggestions_by_workspace ON suggestions (workspace_id, seq);
CREATE TABLE IF NOT EXISTS share_comments (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    workspace_id TEXT NOT NULL,
    chat_id TEXT NOT NULL,
    json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS routines (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL UNIQUE,
    refine_chat TEXT,
    source_chat TEXT,
    state TEXT NOT NULL,
    version INTEGER NOT NULL,
    checked_until TEXT,
    error TEXT,
    created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS routine_versions (
    routine_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    file TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (routine_id, version)
);
CREATE TABLE IF NOT EXISTS routine_runs (
    chat_id TEXT PRIMARY KEY,
    routine_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    trigger TEXT NOT NULL,
    started_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS routine_suggestions (
    id TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    routine_id TEXT,
    json TEXT NOT NULL,
    created_at TEXT NOT NULL
);
-- The messages a suggestion of Find routines came from: never read again.
CREATE TABLE IF NOT EXISTS routine_suggestion_messages (
    seq INTEGER PRIMARY KEY,
    suggestion_id TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS daily_reviews (
    day TEXT PRIMARY KEY,
    workspace_id TEXT UNIQUE
);
CREATE INDEX IF NOT EXISTS events_by_date ON events(ts);
CREATE TABLE IF NOT EXISTS news_read (
    id TEXT PRIMARY KEY,
    read_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS sections (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    position INTEGER NOT NULL,
    folded INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);
"#;

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
    /// The same database read-only; the writer's own connection in memory, where none can share.
    reader: Arc<Mutex<Connection>>,
}

/// A workspace row; worktrees and activity are joined by the workspace manager.
#[derive(Clone, Debug)]
pub struct WorkspaceRow {
    pub id: String,
    pub slug: String,
    pub path: String,
    pub branch: String,
    pub parent_id: Option<String>,
    pub state: WorkspaceState,
    pub tickets: Vec<TicketRef>,
    pub created_at: String,
    pub archived_at: Option<String>,
    pub last_error: Option<String>,
    /// When a chat last finished, failed or asked for input.
    pub attention_at: Option<String>,
    /// When the user last looked at the workspace.
    pub seen_at: Option<String>,
    /// An exploration: detached checkouts of the base branches, read-only chats.
    pub explore: bool,
    /// The name shown instead of the slug.
    pub title: Option<String>,
    /// Named at random (no ticket, no name, no first message): the first message names it.
    pub auto_title: bool,
    /// Where the title comes from; None while nothing named the workspace.
    pub title_source: Option<TitleSource>,
    /// Kept at the top of the sidebar.
    pub pinned: bool,
    /// Its place in the sidebar, highest first; set by the store when the workspace is inserted.
    pub position: f64,
    /// The sidebar section it is filed in.
    pub section_id: Option<String>,
    /// Its house's place in the Office, once the user moved one.
    pub house_position: Option<u32>,
}

impl WorkspaceRow {
    /// Timestamps share one format (RFC 3339, UTC, milliseconds): they compare as strings.
    pub fn unread(&self) -> bool {
        match (&self.attention_at, &self.seen_at) {
            (Some(attention), Some(seen)) => attention > seen,
            (Some(_), None) => true,
            (None, _) => false,
        }
    }
}

/// Who gave a workspace its title, which decides what may replace it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleSource {
    /// Renamed by the user, or named at creation: nothing replaces it.
    User,
    /// Drawn from the first message: tickets attached later do not replace it.
    Message,
    /// The first ticket's title: follows the first ticket, until the user renames it.
    Ticket,
    /// Drawn from the conversation as the work went on: named again later, as from the message.
    Conversation,
}

impl TitleSource {
    pub fn as_str(self) -> &'static str {
        match self {
            TitleSource::User => "user",
            TitleSource::Message => "message",
            TitleSource::Ticket => "ticket",
            TitleSource::Conversation => "conversation",
        }
    }

    pub fn parse(s: &str) -> Option<TitleSource> {
        match s {
            "user" => Some(TitleSource::User),
            "message" => Some(TitleSource::Message),
            "ticket" => Some(TitleSource::Ticket),
            "conversation" => Some(TitleSource::Conversation),
            _ => None,
        }
    }
}

/// The title a workspace takes from its tickets: the first one's, when Linear described it.
pub fn ticket_title(tickets: &[TicketRef]) -> Option<&str> {
    tickets
        .first()
        .and_then(|t| t.title.as_deref())
        .filter(|t| !t.trim().is_empty())
}

/// A design chat's share (story 51) as the daemon keeps it.
#[derive(Clone, Debug, PartialEq)]
pub struct DesignShareRow {
    pub chat_id: String,
    pub share_id: String,
    /// Where reviewers open it.
    pub url: String,
    pub reviewers: Vec<String>,
    /// Open to every member of the team too.
    pub team: bool,
    pub created_at: String,
    pub stopped_at: Option<String>,
}

/// A workspace's share (story 53), live or stopped.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceShareRow {
    pub chat_ids: Option<Vec<String>>,
    pub workspace_id: String,
    pub share_id: String,
    /// Where guests open it.
    pub url: String,
    pub guests: Vec<chauffeur_protocol::workspace_share::Guest>,
    /// Open to every member of the team too, as viewers.
    pub team: bool,
    pub created_at: String,
    pub stopped_at: Option<String>,
}

#[derive(Clone, Debug)]
pub struct WorktreeRow {
    pub id: String,
    pub workspace_id: String,
    pub repo: String,
    pub clone_path: String,
    pub path: String,
    pub branch: String,
    pub base_branch: String,
    pub base_commit: Option<String>,
    pub origin: WorktreeOrigin,
    pub group: Option<String>,
    pub removed: bool,
    pub snapshot_ref: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug)]
pub struct PortBlock {
    pub owner: String,
    pub workspace_id: String,
    pub base: u16,
    pub size: u16,
}

#[derive(Clone, Debug, Default)]
pub struct CachedRepo {
    pub name: String,
    /// `owner/name`: two owners may have a repo of the same name.
    pub full_name: String,
    pub owner_is_org: bool,
    pub private: bool,
    pub default_branch: Option<String>,
    pub description: Option<String>,
    pub pushed_at: Option<String>,
}

impl CachedRepo {
    pub fn owner(&self) -> &str {
        self.full_name
            .split_once('/')
            .map_or("", |(owner, _)| owner)
    }
}

/// Columns added after a store was first created.
fn migrate(conn: &Connection) -> anyhow::Result<()> {
    // The cache of one organization's repos, keyed by their short name: `github_repos` (every
    // owner's, by `owner/name`) replaced it, refilled at the next listing.
    conn.execute_batch("DROP TABLE IF EXISTS repos_cache")?;
    // The full-text index of archived conversations, kept by triggers on every event: search's
    // own index (`search.db`) covers every workspace now.
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS conversation_search_insert;
         DROP TRIGGER IF EXISTS conversation_search_delete;
         DROP TRIGGER IF EXISTS conversation_search_update;
         DROP VIEW IF EXISTS searchable_messages;
         DROP TABLE IF EXISTS conversation_search;",
    )?;
    if !conn
        .prepare("SELECT 1 FROM pragma_table_info('workspace_shares') WHERE name='chat_ids'")?
        .exists([])?
    {
        conn.execute_batch("ALTER TABLE workspace_shares ADD COLUMN chat_ids TEXT")?;
    }
    // Backups of workspaces running here (a worker: where they go), and where each one's backups
    // stand, as this daemon took them or as the worker a moved one runs on said
    // (cloud_workspace_backup.rs).
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS workspace_backup_configs (
            workspace_id TEXT PRIMARY KEY,
            info TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS workspace_backup_status (
            workspace_id TEXT PRIMARY KEY,
            info TEXT NOT NULL
        );",
    )?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS workspace_imported_database_exports (
            workspace_id TEXT NOT NULL,
            ordinal INTEGER NOT NULL,
            root TEXT NOT NULL,
            receipt TEXT NOT NULL,
            PRIMARY KEY(workspace_id,ordinal)
        );",
    )?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS workspace_database_restores (
            attempt_id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL,
            operation_id TEXT NOT NULL,
            export_name TEXT NOT NULL,
            quarantine TEXT NOT NULL,
            plan TEXT NOT NULL,
            receipt TEXT,
            UNIQUE(workspace_id,export_name)
        );
        CREATE UNIQUE INDEX IF NOT EXISTS workspace_database_restore_pending
            ON workspace_database_restores(workspace_id) WHERE receipt IS NULL;",
    )?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS workspace_database_exports (
        attempt_id TEXT PRIMARY KEY,
        workspace_id TEXT NOT NULL,
        plan TEXT NOT NULL,
        receipt TEXT
    );",
    )?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS workspace_development_profiles (
            workspace_id TEXT NOT NULL,
            revision INTEGER NOT NULL CHECK(revision>0),
            digest TEXT NOT NULL,
            profile TEXT NOT NULL,
            PRIMARY KEY(workspace_id,revision)
        );
        CREATE TABLE IF NOT EXISTS workspace_development_profile_heads (
            workspace_id TEXT PRIMARY KEY,
            revision INTEGER NOT NULL CHECK(revision>0),
            digest TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS workspace_service_provisions (
            workspace_id TEXT PRIMARY KEY,
            operation_id TEXT NOT NULL,
            socket TEXT NOT NULL,
            projects TEXT NOT NULL,
            profile TEXT
        );
        CREATE TABLE IF NOT EXISTS workspace_imported_development_profiles (
            workspace_id TEXT NOT NULL,
            ordinal INTEGER NOT NULL,
            profile TEXT NOT NULL,
            PRIMARY KEY(workspace_id,ordinal)
        );",
    )?;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS workspace_agent_launches (
        launch_id TEXT PRIMARY KEY,
        workspace_id TEXT NOT NULL,
        chat_id TEXT NOT NULL,
        sidecar_generation TEXT NOT NULL,
        phase TEXT NOT NULL CHECK(phase IN ('launching','acknowledged','stopped'))
    ); CREATE INDEX IF NOT EXISTS workspace_agent_launches_pending ON workspace_agent_launches(workspace_id,phase);")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS workspace_agent_roots (
        launch_id TEXT PRIMARY KEY,
        sidecar_pid INTEGER NOT NULL,
        sidecar_stamp TEXT NOT NULL,
        native_pid INTEGER NOT NULL,
        native_stamp TEXT NOT NULL
    )",
    )?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS workspace_stop_attempts (
        workspace_id TEXT PRIMARY KEY,
        fence TEXT NOT NULL,
        daemon_generation TEXT NOT NULL,
        settled INTEGER NOT NULL CHECK(settled IN (0,1))
    )",
    )?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS workspace_preparation_attempts (
        attempt_id TEXT PRIMARY KEY,
        workspace_id TEXT NOT NULL,
        fence TEXT NOT NULL,
        daemon_generation TEXT NOT NULL,
        settled INTEGER NOT NULL CHECK(settled IN (0,1))
    ); CREATE UNIQUE INDEX IF NOT EXISTS workspace_preparation_pending
       ON workspace_preparation_attempts(workspace_id) WHERE settled=0;",
    )?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS workspace_process_sweeps (
        attempt_id TEXT PRIMARY KEY,
        workspace_id TEXT NOT NULL,
        plan TEXT NOT NULL,
        receipt TEXT
    )",
    )?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS workspace_database_stops (
        attempt_id TEXT PRIMARY KEY,
        workspace_id TEXT NOT NULL,
        plan TEXT NOT NULL,
        receipt TEXT
    )",
    )?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS workspace_source_stops (
        digest TEXT PRIMARY KEY,
        workspace_id TEXT NOT NULL,
        fence TEXT NOT NULL,
        receipt TEXT NOT NULL
    )",
    )?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS workspace_service_stops (
        attempt_id TEXT PRIMARY KEY,
        workspace_id TEXT NOT NULL,
        plan TEXT NOT NULL,
        receipt TEXT
    )",
    )?;
    if !conn
        .prepare("SELECT 1 FROM pragma_table_info('workspace_restores') WHERE name='activated'")?
        .exists([])?
    {
        // The committed journal state that let a restored workspace run here.
        conn.execute_batch("ALTER TABLE workspace_restores ADD COLUMN activated TEXT")?;
    }
    if !conn
        .prepare(
            "SELECT 1 FROM pragma_table_info('workspace_service_provisions') WHERE name='keep'",
        )?
        .exists([])?
    {
        // Coming back, the user asked to keep this machine's earlier copy: its volumes set aside
        // stay after activation.
        conn.execute_batch(
            "ALTER TABLE workspace_service_provisions ADD COLUMN keep INTEGER NOT NULL DEFAULT 0",
        )?;
    }
    if !conn
        .prepare("SELECT 1 FROM pragma_table_info('workspace_restores') WHERE name='native_files'")?
        .exists([])?
    {
        // What the restoration added to the machine's native homes, removed with its copy.
        conn.execute_batch("ALTER TABLE workspace_restores ADD COLUMN native_files TEXT")?;
    }
    if !conn
        .prepare("SELECT 1 FROM pragma_table_info('workspace_transfers') WHERE name='discard'")?
        .exists([])?
    {
        // The copy a return left on its source, while its deletion there is pending.
        conn.execute_batch("ALTER TABLE workspace_transfers ADD COLUMN discard TEXT")?;
    }
    if !conn
        .prepare("SELECT 1 FROM pragma_table_info('workspaces') WHERE name='execution_fence'")?
        .exists([])?
    {
        conn.execute_batch("ALTER TABLE workspaces ADD COLUMN execution_fence TEXT")?;
    }
    if !conn
        .prepare("SELECT 1 FROM pragma_table_info('design_shares') WHERE name='team'")?
        .exists([])?
    {
        // A share opened to the team, beside its named reviewers.
        conn.execute_batch("ALTER TABLE design_shares ADD COLUMN team INTEGER NOT NULL DEFAULT 0")?;
    }

    let has_fast = conn
        .prepare("SELECT 1 FROM pragma_table_info('chats') WHERE name = 'fast'")?
        .exists([])?;
    if !has_fast {
        conn.execute_batch("ALTER TABLE chats ADD COLUMN fast INTEGER NOT NULL DEFAULT 0")?;
    }
    let has_attention = conn
        .prepare("SELECT 1 FROM pragma_table_info('workspaces') WHERE name = 'attention_at'")?
        .exists([])?;
    if !has_attention {
        conn.execute_batch(
            "ALTER TABLE workspaces ADD COLUMN attention_at TEXT;
             ALTER TABLE workspaces ADD COLUMN seen_at TEXT;",
        )?;
    }
    let has_explore = conn
        .prepare("SELECT 1 FROM pragma_table_info('workspaces') WHERE name = 'explore'")?
        .exists([])?;
    if !has_explore {
        conn.execute_batch("ALTER TABLE workspaces ADD COLUMN explore INTEGER NOT NULL DEFAULT 0")?;
    }
    let has_pinned = conn
        .prepare("SELECT 1 FROM pragma_table_info('workspaces') WHERE name = 'pinned'")?
        .exists([])?;
    if !has_pinned {
        conn.execute_batch("ALTER TABLE workspaces ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0")?;
    }
    let has_position = conn
        .prepare("SELECT 1 FROM pragma_table_info('workspaces') WHERE name = 'position'")?
        .exists([])?;
    if !has_position {
        // Newest on top, as the sidebar showed them until then.
        conn.execute_batch(
            "ALTER TABLE workspaces ADD COLUMN position REAL NOT NULL DEFAULT 0;
             UPDATE workspaces SET position =
               (SELECT COUNT(*) FROM workspaces AS older WHERE older.created_at <= workspaces.created_at);",
        )?;
    }
    let has_closed = conn
        .prepare("SELECT 1 FROM pragma_table_info('chats') WHERE name = 'closed'")?
        .exists([])?;
    if !has_closed {
        // Every title so far was the automatic one: chats could not be renamed.
        conn.execute_batch(
            "ALTER TABLE chats ADD COLUMN closed INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE chats ADD COLUMN auto_title TEXT;
             UPDATE chats SET auto_title = title;",
        )?;
    }
    let has_section = conn
        .prepare("SELECT 1 FROM pragma_table_info('workspaces') WHERE name = 'section_id'")?
        .exists([])?;
    if !has_section {
        conn.execute_batch("ALTER TABLE workspaces ADD COLUMN section_id TEXT")?;
    }
    let has_title = conn
        .prepare("SELECT 1 FROM pragma_table_info('workspaces') WHERE name = 'title'")?
        .exists([])?;
    if !has_title {
        conn.execute_batch(
            "ALTER TABLE workspaces ADD COLUMN title TEXT;
             ALTER TABLE workspaces ADD COLUMN auto_title INTEGER NOT NULL DEFAULT 0;",
        )?;
    }
    let has_title_source = conn
        .prepare("SELECT 1 FROM pragma_table_info('workspaces') WHERE name = 'title_source'")?
        .exists([])?;
    if !has_title_source {
        // A title so far was the user's or the first message's, which cannot be told apart: both
        // are kept as the user's. A workspace without one takes its first ticket's title.
        conn.execute_batch(
            "ALTER TABLE workspaces ADD COLUMN title_source TEXT;
             UPDATE workspaces SET title_source = 'user' WHERE title IS NOT NULL;",
        )?;
        let untitled: Vec<(String, String)> = conn
            .prepare("SELECT id, tickets FROM workspaces WHERE title IS NULL")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (id, tickets) in untitled {
            let tickets: Vec<TicketRef> = serde_json::from_str(&tickets).unwrap_or_default();
            if let Some(title) = ticket_title(&tickets) {
                conn.execute(
                    "UPDATE workspaces SET title = ?2, title_source = 'ticket' WHERE id = ?1",
                    params![id, title],
                )?;
            }
        }
    }
    let has_kind = conn
        .prepare("SELECT 1 FROM pragma_table_info('chats') WHERE name = 'kind'")?
        .exists([])?;
    if !has_kind {
        // Every chat so far coded; design chats came with story 47.
        conn.execute_batch(
            "ALTER TABLE chats ADD COLUMN kind TEXT NOT NULL DEFAULT 'chat';
             ALTER TABLE chats ADD COLUMN source_chat TEXT;
             ALTER TABLE chats ADD COLUMN brief TEXT;",
        )?;
    }
    let has_turns = conn
        .prepare("SELECT 1 FROM pragma_table_info('workspaces') WHERE name = 'turns'")?
        .exists([])?;
    if !has_turns {
        // Counted from here: a workspace already at work is named again after its next few
        // messages.
        conn.execute_batch(
            "ALTER TABLE workspaces ADD COLUMN turns INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE workspaces ADD COLUMN titled_at_turn INTEGER;",
        )?;
    }
    let has_fork_sessions = conn
        .prepare("SELECT 1 FROM pragma_table_info('chats') WHERE name = 'fork_sessions'")?
        .exists([])?;
    if !has_fork_sessions {
        conn.execute_batch("ALTER TABLE chats ADD COLUMN fork_sessions TEXT")?;
    }
    let has_parent_chat = conn
        .prepare("SELECT 1 FROM pragma_table_info('chats') WHERE name = 'parent_chat'")?
        .exists([])?;
    if !has_parent_chat {
        // Delegated chats came with story 49.
        conn.execute_batch(
            "ALTER TABLE chats ADD COLUMN parent_chat TEXT;
             ALTER TABLE chats ADD COLUMN parent_tool_use TEXT;",
        )?;
    }
    let has_tab_position = conn
        .prepare("SELECT 1 FROM pragma_table_info('chats') WHERE name = 'tab_position'")?
        .exists([])?;
    if !has_tab_position {
        conn.execute_batch("ALTER TABLE chats ADD COLUMN tab_position INTEGER")?;
    }
    let has_house_position = conn
        .prepare("SELECT 1 FROM pragma_table_info('workspaces') WHERE name = 'house_position'")?
        .exists([])?;
    if !has_house_position {
        conn.execute_batch("ALTER TABLE workspaces ADD COLUMN house_position INTEGER")?;
    }
    let has_forked_from_agent = conn
        .prepare("SELECT 1 FROM pragma_table_info('chats') WHERE name = 'forked_from_agent'")?
        .exists([])?;
    if !has_forked_from_agent {
        conn.execute_batch("ALTER TABLE chats ADD COLUMN forked_from_agent TEXT")?;
    }
    // A chat's events of one type, without reading the others' payloads. Built once over the
    // existing history, which can take a few seconds on a large log.
    conn.execute_batch(&format!(
        "CREATE INDEX IF NOT EXISTS events_by_chat_type ON events(chat_id, {EVENT_TYPE}, seq) \
         WHERE chat_id IS NOT NULL"
    ))?;
    Ok(())
}

/// A chat event's `type`, as `events_by_chat_type` indexes it: a query uses that index only when
/// it spells the expression the same way.
const EVENT_TYPE: &str =
    "(CASE WHEN json_valid(payload) THEN json_extract(payload, '$.event.event.type') END)";

/// Envelopes from `(seq, payload)` rows.
fn envelopes(rows: Vec<(i64, String)>) -> Result<Vec<EventEnvelope>> {
    rows.into_iter()
        .map(|(seq, payload)| {
            let mut envelope: EventEnvelope = serde_json::from_str(&payload)?;
            envelope.seq = Some(seq);
            Ok(envelope)
        })
        .collect()
}

/// Inserts a chat into a workspace that still exists: 1 row, or 0 when the workspace is gone.
pub(crate) fn insert_chat_row(c: &Connection, chat: &ChatInfo) -> rusqlite::Result<usize> {
    c.execute(
        "INSERT INTO chats (id, workspace_id, agent, model, effort, fast, permission_mode, title, status, agent_session_id, created_at, closed, auto_title, kind, source_chat, brief, parent_chat, parent_tool_use, tab_position, forked_from_agent)
         SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?8, ?13, ?14, ?15, ?16, ?17, ?18, ?19
         WHERE EXISTS (SELECT 1 FROM workspaces WHERE id = ?2)",
        params![
            chat.id,
            chat.workspace_id,
            chat.agent.as_str(),
            chat.model,
            chat.effort,
            chat.fast,
            chat.permission_mode.as_str(),
            chat.title,
            chat.status.as_str(),
            chat.agent_session_id,
            chat.created_at,
            chat.closed,
            chat.kind.as_str(),
            chat.source_chat,
            brief_json(chat.brief.as_ref()),
            chat.parent_chat,
            chat.parent_tool_use,
            chat.tab_position,
            chat.forked_from_agent.map(AgentKind::as_str)
        ],
    )
}

fn brief_json(brief: Option<&DesignBrief>) -> Option<String> {
    brief.and_then(|brief| serde_json::to_string(brief).ok())
}

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

impl Store {
    pub fn open(path: &Path) -> anyhow::Result<Store> {
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        // Every chat's transcript is in there: the user's alone. SQLite gives the WAL and shm
        // files the database's mode when it creates them; existing ones are fixed too.
        for suffix in ["", "-wal", "-shm"] {
            let mut file = path.as_os_str().to_owned();
            file.push(suffix);
            crate::paths::private_file(Path::new(&file))?;
        }
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        let reader = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("opening {} to read", path.display()))?;
        reader.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(Store {
            conn: Arc::new(Mutex::new(conn)),
            reader: Arc::new(Mutex::new(reader)),
        })
    }

    pub fn open_in_memory() -> anyhow::Result<Store> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        let conn = Arc::new(Mutex::new(conn));
        Ok(Store {
            reader: conn.clone(),
            conn,
        })
    }

    /// Executes a trusted in-process extension operation under the daemon store lock.
    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        Ok(f(&conn)?)
    }

    /// A read through a long history, on the read-only connection: it sees every write committed
    /// before it starts.
    pub(crate) fn read<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T> {
        let conn = self.reader.lock().expect("store reader mutex poisoned");
        Ok(f(&conn)?)
    }

    /// Critical journal transitions share the connection lock and acknowledge only a synced
    /// commit. Restore the ordinary policy even if the operation or transaction fails.
    pub(crate) fn durable<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut conn = self.conn.lock().expect("store mutex poisoned");
        let previous: i64 = conn.pragma_query_value(None, "synchronous", |r| r.get(0))?;
        conn.pragma_update(None, "synchronous", "EXTRA")?;
        let outcome = f(&mut conn);
        let restore = conn.pragma_update(None, "synchronous", previous);
        let value = outcome?;
        restore?;
        Ok(value)
    }

    /// Missing workspace metadata must never be interpreted as an unfenced workspace.
    pub(crate) fn workspace_fence(&self, workspace: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row(
                "SELECT execution_fence FROM workspaces WHERE id=?1",
                [workspace],
                |r| r.get(0),
            )
            .optional()
        })?
        .ok_or_else(|| crate::Error::not_found(format!("workspace {workspace}")))
    }

    /// Fence acknowledgment requires a WAL fsync, unlike ordinary event traffic. Restore
    /// the connection's prior policy even on a failed write; never clear state on errors.
    pub(crate) fn workspace_fence_cas(
        &self,
        workspace: &str,
        expected: Option<&str>,
        next: Option<&str>,
    ) -> Result<bool> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let previous: i64 = conn.pragma_query_value(None, "synchronous", |r| r.get(0))?;
        conn.pragma_update(None, "synchronous", "EXTRA")?;
        let write = conn.execute(
            "UPDATE workspaces SET execution_fence=?3 WHERE id=?1 AND execution_fence IS ?2
             AND NOT EXISTS(SELECT 1 FROM workspace_preparation_attempts WHERE workspace_id=?1 AND settled!=1)",
            params![workspace, expected, next],
        );
        let restore = conn.pragma_update(None, "synchronous", previous);
        let count = write?;
        restore?;
        Ok(count == 1)
    }

    // --- workspaces ---

    pub fn insert_workspace(&self, row: &WorkspaceRow) -> Result<()> {
        self.insert_review_workspace(row, None)
    }

    pub(crate) fn insert_review_workspace(
        &self,
        row: &WorkspaceRow,
        day: Option<&str>,
    ) -> Result<()> {
        let tickets = serde_json::to_string(&row.tickets)?;
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            if let Some(day) = day {
                tx.execute("INSERT INTO daily_reviews (day, workspace_id) VALUES (?1, ?2)", params![day, row.id])?;
            }
            tx.execute(
                "INSERT INTO workspaces (id, slug, path, branch, parent_id, state, tickets, created_at, archived_at, last_error, explore, title, auto_title, title_source, section_id, position)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                         (SELECT COALESCE(MAX(position), 0) + 1 FROM workspaces))",
                params![
                    row.id,
                    row.slug,
                    row.path,
                    row.branch,
                    row.parent_id,
                    row.state.as_str(),
                    tickets,
                    row.created_at,
                    row.archived_at,
                    row.last_error,
                    row.explore,
                    row.title,
                    row.auto_title,
                    row.title_source.map(TitleSource::as_str),
                    row.section_id
                ],
            )?;
            tx.commit()
        })?;
        Ok(())
    }

    pub fn set_workspace_state(
        &self,
        id: &str,
        state: WorkspaceState,
        last_error: Option<&str>,
    ) -> Result<()> {
        let archived_at = (state == WorkspaceState::Archived).then(now);
        self.with(|c| {
            c.execute(
                "UPDATE workspaces SET state = ?2, last_error = ?3,
                    archived_at = CASE WHEN ?2 = 'archived' THEN ?4 WHEN ?2 = 'ready' THEN NULL ELSE archived_at END
                 WHERE id = ?1",
                params![id, state.as_str(), last_error, archived_at],
            )
        })?;
        Ok(())
    }

    /// Moves a workspace among the top-level workspaces shown (`group`, in the sidebar's order):
    /// above `before`, or last; it joins the pinned or the other ones, filed in `section`. When
    /// two neighbours' places are too close to fit one between them, every workspace is numbered
    /// again: returns whether that happened (they all changed).
    pub fn move_workspace(
        &self,
        id: &str,
        group: &[(String, f64)],
        before: Option<&str>,
        pinned: bool,
        section: Option<&str>,
    ) -> Result<bool> {
        let others: Vec<&(String, f64)> = group.iter().filter(|(other, _)| other != id).collect();
        let at = before
            .and_then(|b| others.iter().position(|(other, _)| other == b))
            .unwrap_or(others.len());
        let above = at.checked_sub(1).map(|i| others[i].1);
        let below = others.get(at).map(|(_, p)| *p);
        let place = |above: Option<f64>, below: Option<f64>| match (above, below) {
            (Some(a), Some(b)) => (a + b) / 2.0,
            (None, Some(b)) => b + 1.0,
            (Some(a), None) => a - 1.0,
            (None, None) => 0.0,
        };
        let crowded = matches!((above, below), (Some(a), Some(b)) if a - b < 1e-6);
        self.with(|c| {
            if crowded {
                // Integers again, the order kept; the move then lands between whole numbers.
                let ids: Vec<String> = c
                    .prepare("SELECT id FROM workspaces ORDER BY position ASC, created_at ASC")?
                    .query_map([], |row| row.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                for (i, other) in ids.iter().enumerate() {
                    c.execute(
                        "UPDATE workspaces SET position = ?2 WHERE id = ?1",
                        params![other, (i + 1) as f64],
                    )?;
                }
                let position_of = |other: &str| -> rusqlite::Result<f64> {
                    c.query_row(
                        "SELECT position FROM workspaces WHERE id = ?1",
                        params![other],
                        |row| row.get(0),
                    )
                };
                let above = at
                    .checked_sub(1)
                    .map(|i| position_of(&others[i].0))
                    .transpose()?;
                let below = others.get(at).map(|(o, _)| position_of(o)).transpose()?;
                c.execute(
                    "UPDATE workspaces SET position = ?2, pinned = ?3, section_id = ?4 WHERE id = ?1",
                    params![id, place(above, below), pinned, section],
                )?;
            } else if above.is_some() || below.is_some() {
                c.execute(
                    "UPDATE workspaces SET position = ?2, pinned = ?3, section_id = ?4 WHERE id = ?1",
                    params![id, place(above, below), pinned, section],
                )?;
            } else {
                c.execute(
                    "UPDATE workspaces SET pinned = ?2, section_id = ?3 WHERE id = ?1",
                    params![id, pinned, section],
                )?;
            }
            Ok(crowded)
        })
    }

    pub fn set_workspace_pinned(&self, id: &str, pinned: bool) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE workspaces SET pinned = ?2 WHERE id = ?1",
                params![id, pinned],
            )
        })?;
        Ok(())
    }

    /// Files a workspace and its children in a section, or in none.
    pub fn set_workspace_section(&self, id: &str, section: Option<&str>) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE workspaces SET section_id = ?2 WHERE id = ?1 OR parent_id = ?1",
                params![id, section],
            )
        })?;
        Ok(())
    }

    // --- sections ---

    /// The sidebar's sections, in order.
    pub fn list_sections(&self) -> Result<Vec<SectionInfo>> {
        self.with(|c| {
            c.prepare(
                "SELECT id, name, folded FROM sections ORDER BY position ASC, created_at ASC",
            )?
            .query_map([], |row| {
                Ok(SectionInfo {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    folded: row.get(2)?,
                })
            })?
            .collect()
        })
    }

    /// A new section, last of them.
    pub fn insert_section(&self, id: &str, name: &str, folded: bool) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO sections (id, name, position, folded, created_at)
                 VALUES (?1, ?2, (SELECT COALESCE(MAX(position), 0) + 1 FROM sections), ?3, ?4)",
                params![id, name, folded, now()],
            )
        })?;
        Ok(())
    }

    pub fn rename_section(&self, id: &str, name: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE sections SET name = ?2 WHERE id = ?1",
                params![id, name],
            )
        })?;
        Ok(())
    }

    pub fn set_section_folded(&self, id: &str, folded: bool) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE sections SET folded = ?2 WHERE id = ?1",
                params![id, folded],
            )
        })?;
        Ok(())
    }

    /// Numbers the sections in the order of `ids`.
    pub fn order_sections(&self, ids: &[String]) -> Result<()> {
        self.with(|c| {
            for (i, id) in ids.iter().enumerate() {
                c.execute(
                    "UPDATE sections SET position = ?2 WHERE id = ?1",
                    params![id, i as i64 + 1],
                )?;
            }
            Ok(())
        })
    }

    /// Deletes a section; its workspaces, archived ones included, go back to none. Returns them.
    pub fn delete_section(&self, id: &str) -> Result<Vec<String>> {
        self.with(|c| {
            let filed: Vec<String> = c
                .prepare("SELECT id FROM workspaces WHERE section_id = ?1")?
                .query_map(params![id], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            c.execute(
                "UPDATE workspaces SET section_id = NULL WHERE section_id = ?1",
                params![id],
            )?;
            c.execute("DELETE FROM sections WHERE id = ?1", params![id])?;
            Ok(filed)
        })
    }

    /// A chat of the workspace needs the user: it finished, failed or asks for input.
    pub fn set_workspace_attention(&self, id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE workspaces SET attention_at = ?2 WHERE id = ?1",
                params![id, now()],
            )
        })?;
        Ok(())
    }

    /// The user names the workspace (None: the slug again); nothing replaces their choice, and
    /// the workspace no longer waits for a name.
    pub fn set_workspace_title(&self, id: &str, title: Option<&str>) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE workspaces SET title = ?2, title_source = 'user', auto_title = 0 WHERE id = ?1",
                params![id, title],
            )
        })?;
        Ok(())
    }

    /// The title drawn from the first message, for a workspace nothing named meanwhile. Returns
    /// whether it was taken.
    pub fn set_message_title(&self, id: &str, title: &str) -> Result<bool> {
        let changed = self.with(|c| {
            c.execute(
                "UPDATE workspaces SET title = ?2, title_source = 'message'
                 WHERE id = ?1 AND title_source IS NULL",
                params![id, title],
            )
        })?;
        Ok(changed > 0)
    }

    /// One more message from the user, in any chat of the workspace.
    pub fn count_turn(&self, id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE workspaces SET turns = turns + 1 WHERE id = ?1",
                params![id],
            )
        })?;
        Ok(())
    }

    /// Takes the workspace's next naming from its conversation: true once its `first`th message
    /// came, then once every `every` more, while Chauffeur chose its title (from a message, from
    /// the conversation, or none yet without tickets) and it is ready.
    pub fn take_conversation_title(&self, id: &str, first: u32, every: u32) -> Result<bool> {
        let changed = self.with(|c| {
            c.execute(
                "UPDATE workspaces SET titled_at_turn = turns
                 WHERE id = ?1 AND state = 'ready' AND turns >= ?2
                   AND (titled_at_turn IS NULL OR turns >= titled_at_turn + ?3)
                   AND (title_source IN ('message', 'conversation')
                        OR (title_source IS NULL AND tickets = '[]'))",
                params![id, first, every],
            )
        })?;
        Ok(changed > 0)
    }

    /// The title drawn from the conversation, unless the user or a ticket named the workspace
    /// meanwhile. Returns whether the title changed.
    pub fn set_conversation_title(&self, id: &str, title: &str) -> Result<bool> {
        let changed = self.with(|c| {
            c.execute(
                "UPDATE workspaces SET title = ?2, title_source = 'conversation'
                 WHERE id = ?1 AND title IS NOT ?2
                   AND (title_source IN ('message', 'conversation')
                        OR (title_source IS NULL AND tickets = '[]'))",
                params![id, title],
            )
        })?;
        Ok(changed > 0)
    }

    /// The first ticket's title (when Linear described it), for a workspace without a title or
    /// titled after its tickets: never over the user's or the first message's. Returns whether
    /// the title changed.
    pub fn take_ticket_title(&self, id: &str, tickets: &[TicketRef]) -> Result<bool> {
        let Some(title) = ticket_title(tickets) else {
            return Ok(false);
        };
        let changed = self.with(|c| {
            c.execute(
                "UPDATE workspaces SET title = ?2, title_source = 'ticket'
                 WHERE id = ?1 AND (title_source IS NULL OR title_source = 'ticket')
                   AND title IS NOT ?2",
                params![id, title],
            )
        })?;
        Ok(changed > 0)
    }

    /// A workspace named before anything of it exists on disk: slug, path and branch, and the
    /// title unless something named it meanwhile (its tickets, the user).
    pub fn rename_workspace(&self, row: &WorkspaceRow) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE workspaces SET slug = ?2, path = ?3, branch = ?4, auto_title = 0,
                    title = CASE WHEN title_source IS NULL THEN ?5 ELSE title END,
                    title_source = CASE WHEN title_source IS NULL AND ?5 IS NOT NULL
                                        THEN 'message' ELSE title_source END
                 WHERE id = ?1",
                params![row.id, row.slug, row.path, row.branch, row.title],
            )
        })?;
        Ok(())
    }

    /// An exploration that became a workspace, on `branch`.
    pub fn promote_workspace(&self, id: &str, branch: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE workspaces SET explore = 0, branch = ?2 WHERE id = ?1",
                params![id, branch],
            )
        })?;
        Ok(())
    }

    pub fn set_worktree_branch(&self, id: &str, branch: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE worktrees SET branch = ?2 WHERE id = ?1",
                params![id, branch],
            )
        })?;
        Ok(())
    }

    /// Takes the workspace's wait for a name: true once, for whoever names it. A workspace titled
    /// after its tickets still waits, for its folder and branch.
    pub fn take_auto_title(&self, id: &str) -> Result<bool> {
        let changed = self.with(|c| {
            c.execute(
                "UPDATE workspaces SET auto_title = 0
                 WHERE id = ?1 AND auto_title = 1 AND (title IS NULL OR title_source = 'ticket')",
                params![id],
            )
        })?;
        Ok(changed > 0)
    }

    pub fn set_workspace_seen(&self, id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE workspaces SET seen_at = ?2 WHERE id = ?1",
                params![id, now()],
            )
        })?;
        Ok(())
    }

    pub fn set_workspace_tickets(&self, id: &str, tickets: &[TicketRef]) -> Result<()> {
        let tickets = serde_json::to_string(tickets)?;
        self.with(|c| {
            c.execute(
                "UPDATE workspaces SET tickets = ?2 WHERE id = ?1",
                params![id, tickets],
            )
        })?;
        Ok(())
    }

    pub fn delete_workspace(&self, id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "DELETE FROM chat_queue WHERE chat_id IN (SELECT id FROM chats WHERE workspace_id = ?1)",
                params![id],
            )?;
            // A creation given up after chats were filled (a shared workspace's) keeps no message.
            // The workspace's own events stay: its followers still read why the creation failed.
            c.execute(
                "DELETE FROM events WHERE chat_id IN (SELECT id FROM chats WHERE workspace_id = ?1)",
                params![id],
            )?;
            c.execute("DELETE FROM chat_send_receipts WHERE chat_id IN (SELECT id FROM chats WHERE workspace_id=?1)", params![id])?;
            c.execute("DELETE FROM chats WHERE workspace_id = ?1", params![id])?;
            c.execute("DELETE FROM worktrees WHERE workspace_id = ?1", params![id])?;
            c.execute(
                "DELETE FROM port_blocks WHERE workspace_id = ?1",
                params![id],
            )?;
            c.execute("DELETE FROM workspaces WHERE id = ?1", params![id])
        })?;
        Ok(())
    }

    fn workspace_from_row(row: &Row) -> rusqlite::Result<WorkspaceRow> {
        let state: String = row.get("state")?;
        let tickets: String = row.get("tickets")?;
        Ok(WorkspaceRow {
            id: row.get("id")?,
            slug: row.get("slug")?,
            path: row.get("path")?,
            branch: row.get("branch")?,
            parent_id: row.get("parent_id")?,
            state: WorkspaceState::parse(&state).unwrap_or(WorkspaceState::CleanupFailed),
            tickets: serde_json::from_str(&tickets).unwrap_or_default(),
            created_at: row.get("created_at")?,
            archived_at: row.get("archived_at")?,
            last_error: row.get("last_error")?,
            attention_at: row.get("attention_at")?,
            seen_at: row.get("seen_at")?,
            explore: row.get("explore")?,
            title: row.get("title")?,
            auto_title: row.get("auto_title")?,
            title_source: row
                .get::<_, Option<String>>("title_source")?
                .as_deref()
                .and_then(TitleSource::parse),
            pinned: row.get("pinned")?,
            position: row.get("position")?,
            section_id: row.get("section_id")?,
            house_position: row.get("house_position")?,
        })
    }

    /// Finds a workspace by id or slug.
    pub fn find_workspace(&self, key: &str) -> Result<Option<WorkspaceRow>> {
        self.with(|c| {
            c.query_row(
                "SELECT * FROM workspaces WHERE id = ?1 OR slug = ?1",
                params![key],
                Self::workspace_from_row,
            )
            .optional()
        })
    }

    pub fn list_workspaces(&self) -> Result<Vec<WorkspaceRow>> {
        self.with(|c| {
            let mut stmt =
                c.prepare("SELECT * FROM workspaces ORDER BY pinned DESC, position DESC")?;
            stmt.query_map([], Self::workspace_from_row)?.collect()
        })
    }

    pub fn slug_exists(&self, slug: &str) -> Result<bool> {
        self.with(|c| {
            c.query_row(
                "SELECT 1 FROM workspaces WHERE slug = ?1",
                params![slug],
                |_| Ok(()),
            )
            .optional()
            .map(|found| found.is_some())
        })
    }

    // --- worktrees ---

    pub fn insert_worktree(&self, row: &WorktreeRow) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO worktrees (id, workspace_id, repo, clone_path, path, branch, base_branch, base_commit, origin, grp, removed, snapshot_ref, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    row.id,
                    row.workspace_id,
                    row.repo,
                    row.clone_path,
                    row.path,
                    row.branch,
                    row.base_branch,
                    row.base_commit,
                    origin_str(row.origin),
                    row.group,
                    row.removed,
                    row.snapshot_ref,
                    row.created_at
                ],
            )
        })?;
        Ok(())
    }

    pub fn set_worktree_removed(
        &self,
        id: &str,
        removed: bool,
        snapshot_ref: Option<&str>,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE worktrees SET removed = ?2, snapshot_ref = COALESCE(?3, snapshot_ref) WHERE id = ?1",
                params![id, removed, snapshot_ref],
            )
        })?;
        Ok(())
    }

    pub fn clear_snapshot_ref(&self, id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE worktrees SET snapshot_ref = NULL WHERE id = ?1",
                params![id],
            )
        })?;
        Ok(())
    }

    fn worktree_from_row(row: &Row) -> rusqlite::Result<WorktreeRow> {
        let origin: String = row.get("origin")?;
        Ok(WorktreeRow {
            id: row.get("id")?,
            workspace_id: row.get("workspace_id")?,
            repo: row.get("repo")?,
            clone_path: row.get("clone_path")?,
            path: row.get("path")?,
            branch: row.get("branch")?,
            base_branch: row.get("base_branch")?,
            base_commit: row.get("base_commit")?,
            origin: if origin == "agent" {
                WorktreeOrigin::Agent
            } else {
                WorktreeOrigin::Workspace
            },
            group: row.get("grp")?,
            removed: row.get("removed")?,
            snapshot_ref: row.get("snapshot_ref")?,
            created_at: row.get("created_at")?,
        })
    }

    pub fn worktrees_of(&self, workspace_id: &str) -> Result<Vec<WorktreeRow>> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT * FROM worktrees WHERE workspace_id = ?1 ORDER BY created_at, repo",
            )?;
            stmt.query_map(params![workspace_id], Self::worktree_from_row)?
                .collect()
        })
    }

    /// Every worktree of a repo, by clone or by name, whatever its workspace or state (archived
    /// and removed ones included).
    pub fn worktrees_of_repo(&self, clone_path: &str, repo: &str) -> Result<Vec<WorktreeRow>> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT * FROM worktrees WHERE clone_path = ?1 OR repo = ?2 ORDER BY created_at",
            )?;
            stmt.query_map(params![clone_path, repo], Self::worktree_from_row)?
                .collect()
        })
    }

    /// Archived worktrees whose snapshot is older than `before` (RFC 3339).
    pub fn expired_snapshots(&self, before: &str) -> Result<Vec<WorktreeRow>> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT w.* FROM worktrees w JOIN workspaces s ON s.id = w.workspace_id
                 WHERE w.snapshot_ref IS NOT NULL AND s.state = 'archived' AND s.archived_at < ?1",
            )?;
            stmt.query_map(params![before], Self::worktree_from_row)?
                .collect()
        })
    }

    // --- ports ---

    pub fn port_blocks(&self) -> Result<Vec<PortBlock>> {
        self.with(|c| {
            let mut stmt = c.prepare("SELECT owner, workspace_id, base, size FROM port_blocks")?;
            stmt.query_map([], |row| {
                Ok(PortBlock {
                    owner: row.get(0)?,
                    workspace_id: row.get(1)?,
                    base: row.get(2)?,
                    size: row.get(3)?,
                })
            })?
            .collect()
        })
    }

    pub fn port_block(&self, owner: &str) -> Result<Option<PortBlock>> {
        self.with(|c| {
            c.query_row(
                "SELECT owner, workspace_id, base, size FROM port_blocks WHERE owner = ?1",
                params![owner],
                |row| {
                    Ok(PortBlock {
                        owner: row.get(0)?,
                        workspace_id: row.get(1)?,
                        base: row.get(2)?,
                        size: row.get(3)?,
                    })
                },
            )
            .optional()
        })
    }

    pub fn insert_port_block(&self, block: &PortBlock) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO port_blocks (owner, workspace_id, base, size) VALUES (?1, ?2, ?3, ?4)",
                params![block.owner, block.workspace_id, block.base, block.size],
            )
        })?;
        Ok(())
    }

    pub fn release_port_blocks(&self, workspace_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "DELETE FROM port_blocks WHERE workspace_id = ?1",
                params![workspace_id],
            )
        })?;
        Ok(())
    }

    pub fn release_port_block(&self, owner: &str) -> Result<()> {
        self.with(|c| c.execute("DELETE FROM port_blocks WHERE owner = ?1", params![owner]))?;
        Ok(())
    }

    // --- chats ---

    /// Inserts a chat into a workspace that still exists; false when it is gone (a creation that
    /// failed meanwhile), checked under the same lock as `delete_workspace`.
    pub fn insert_chat(&self, chat: &ChatInfo) -> Result<bool> {
        let inserted = self.with(|c| insert_chat_row(c, chat))?;
        Ok(inserted == 1)
    }

    /// Inserts a chat forked from `source_id` with a copy of the source's stored events (their
    /// times kept), up to the one numbered `up_to` when given, in one transaction: the fork shows
    /// the conversation so far from its first read. `fork_sessions` are the agent sessions its
    /// conversation came through, oldest first. Without `fork_points`, the turns copied lose
    /// theirs: a fork into the other agent cannot fork from sessions that are not its agent's.
    /// Returns the events copied, with their sequence numbers; None when its workspace is gone,
    /// as `insert_chat`'s false.
    pub fn insert_fork(
        &self,
        chat: &ChatInfo,
        source_id: &str,
        fork_sessions: &[String],
        up_to: Option<i64>,
        fork_points: bool,
    ) -> Result<Option<Vec<EventEnvelope>>> {
        let sessions = serde_json::to_string(fork_sessions)?;
        let inserted = self.with(|c| {
            let tx = c.unchecked_transaction()?;
            if insert_chat_row(&tx, chat)? != 1 {
                return Ok(None);
            }
            tx.execute(
                "UPDATE chats SET fork_sessions = ?2 WHERE id = ?1",
                params![chat.id, sessions],
            )?;
            let rows: Vec<(String, Option<String>, String)> = tx
                .prepare(
                    "SELECT ts, workspace_id, payload FROM events WHERE chat_id = ?1 AND registry = 0 AND seq <= ?2 ORDER BY seq",
                )?
                .query_map(params![source_id, up_to.unwrap_or(i64::MAX)], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            let mut insert = tx.prepare(
                "INSERT INTO events (ts, workspace_id, chat_id, registry, payload) VALUES (?1, ?2, ?3, 0, ?4)",
            )?;
            let mut copied = Vec::new();
            for (ts, workspace_id, payload) in rows {
                let Ok(mut envelope) = serde_json::from_str::<EventEnvelope>(&payload) else {
                    // An event this daemon no longer reads: the source cannot show it either.
                    continue;
                };
                let Event::Chat { chat_id, event, .. } = &mut envelope.event else {
                    continue;
                };
                chat_id.clone_from(&chat.id);
                if !fork_points && let ChatEvent::TurnEnded { fork_point, .. } = event {
                    *fork_point = None;
                }
                envelope.seq = None;
                let payload = serde_json::to_string(&envelope)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                insert.execute(params![ts, workspace_id, chat.id, payload])?;
                envelope.seq = Some(tx.last_insert_rowid());
                copied.push(envelope);
            }
            drop(insert);
            tx.commit()?;
            Ok(Some(copied))
        })?;
        Ok(inserted)
    }

    /// A share carries readable history, never a native session, pending request or task.
    pub fn import_shared_history(
        &self,
        chat: &ChatInfo,
        history: &[EventEnvelope],
    ) -> Result<Vec<EventEnvelope>> {
        let copied = self.with(|conn| {
            let mut copied_events = Vec::new();
            let tx = conn.unchecked_transaction()?;
            tx.execute("UPDATE chats SET forked_from_agent = ?2 WHERE id = ?1", params![chat.id, chat.agent.as_str()])?;
            for envelope in history {
                let Event::Chat { event, .. } = &envelope.event else { continue };
                let event = match event {
                    ChatEvent::UserMessage { text, .. } => ChatEvent::UserMessage {
                        text: text.clone(), attachments: None, queued: false,
                        message_id: None, suggested_by: None, delegated_answer: None,
                    },
                    ChatEvent::AssistantText { text, message_id, .. } => ChatEvent::AssistantText {
                        text: text.clone(), message_id: message_id.clone(), parent_tool_use_id: None,
                    },
                    _ => continue,
                };
                let mut copied = EventEnvelope { seq: None, ts: envelope.ts.clone(), event: Event::Chat {
                    workspace_id: chat.workspace_id.clone(), chat_id: chat.id.clone(), event,
                }};
                let payload = serde_json::to_string(&copied).expect("events serialize");
                tx.execute("INSERT INTO events (ts, workspace_id, chat_id, registry, payload) VALUES (?1, ?2, ?3, 0, ?4)", params![copied.ts, chat.workspace_id, chat.id, payload])?;
                copied.seq = Some(tx.last_insert_rowid());
                copied_events.push(copied);
            }
            tx.commit()?;
            Ok(copied_events)
        })?;
        Ok(copied)
    }

    /// The agent sessions a forked chat's conversation came through, oldest first (empty for a
    /// chat that was not forked): their transcripts hold what came before the fork.
    pub fn chat_fork_sessions(&self, id: &str) -> Result<Vec<String>> {
        let json: Option<String> = self.with(|c| {
            c.query_row(
                "SELECT fork_sessions FROM chats WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()
            .map(Option::flatten)
        })?;
        Ok(json
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default())
    }

    /// Stores a chat's settings and status; its title and whether it is closed have their own
    /// setters, so that a copy read earlier cannot undo a rename or a close.
    pub fn update_chat(&self, chat: &ChatInfo) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE chats SET model = ?2, effort = ?3, permission_mode = ?4, status = ?5, agent_session_id = ?6, fast = ?7, fork_sessions = CASE WHEN agent != ?8 THEN NULL ELSE fork_sessions END, agent = ?8, forked_from_agent = ?9 WHERE id = ?1",
                params![
                    chat.id,
                    chat.model,
                    chat.effort,
                    chat.permission_mode.as_str(),
                    chat.status.as_str(),
                    chat.agent_session_id,
                    chat.fast,
                    chat.agent.as_str(),
                    chat.forked_from_agent.map(AgentKind::as_str)
                ],
            )
        })?;
        Ok(())
    }

    fn chat_from_row(row: &Row) -> rusqlite::Result<ChatInfo> {
        let agent: String = row.get("agent")?;
        let mode: String = row.get("permission_mode")?;
        let status: String = row.get("status")?;
        let kind: String = row.get("kind")?;
        let brief: Option<String> = row.get("brief")?;
        let forked_from_agent: Option<String> = row.get("forked_from_agent")?;
        Ok(ChatInfo {
            id: row.get("id")?,
            progress: None,
            workspace_id: row.get("workspace_id")?,
            agent: AgentKind::parse(&agent).unwrap_or_default(),
            model: row.get("model")?,
            effort: row.get("effort")?,
            fast: row.get("fast")?,
            permission_mode: PermissionMode::parse(&mode).unwrap_or_default(),
            title: row.get("title")?,
            status: ChatStatus::parse(&status).unwrap_or(ChatStatus::Error),
            agent_session_id: row.get("agent_session_id")?,
            created_at: row.get("created_at")?,
            // Kept in memory by the chat manager, which fills it in.
            running_tasks: 0,
            permission_mode_effective: None,
            closed: row.get("closed")?,
            // Read from the event log by the chat manager.
            last_activity_at: None,
            // Reported by the agent session, kept in memory by the chat manager.
            mcp_servers: Vec::new(),
            kind: ChatKind::parse(&kind).unwrap_or_default(),
            source_chat: row.get("source_chat")?,
            brief: brief.and_then(|json| serde_json::from_str(&json).ok()),
            parent_chat: row.get("parent_chat")?,
            parent_tool_use: row.get("parent_tool_use")?,
            tab_position: row.get("tab_position")?,
            forked_from_agent: forked_from_agent.as_deref().and_then(AgentKind::parse),
        })
    }

    /// Numbers the Office's houses in the order given, in one transaction.
    pub fn order_houses(&self, ids: &[String]) -> Result<()> {
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            for (i, id) in ids.iter().enumerate() {
                tx.execute(
                    "UPDATE workspaces SET house_position = ?2 WHERE id = ?1",
                    params![id, i as i64],
                )?;
            }
            tx.commit()
        })
    }

    /// Numbers the tabs of a workspace's chats in the order given, in one transaction.
    pub fn order_chat_tabs(&self, ids: &[String]) -> Result<()> {
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            for (i, id) in ids.iter().enumerate() {
                tx.execute(
                    "UPDATE chats SET tab_position = ?2 WHERE id = ?1",
                    params![id, i as i64],
                )?;
            }
            tx.commit()
        })
    }

    /// A design chat's design brief; None once the chat had its first message.
    pub fn set_chat_brief(&self, id: &str, brief: Option<&DesignBrief>) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE chats SET brief = ?2 WHERE id = ?1",
                params![id, brief_json(brief)],
            )
        })?;
        Ok(())
    }

    /// The title shown; None for the automatic one.
    pub fn set_chat_title(&self, id: &str, title: Option<&str>) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE chats SET title = COALESCE(?2, auto_title) WHERE id = ?1",
                params![id, title],
            )
        })?;
        Ok(())
    }

    pub fn set_chat_closed(&self, id: &str, closed: bool) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE chats SET closed = ?2 WHERE id = ?1",
                params![id, closed],
            )
        })?;
        Ok(())
    }

    /// When a chat last had an event stored.
    pub fn chat_last_event_at(&self, id: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row(
                "SELECT ts FROM events WHERE chat_id = ?1 ORDER BY seq DESC LIMIT 1",
                params![id],
                |row| row.get(0),
            )
            .optional()
        })
    }

    /// Up to `limit` of the chat's events after `after_seq`, oldest first, each with its stored
    /// size. Once the page's payloads would pass `max_bytes` in all, the next row comes back
    /// with None and the page ends: its payload is never loaded.
    pub fn chat_event_page(
        &self,
        chat_id: &str,
        after_seq: i64,
        limit: u32,
        max_bytes: u64,
    ) -> Result<Vec<(i64, i64, Option<String>)>> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT seq, length(CAST(payload AS BLOB)), payload
                 FROM events WHERE chat_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
            )?;
            let mut rows = stmt.query(params![chat_id, after_seq, limit])?;
            let (mut page, mut total) = (Vec::new(), 0u64);
            while let Some(row) = rows.next()? {
                let (seq, size): (i64, i64) = (row.get(0)?, row.get(1)?);
                total += size as u64;
                if total > max_bytes {
                    page.push((seq, size, None));
                    break;
                }
                page.push((seq, size, Some(row.get(2)?)));
            }
            Ok(page)
        })
    }

    /// The sequence number of the chat's last stored event.
    pub fn chat_last_seq(&self, id: &str) -> Result<Option<i64>> {
        self.with(|c| {
            c.query_row(
                "SELECT MAX(seq) FROM events WHERE chat_id = ?1",
                params![id],
                |row| row.get(0),
            )
        })
    }

    /// Deletes a chat with its queue, its events and the registry's records of it.
    pub fn delete_chat(&self, id: &str) -> Result<()> {
        // Chat ids are UUIDs: nothing in them for LIKE to read as a wildcard.
        let upserts = format!("%\"chat\":{{\"id\":\"{id}\"%");
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            tx.execute("DELETE FROM chat_queue WHERE chat_id = ?1", params![id])?;
            tx.execute("DELETE FROM events WHERE chat_id = ?1", params![id])?;
            tx.execute(
                "DELETE FROM events WHERE registry = 1 AND payload LIKE ?1",
                params![upserts],
            )?;
            tx.execute(
                "DELETE FROM chat_send_receipts WHERE chat_id=?1",
                params![id],
            )?;
            tx.execute("DELETE FROM chats WHERE id = ?1", params![id])?;
            tx.commit()
        })?;
        Ok(())
    }

    pub fn chat(&self, id: &str) -> Result<Option<ChatInfo>> {
        self.with(|c| {
            c.query_row(
                "SELECT * FROM chats WHERE id = ?1",
                params![id],
                Self::chat_from_row,
            )
            .optional()
        })
    }

    pub fn chats_of(&self, workspace_id: &str) -> Result<Vec<ChatInfo>> {
        self.with(|c| {
            let mut stmt =
                c.prepare("SELECT * FROM chats WHERE workspace_id = ?1 ORDER BY created_at")?;
            stmt.query_map(params![workspace_id], Self::chat_from_row)?
                .collect()
        })
    }

    /// The chats whose turn runs, and every chat of a workspace being created, with the ids of
    /// the workspaces being created.
    pub fn chats_at_work(&self) -> Result<(Vec<ChatInfo>, Vec<String>)> {
        self.with(|c| {
            let creating = c
                .prepare("SELECT id FROM workspaces WHERE state = 'creating'")?
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            let chats = c
                .prepare(
                    "SELECT * FROM chats WHERE status IN ('starting', 'working') \
                     OR workspace_id IN (SELECT id FROM workspaces WHERE state = 'creating')",
                )?
                .query_map([], Self::chat_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((chats, creating))
        })
    }

    pub fn all_chats(&self) -> Result<Vec<ChatInfo>> {
        self.with(|c| {
            let mut stmt = c.prepare("SELECT * FROM chats ORDER BY created_at")?;
            stmt.query_map([], Self::chat_from_row)?.collect()
        })
    }

    // --- events ---

    /// Appends an event and returns its sequence number.
    pub fn append_event(
        &self,
        ts: &str,
        workspace_id: Option<&str>,
        chat_id: Option<&str>,
        registry: bool,
        payload: &str,
    ) -> Result<i64> {
        self.with(|c| {
            c.execute(
                "INSERT INTO events (ts, workspace_id, chat_id, registry, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![ts, workspace_id, chat_id, registry, payload],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn last_seq(&self) -> Result<i64> {
        self.with(|c| c.query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| r.get(0)))
    }

    /// Persisted events after `after_seq`, oldest first, matching the filter.
    pub fn events_after(&self, after_seq: i64, filter: &EventFilter) -> Result<Vec<EventEnvelope>> {
        self.events_part(after_seq, filter, None)
    }

    /// `events_after`, or one `part` of it: each chat's last events, or the older ones. An event
    /// that belongs to no chat is always in the tail.
    pub fn events_part(
        &self,
        after_seq: i64,
        filter: &EventFilter,
        part: Option<Replay>,
    ) -> Result<Vec<EventEnvelope>> {
        self.events_page(after_seq, None, filter, part)
    }

    pub fn events_page(
        &self,
        after_seq: i64,
        before_seq: Option<i64>,
        filter: &EventFilter,
        part: Option<Replay>,
    ) -> Result<Vec<EventEnvelope>> {
        let rows: Vec<(i64, String)> = self.read(|c| {
            // Filter before LIMIT: unrelated chats must not consume a page, otherwise a
            // client can mistake an empty page for the end and replay everything live.
            let mut scope = String::from("seq > ?");
            let mut values = vec![rusqlite::types::Value::Integer(after_seq)];
            if let Some(before) = before_seq {
                scope.push_str(" AND seq < ?");
                values.push(rusqlite::types::Value::Integer(before));
            }
            if !filter.all {
                let mut scopes = Vec::new();
                if filter.registry {
                    scopes.push(String::from("registry = 1"));
                }
                for (column, ids) in [
                    ("chat_id", &filter.chats),
                    ("workspace_id", &filter.workspaces),
                ] {
                    if !ids.is_empty() {
                        let placeholders = vec!["?"; ids.len()].join(",");
                        scopes.push(format!("(registry = 0 AND {column} IN ({placeholders}))"));
                        values.extend(ids.iter().cloned().map(rusqlite::types::Value::Text));
                    }
                }
                scope.push_str(" AND (");
                scope.push_str(&if scopes.is_empty() {
                    String::from("0")
                } else {
                    scopes.join(" OR ")
                });
                scope.push(')');
            }
            let sql = match part {
                None => {
                    format!("SELECT seq, payload FROM events WHERE {scope} ORDER BY seq LIMIT ?")
                }
                Some(part) => {
                    // Counted among the events the filter lets through, from each chat's newest.
                    // The window sorts sequence numbers only: payloads, a workspace's tool
                    // outputs included, are read for the rows kept.
                    let (keep, per_chat) = match part {
                        Replay::Tail { per_chat } => ("chat_id IS NULL OR rn <= ?", per_chat),
                        Replay::BeforeTail { per_chat } => {
                            ("chat_id IS NOT NULL AND rn > ?", per_chat)
                        }
                    };
                    values.push(rusqlite::types::Value::Integer(i64::from(per_chat)));
                    format!(
                        "SELECT seq, payload FROM events WHERE seq IN (\
                         SELECT seq FROM (SELECT seq, chat_id, ROW_NUMBER() OVER \
                         (PARTITION BY chat_id ORDER BY seq DESC) AS rn FROM events WHERE {scope}) \
                         WHERE {keep} ORDER BY seq LIMIT ?) ORDER BY seq"
                    )
                }
            };
            values.push(rusqlite::types::Value::Integer(i64::from(
                filter.limit.unwrap_or(100_000),
            )));
            let mut stmt = c.prepare(&sql)?;
            stmt.query_map(rusqlite::params_from_iter(values), |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect()
        })?;
        envelopes(rows)
    }

    /// A chat's stored events, oldest first.
    pub fn events_of_chat(&self, chat_id: &str) -> Result<Vec<EventEnvelope>> {
        let rows: Vec<(i64, String)> = self.read(|c| {
            let mut stmt =
                c.prepare("SELECT seq, payload FROM events WHERE chat_id = ?1 ORDER BY seq")?;
            stmt.query_map(params![chat_id], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })?;
        envelopes(rows)
    }

    /// How a chat's turn numbered `turn` (counted from 1 in the order of its `turn_ended` events)
    /// ended: the seq of that event, its outcome and its fork point.
    pub fn turn_end(
        &self,
        chat_id: &str,
        turn: u32,
    ) -> Result<Option<(i64, TurnOutcome, Option<ForkPoint>)>> {
        let rows: Vec<(i64, String)> = self.read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT seq, payload FROM events WHERE chat_id = ?1 AND {EVENT_TYPE} = 'turn_ended' AND registry = 0 ORDER BY seq"
            ))?;
            stmt.query_map(params![chat_id], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })?;
        let mut ends = rows.into_iter().filter_map(|(seq, payload)| {
            let envelope: EventEnvelope = serde_json::from_str(&payload).ok()?;
            let Event::Chat {
                event:
                    ChatEvent::TurnEnded {
                        outcome,
                        fork_point,
                        ..
                    },
                ..
            } = envelope.event
            else {
                return None;
            };
            Some((seq, outcome, fork_point))
        });
        Ok(turn.checked_sub(1).and_then(|n| ends.nth(n as usize)))
    }

    /// The latest persisted chat events of one `type`, newest first.
    pub fn last_events_of_type(
        &self,
        chat_id: &str,
        kind: &str,
        limit: u32,
    ) -> Result<Vec<EventEnvelope>> {
        let rows: Vec<(i64, String)> = self.read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT seq, payload FROM events WHERE chat_id = ?1 AND {EVENT_TYPE} = ?2 ORDER BY seq DESC LIMIT ?3"
            ))?;
            stmt.query_map(params![chat_id, kind, limit], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })?;
        envelopes(rows)
    }

    /// A chat's persisted events of one `type` after the sequence number `after`, oldest first.
    pub fn events_of_type_after(
        &self,
        chat_id: &str,
        kind: &str,
        after: i64,
    ) -> Result<Vec<EventEnvelope>> {
        let rows: Vec<(i64, String)> = self.read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT seq, payload FROM events WHERE chat_id = ?1 AND {EVENT_TYPE} = ?3 AND seq > ?2 ORDER BY seq"
            ))?;
            stmt.query_map(params![chat_id, after, kind], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect()
        })?;
        envelopes(rows)
    }

    /// The events of a chat whose payload holds `needle` (e.g. a tool use id), oldest first.
    pub fn chat_events_mentioning(
        &self,
        chat_id: &str,
        needle: &str,
        limit: u32,
    ) -> Result<Vec<EventEnvelope>> {
        let escaped = needle
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("%{escaped}%");
        let rows: Vec<(i64, String)> = self.read(|c| {
            let mut stmt = c.prepare(
                "SELECT seq, payload FROM events WHERE chat_id = ?1 AND payload LIKE ?2 ESCAPE '\\' ORDER BY seq LIMIT ?3",
            )?;
            stmt.query_map(params![chat_id, pattern, limit], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })?;
        envelopes(rows)
    }

    /// The workspace's chats' tool calls whose payload holds `needle`, newest first.
    pub fn workspace_tool_calls_mentioning(
        &self,
        workspace_id: &str,
        needle: &str,
        limit: u32,
    ) -> Result<Vec<EventEnvelope>> {
        let escaped = needle
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("%{escaped}%");
        let rows: Vec<(i64, String)> = self.read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT seq, payload FROM events WHERE workspace_id = ?1 AND chat_id IS NOT NULL AND {EVENT_TYPE} = 'tool_call' AND payload LIKE ?2 ESCAPE '\\' ORDER BY seq DESC LIMIT ?3"
            ))?;
            stmt.query_map(params![workspace_id, pattern, limit], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })?;
        envelopes(rows)
    }

    // --- chat queues ---

    pub fn queue_of(&self, chat_id: &str) -> Result<Vec<QueuedMessage>> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT id, text, attachments, created_at FROM chat_queue WHERE chat_id = ?1 ORDER BY position",
            )?;
            stmt.query_map(params![chat_id], |row| {
                let attachments: String = row.get(2)?;
                Ok(QueuedMessage {
                    id: row.get(0)?,
                    text: row.get(1)?,
                    attachments: serde_json::from_str(&attachments).unwrap_or_default(),
                    created_at: row.get(3)?,
                })
            })?
            .collect()
        })
    }

    /// Queues a message for a chat that still exists; false when it is gone (deleted, or taken
    /// away with a creation that failed).
    pub fn queue_push(&self, message: &QueuedMessage, chat_id: &str) -> Result<bool> {
        self.queue_insert(message, chat_id, None)
    }

    /// Queues a message for a chat whose workspace is still being created; false once the
    /// creation ended (failed: the chat is gone; done: its queue was already handed to the agent).
    /// Checked under the same lock as `delete_workspace` and the workspace's state change.
    pub fn queue_push_while_creating(
        &self,
        message: &QueuedMessage,
        chat_id: &str,
    ) -> Result<bool> {
        self.queue_insert(message, chat_id, Some(WorkspaceState::Creating))
    }

    fn queue_insert(
        &self,
        message: &QueuedMessage,
        chat_id: &str,
        state: Option<WorkspaceState>,
    ) -> Result<bool> {
        let attachments = serde_json::to_string(&message.attachments)?;
        let inserted = self.with(|c| {
            c.execute(
                "INSERT INTO chat_queue (id, chat_id, position, text, attachments, created_at)
                 SELECT ?1, ?2, (SELECT COALESCE(MAX(position), 0) + 1 FROM chat_queue WHERE chat_id = ?2), ?3, ?4, ?5
                 WHERE EXISTS (
                     SELECT 1 FROM chats JOIN workspaces ON workspaces.id = chats.workspace_id
                     WHERE chats.id = ?2 AND (?6 IS NULL OR workspaces.state = ?6)
                 )",
                params![
                    message.id,
                    chat_id,
                    message.text,
                    attachments,
                    message.created_at,
                    state.map(|s| s.as_str())
                ],
            )
        })?;
        Ok(inserted == 1)
    }

    /// Keeps the listed messages in that order with those texts; removes the others.
    pub fn queue_set(&self, chat_id: &str, items: &[QueueItem]) -> Result<()> {
        let mut conn = self.conn.lock().expect("store mutex poisoned");
        let tx = conn.transaction()?;
        let ids: Vec<String> = {
            let mut stmt = tx.prepare("SELECT id FROM chat_queue WHERE chat_id = ?1")?;
            stmt.query_map(params![chat_id], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        for id in ids.iter().filter(|id| !items.iter().any(|i| &i.id == *id)) {
            tx.execute("DELETE FROM chat_queue WHERE id = ?1", params![id])?;
        }
        for (position, item) in items.iter().enumerate() {
            tx.execute(
                "UPDATE chat_queue SET position = ?3, text = ?4 WHERE id = ?1 AND chat_id = ?2",
                params![item.id, chat_id, position as i64 + 1, item.text],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Removes and returns one queued message.
    pub fn queue_take(&self, chat_id: &str, id: &str) -> Result<Option<QueuedMessage>> {
        let message = self.queue_of(chat_id)?.into_iter().find(|m| m.id == id);
        if message.is_some() {
            self.with(|c| c.execute("DELETE FROM chat_queue WHERE id = ?1", params![id]))?;
        }
        Ok(message)
    }

    /// Removes and returns the first queued message.
    pub fn queue_pop(&self, chat_id: &str) -> Result<Option<QueuedMessage>> {
        match self.queue_of(chat_id)?.into_iter().next() {
            Some(first) => self.queue_take(chat_id, &first.id),
            None => Ok(None),
        }
    }

    // --- repos cache and kv ---

    pub fn replace_repos_cache(&self, repos: &[CachedRepo]) -> Result<()> {
        let mut conn = self.conn.lock().expect("store mutex poisoned");
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM github_repos", [])?;
        for repo in repos {
            tx.execute(
                "INSERT OR REPLACE INTO github_repos (full_name, name, owner_is_org, private, default_branch, description, pushed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![repo.full_name, repo.name, repo.owner_is_org, repo.private, repo.default_branch, repo.description, repo.pushed_at],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// When a workspace last took in each clone (its worktrees made with the workspace or added
    /// to it, not the agents'), by clone path.
    pub fn last_use_by_clone(&self) -> Result<std::collections::HashMap<String, String>> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT clone_path, MAX(created_at) FROM worktrees WHERE origin = 'workspace' GROUP BY clone_path",
            )?;
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })
    }

    pub fn repos_cache(&self) -> Result<Vec<CachedRepo>> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT name, full_name, owner_is_org, private, default_branch, description, pushed_at FROM github_repos ORDER BY pushed_at DESC",
            )?;
            stmt.query_map([], |row| {
                Ok(CachedRepo {
                    name: row.get(0)?,
                    full_name: row.get(1)?,
                    owner_is_org: row.get(2)?,
                    private: row.get(3)?,
                    default_branch: row.get(4)?,
                    description: row.get(5)?,
                    pushed_at: row.get(6)?,
                })
            })?
            .collect()
        })
    }

    /// A design chat's share (story 51), live or stopped; none before one.
    pub fn design_share(&self, chat_id: &str) -> Result<Option<DesignShareRow>> {
        self.with(|c| {
            c.query_row(
                "SELECT chat_id, share_id, url, reviewers, created_at, stopped_at, team FROM design_shares WHERE chat_id = ?1",
                params![chat_id],
                Self::design_share_from_row,
            )
            .optional()
        })
    }

    /// Every live share, to resume at start.
    pub fn design_shares_live(&self) -> Result<Vec<DesignShareRow>> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT chat_id, share_id, url, reviewers, created_at, stopped_at, team FROM design_shares WHERE stopped_at IS NULL",
            )?;
            stmt.query_map([], Self::design_share_from_row)?.collect()
        })
    }

    fn design_share_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DesignShareRow> {
        let reviewers: String = row.get(3)?;
        Ok(DesignShareRow {
            chat_id: row.get(0)?,
            share_id: row.get(1)?,
            url: row.get(2)?,
            reviewers: serde_json::from_str(&reviewers).unwrap_or_default(),
            created_at: row.get(4)?,
            stopped_at: row.get(5)?,
            team: row.get(6)?,
        })
    }

    /// A new share takes the chat's row (an earlier one, stopped, is forgotten).
    pub fn design_share_put(&self, row: &DesignShareRow) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO design_shares (chat_id, share_id, url, reviewers, created_at, stopped_at, team) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
                 ON CONFLICT (chat_id) DO UPDATE SET share_id = excluded.share_id, url = excluded.url, reviewers = excluded.reviewers, created_at = excluded.created_at, stopped_at = excluded.stopped_at, team = excluded.team",
                params![
                    row.chat_id,
                    row.share_id,
                    row.url,
                    serde_json::to_string(&row.reviewers).expect("reviewers serialize"),
                    row.created_at,
                    row.stopped_at,
                    row.team
                ],
            )?;
            Ok(())
        })
    }

    /// A workspace's share (story 53), live or stopped; none before one.
    pub fn workspace_share(&self, workspace_id: &str) -> Result<Option<WorkspaceShareRow>> {
        self.with(|c| {
            c.query_row(
                "SELECT workspace_id, share_id, url, guests, team, created_at, stopped_at, chat_ids FROM workspace_shares WHERE workspace_id = ?1",
                params![workspace_id],
                Self::workspace_share_from_row,
            )
            .optional()
        })
    }

    /// Every live workspace share, to resume at start.
    pub fn workspace_shares_live(&self) -> Result<Vec<WorkspaceShareRow>> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT workspace_id, share_id, url, guests, team, created_at, stopped_at, chat_ids FROM workspace_shares WHERE stopped_at IS NULL",
            )?;
            stmt.query_map([], Self::workspace_share_from_row)?.collect()
        })
    }

    fn workspace_share_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkspaceShareRow> {
        let guests: String = row.get(3)?;
        Ok(WorkspaceShareRow {
            chat_ids: row
                .get::<_, Option<String>>(7)?
                .map(|value| serde_json::from_str(&value))
                .transpose()
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        7,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
            workspace_id: row.get(0)?,
            share_id: row.get(1)?,
            url: row.get(2)?,
            guests: serde_json::from_str(&guests).unwrap_or_default(),
            team: row.get(4)?,
            created_at: row.get(5)?,
            stopped_at: row.get(6)?,
        })
    }

    /// A new share takes the workspace's row (an earlier one, stopped, is forgotten).
    pub fn workspace_share_put(&self, row: &WorkspaceShareRow) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO workspace_shares (workspace_id, share_id, url, guests, team, created_at, stopped_at, chat_ids) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
                 ON CONFLICT (workspace_id) DO UPDATE SET share_id = excluded.share_id, url = excluded.url, guests = excluded.guests, team = excluded.team, created_at = excluded.created_at, stopped_at = excluded.stopped_at, chat_ids = excluded.chat_ids",
                params![
                    row.workspace_id,
                    row.share_id,
                    row.url,
                    serde_json::to_string(&row.guests).expect("guests serialize"),
                    row.team,
                    row.created_at,
                    row.stopped_at,
                    row.chat_ids.as_ref().map(|ids| serde_json::to_string(ids).expect("chat ids serialize"))
                ],
            )?;
            Ok(())
        })
    }

    /// A guest's suggestion, kept once: a suggestion the store has already keeps its state
    /// (the service may tell it again on reconnection). Whether it was new.
    pub fn suggestion_insert(&self, workspace_id: &str, suggestion: &Suggestion) -> Result<bool> {
        self.with(|c| {
            let inserted = c.execute(
                "INSERT OR IGNORE INTO suggestions (id, workspace_id, chat_id, author_name, author_email, text, at, state, decided_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    suggestion.id,
                    workspace_id,
                    suggestion.chat,
                    suggestion.author.name,
                    suggestion.author.email,
                    suggestion.text,
                    suggestion.at as i64,
                    suggestion_state_str(suggestion.state),
                    suggestion.decided_at.map(|v| v as i64)
                ],
            )?;
            Ok(inserted > 0)
        })
    }

    pub fn suggestion(&self, id: &str) -> Result<Option<Suggestion>> {
        self.with(|c| {
            c.query_row(
                &format!("SELECT {SUGGESTION_COLUMNS} FROM suggestions WHERE id = ?1"),
                params![id],
                suggestion_from_row,
            )
            .optional()
        })
    }

    /// A workspace's suggestions, oldest first.
    pub fn suggestions_of_workspace(&self, workspace_id: &str) -> Result<Vec<Suggestion>> {
        self.with(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {SUGGESTION_COLUMNS} FROM suggestions WHERE workspace_id = ?1 ORDER BY seq"
            ))?;
            stmt.query_map(params![workspace_id], suggestion_from_row)?
                .collect()
        })
    }

    /// A chat's suggestions, oldest first.
    pub fn suggestions_of_chat(&self, chat_id: &str) -> Result<Vec<Suggestion>> {
        self.with(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {SUGGESTION_COLUMNS} FROM suggestions WHERE chat_id = ?1 ORDER BY seq"
            ))?;
            stmt.query_map(params![chat_id], suggestion_from_row)?
                .collect()
        })
    }

    /// Decides a pending suggestion. False when it was not pending any more.
    pub fn suggestion_decide(&self, id: &str, state: SuggestionState, at: u64) -> Result<bool> {
        self.with(|c| {
            let changed = c.execute(
                "UPDATE suggestions SET state = ?2, decided_at = ?3 WHERE id = ?1 AND state = 'pending'",
                params![id, suggestion_state_str(state), at as i64],
            )?;
            Ok(changed > 0)
        })
    }

    /// Its author took a suggestion back, unless it was sent meanwhile. Whether it changed.
    pub fn suggestion_delete(&self, id: &str) -> Result<bool> {
        self.with(|c| {
            let changed = c.execute(
                "UPDATE suggestions SET state = 'deleted' WHERE id = ?1 AND state IN ('pending', 'dismissed')",
                params![id],
            )?;
            Ok(changed > 0)
        })
    }

    /// The guests' comments on the shared chats of a workspace, whole, as the share service holds
    /// them: those it no longer has were deleted by their authors. The comments of a chat no
    /// longer shared stay: the service dropped them with the chat, the owner keeps them.
    pub fn share_comments_replace(
        &self,
        workspace_id: &str,
        chats: &[String],
        comments: &[ShareComment],
    ) -> Result<()> {
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            for chat in chats {
                tx.execute(
                    "DELETE FROM share_comments WHERE workspace_id = ?1 AND chat_id = ?2",
                    params![workspace_id, chat],
                )?;
            }
            for comment in comments {
                tx.execute(
                    "INSERT OR IGNORE INTO share_comments (id, workspace_id, chat_id, json) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        comment.id,
                        workspace_id,
                        comment.chat,
                        serde_json::to_string(comment).expect("comments serialize")
                    ],
                )?;
            }
            tx.commit()
        })
    }

    /// A chat's comments, oldest first.
    pub fn share_comments_of_chat(&self, chat_id: &str) -> Result<Vec<ShareComment>> {
        self.with(|c| {
            let mut stmt =
                c.prepare("SELECT json FROM share_comments WHERE chat_id = ?1 ORDER BY seq")?;
            let comments = stmt
                .query_map(params![chat_id], |row| row.get::<_, String>(0))?
                .filter_map(|json| serde_json::from_str(&json.ok()?).ok())
                .collect();
            Ok(comments)
        })
    }

    /// Puts a suggestion back to pending: its sending failed after it was marked sent.
    pub fn suggestion_reopen(&self, id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE suggestions SET state = 'pending', decided_at = NULL WHERE id = ?1",
                params![id],
            )?;
            Ok(())
        })
    }

    pub fn kv_get(&self, key: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row("SELECT value FROM kv WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()
        })
    }

    pub fn kv_set(&self, key: &str, value: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO kv (key, value) VALUES (?1, ?2)",
                params![key, value],
            )
        })?;
        Ok(())
    }

    pub fn kv_delete(&self, key: &str) -> Result<()> {
        self.with(|c| c.execute("DELETE FROM kv WHERE key = ?1", params![key]))?;
        Ok(())
    }

    /// Marks news items read (`crate::news`), forgetting those read more than 90 days ago: no
    /// source keeps an item that long.
    pub fn mark_news_read(&self, ids: &[String]) -> Result<()> {
        let at = now();
        let forgotten = (chrono::Utc::now() - chrono::Duration::days(90))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        self.with(|c| {
            for id in ids {
                c.execute(
                    "INSERT OR IGNORE INTO news_read (id, read_at) VALUES (?1, ?2)",
                    params![id, at],
                )?;
            }
            c.execute(
                "DELETE FROM news_read WHERE read_at < ?1",
                params![forgotten],
            )
        })?;
        Ok(())
    }

    pub fn news_read(&self) -> Result<std::collections::HashSet<String>> {
        self.with(|c| {
            c.prepare("SELECT id FROM news_read")?
                .query_map([], |r| r.get(0))?
                .collect()
        })
    }
}

fn origin_str(origin: WorktreeOrigin) -> &'static str {
    match origin {
        WorktreeOrigin::Workspace => "workspace",
        WorktreeOrigin::Agent => "agent",
    }
}

/// Which events a subscriber wants.
#[derive(Clone, Debug, Default)]
pub struct EventFilter {
    pub all: bool,
    pub registry: bool,
    /// The `office` scope (live only: it is never stored).
    pub office: bool,
    pub workspaces: Vec<String>,
    pub chats: Vec<String>,
    pub limit: Option<u32>,
}

impl EventFilter {
    pub fn matches(
        &self,
        workspace_id: Option<&str>,
        chat_id: Option<&str>,
        registry: bool,
    ) -> bool {
        if self.all {
            return true;
        }
        if registry {
            return self.registry;
        }
        if let Some(chat_id) = chat_id
            && self.chats.iter().any(|c| c == chat_id)
        {
            return true;
        }
        if let Some(workspace_id) = workspace_id
            && self.workspaces.iter().any(|w| w == workspace_id)
        {
            return true;
        }
        false
    }
}

const SUGGESTION_COLUMNS: &str =
    "id, chat_id, author_name, author_email, text, at, state, decided_at";

fn suggestion_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Suggestion> {
    let state: String = row.get(6)?;
    Ok(Suggestion {
        id: row.get(0)?,
        chat: row.get(1)?,
        author: chauffeur_protocol::client::CommentAuthor {
            name: row.get(2)?,
            email: row.get(3)?,
        },
        text: row.get(4)?,
        at: row.get::<_, i64>(5)? as u64,
        state: match state.as_str() {
            "sent" => SuggestionState::Sent,
            "sent_edited" => SuggestionState::SentEdited,
            "dismissed" => SuggestionState::Dismissed,
            "deleted" => SuggestionState::Deleted,
            _ => SuggestionState::Pending,
        },
        decided_at: row.get::<_, Option<i64>>(7)?.map(|v| v as u64),
    })
}

fn suggestion_state_str(state: SuggestionState) -> &'static str {
    match state {
        SuggestionState::Pending => "pending",
        SuggestionState::Sent => "sent",
        SuggestionState::SentEdited => "sent_edited",
        SuggestionState::Dismissed => "dismissed",
        SuggestionState::Deleted => "deleted",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suggestion(id: &str, chat: &str) -> Suggestion {
        Suggestion {
            id: id.into(),
            chat: chat.into(),
            author: chauffeur_protocol::client::CommentAuthor {
                name: "Bob".into(),
                email: "bob@example.com".into(),
            },
            text: "Split the API first".into(),
            at: 5,
            state: SuggestionState::Pending,
            decided_at: None,
        }
    }

    #[test]
    fn deleting_a_workspace_deletes_the_events_of_its_chats_and_only_those() {
        let store = Store::open_in_memory().unwrap();
        store
            .with(|c| {
                c.execute_batch(
                    "INSERT INTO workspaces(id,slug,path,branch,state,created_at) VALUES ('gone','gone','/tmp/gone','','creating','old'), ('kept','kept','/tmp/kept','','active','old');
                     INSERT INTO chats(id,workspace_id,agent,permission_mode,status,created_at) VALUES ('c-gone','gone','claude','auto','idle','old'), ('c-kept','kept','claude','auto','idle','old');",
                )
            })
            .unwrap();
        for (workspace, chat) in [("gone", "c-gone"), ("kept", "c-kept")] {
            store
                .append_event("t", Some(workspace), Some(chat), false, "{}")
                .unwrap();
        }
        // Why the creation failed: its followers read it after the workspace is gone.
        store
            .append_event("t", Some("gone"), None, false, "{}")
            .unwrap();
        store.delete_workspace("gone").unwrap();
        let events_of = |chat: &str| -> i64 {
            store
                .with(|c| {
                    c.query_row(
                        "SELECT COUNT(*) FROM events WHERE chat_id = ?1",
                        params![chat],
                        |row| row.get(0),
                    )
                })
                .unwrap()
        };
        assert_eq!(events_of("c-gone"), 0);
        assert_eq!(events_of("c-kept"), 1);
        let workspace_events: i64 = store
            .with(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM events WHERE workspace_id = 'gone' AND chat_id IS NULL",
                    [],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(workspace_events, 1);
    }

    #[test]
    fn a_suggestion_is_kept_once_and_decided_once() {
        let store = Store::open_in_memory().unwrap();
        assert!(
            store
                .suggestion_insert("w", &suggestion("s1", "c1"))
                .unwrap()
        );
        assert!(
            store
                .suggestion_insert("w", &suggestion("s2", "c2"))
                .unwrap()
        );
        assert!(
            store
                .suggestion_decide("s1", SuggestionState::Sent, 9)
                .unwrap()
        );
        // Told again by the service after a reconnection: the decision stays.
        assert!(
            !store
                .suggestion_insert("w", &suggestion("s1", "c1"))
                .unwrap()
        );
        assert!(
            !store
                .suggestion_decide("s1", SuggestionState::Dismissed, 10)
                .unwrap()
        );
        let kept = store.suggestion("s1").unwrap().unwrap();
        assert_eq!(
            (kept.state, kept.decided_at),
            (SuggestionState::Sent, Some(9))
        );
        assert_eq!(store.suggestions_of_chat("c2").unwrap().len(), 1);
        let ids: Vec<String> = store
            .suggestions_of_workspace("w")
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, ["s1", "s2"]);
    }

    #[test]
    fn a_guest_deletes_a_waiting_or_dismissed_suggestion_never_a_sent_one() {
        let store = Store::open_in_memory().unwrap();
        for id in ["s1", "s2", "s3"] {
            store.suggestion_insert("w", &suggestion(id, "c1")).unwrap();
        }
        store
            .suggestion_decide("s2", SuggestionState::Dismissed, 9)
            .unwrap();
        store
            .suggestion_decide("s3", SuggestionState::Sent, 9)
            .unwrap();
        assert!(store.suggestion_delete("s1").unwrap());
        assert!(store.suggestion_delete("s2").unwrap());
        assert!(!store.suggestion_delete("s3").unwrap());
        let states: Vec<SuggestionState> = store
            .suggestions_of_chat("c1")
            .unwrap()
            .into_iter()
            .map(|s| s.state)
            .collect();
        assert_eq!(
            states,
            [
                SuggestionState::Deleted,
                SuggestionState::Deleted,
                SuggestionState::Sent
            ]
        );
    }

    #[test]
    fn the_shared_chats_comments_are_replaced_whole_and_read_by_chat() {
        let store = Store::open_in_memory().unwrap();
        let comment = |id: &str, chat: &str| ShareComment {
            id: id.into(),
            chat: chat.into(),
            message: "m1".into(),
            quote: "a monolith".into(),
            author: chauffeur_protocol::client::CommentAuthor {
                name: "Bob".into(),
                email: "bob@example.com".into(),
            },
            text: "Why?".into(),
            at: 5,
            reply_to: None,
            mentions: Vec::new(),
        };
        let chats = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<Vec<_>>();
        store
            .share_comments_replace(
                "w",
                &chats(&["c1", "c2", "c4"]),
                &[
                    comment("k1", "c1"),
                    comment("k2", "c2"),
                    comment("k4", "c4"),
                ],
            )
            .unwrap();
        store
            .share_comments_replace("other", &chats(&["c3"]), &[comment("k3", "c3")])
            .unwrap();
        // c4 left the share: the service no longer sends its comments, the owner keeps them.
        store
            .share_comments_replace("w", &chats(&["c1", "c2"]), &[comment("k2", "c2")])
            .unwrap();
        assert!(store.share_comments_of_chat("c1").unwrap().is_empty());
        assert_eq!(
            store.share_comments_of_chat("c2").unwrap(),
            [comment("k2", "c2")]
        );
        assert_eq!(store.share_comments_of_chat("c3").unwrap().len(), 1);
        assert_eq!(
            store.share_comments_of_chat("c4").unwrap(),
            [comment("k4", "c4")]
        );
    }

    #[test]
    fn a_new_workspace_share_takes_the_place_of_a_stopped_one() {
        use chauffeur_protocol::workspace_share::{Guest, GuestRole};
        let store = Store::open_in_memory().unwrap();
        let row = WorkspaceShareRow {
            chat_ids: Some(vec!["c1".into()]),
            workspace_id: "w".into(),
            share_id: "a1".into(),
            url: "https://design.askdialog.ai/w/a1".into(),
            guests: vec![Guest {
                email: "bob@example.com".into(),
                role: GuestRole::Suggest,
            }],
            team: false,
            created_at: now(),
            stopped_at: Some(now()),
        };
        store.workspace_share_put(&row).unwrap();
        assert!(store.workspace_shares_live().unwrap().is_empty());
        let again = WorkspaceShareRow {
            share_id: "b2".into(),
            stopped_at: None,
            team: true,
            ..row
        };
        store.workspace_share_put(&again).unwrap();
        assert_eq!(store.workspace_share("w").unwrap(), Some(again.clone()));
        assert_eq!(store.workspace_shares_live().unwrap(), vec![again]);
    }

    #[test]
    fn an_event_page_stops_before_loading_a_payload_past_its_byte_budget() {
        let store = Store::open_in_memory().unwrap();
        for text in ["a".repeat(10), "b".repeat(10), "c".repeat(10)] {
            store
                .append_event("t", Some("w"), Some("chat"), false, &text)
                .unwrap();
        }
        store
            .append_event("t", Some("w"), Some("other"), false, "x")
            .unwrap();
        let page = store.chat_event_page("chat", 0, 100, 25).unwrap();
        let loaded: Vec<_> = page
            .iter()
            .map(|(_, size, p)| (*size, p.is_some()))
            .collect();
        assert_eq!(loaded, vec![(10, true), (10, true), (10, false)]);
        let all = store.chat_event_page("chat", page[0].0, 100, 1000).unwrap();
        assert_eq!(all.len(), 2, "the page starts after the given sequence");
    }

    #[test]
    fn news_read_once_stays_read_and_is_forgotten_after_ninety_days() {
        let store = Store::open_in_memory().unwrap();
        store
            .mark_news_read(&["hn:1".into(), "hn:2".into()])
            .unwrap();
        store.mark_news_read(&["hn:1".into()]).unwrap();
        let old = (chrono::Utc::now() - chrono::Duration::days(91)).to_rfc3339();
        store
            .with(|c| c.execute("UPDATE news_read SET read_at = ?1 WHERE id = 'hn:2'", [old]))
            .unwrap();
        store.mark_news_read(&["hn:3".into()]).unwrap();
        let mut read: Vec<_> = store.news_read().unwrap().into_iter().collect();
        read.sort();
        assert_eq!(read, ["hn:1", "hn:3"]);
    }

    fn chat_event(store: &Store, event: ChatEvent) -> i64 {
        let envelope = EventEnvelope {
            seq: None,
            ts: now(),
            event: Event::Chat {
                workspace_id: "w".into(),
                chat_id: "c".into(),
                event,
            },
        };
        let payload = serde_json::to_string(&envelope).unwrap();
        store
            .append_event(&envelope.ts, Some("w"), Some("c"), false, &payload)
            .unwrap()
    }

    #[test]
    fn events_of_a_type_are_those_of_that_type_not_those_quoting_it() {
        let store = Store::open_in_memory().unwrap();
        let event = |value: serde_json::Value| serde_json::from_value::<ChatEvent>(value).unwrap();
        let ended = chat_event(
            &store,
            event(serde_json::json!({"type": "turn_ended", "outcome": "success"})),
        );
        chat_event(
            &store,
            event(serde_json::json!({
                "type": "user_message",
                "text": r#"why does the log say "type":"turn_ended"?"#,
                "queued": false,
            })),
        );
        let seqs = |events: Vec<EventEnvelope>| -> Vec<i64> {
            events.iter().map(|e| e.seq.unwrap()).collect()
        };
        assert_eq!(
            seqs(store.last_events_of_type("c", "turn_ended", 10).unwrap()),
            vec![ended]
        );
        assert_eq!(
            seqs(store.events_of_type_after("c", "turn_ended", 0).unwrap()),
            vec![ended]
        );
        assert_eq!(
            store.turn_end("c", 1).unwrap().map(|(seq, ..)| seq),
            Some(ended)
        );
    }

    #[test]
    fn reading_a_type_uses_the_index_not_every_payload_of_the_chat() {
        let store = Store::open_in_memory().unwrap();
        let plan: Vec<String> = store
            .with(|c| {
                c.prepare(&format!(
                    "EXPLAIN QUERY PLAN SELECT seq, payload FROM events \
                     WHERE chat_id = ?1 AND {EVENT_TYPE} = ?2 ORDER BY seq DESC LIMIT 1"
                ))?
                .query_map(params!["c", "turn_ended"], |r| r.get(3))?
                .collect()
            })
            .unwrap();
        assert!(
            plan.iter().any(|step| step.contains("events_by_chat_type")),
            "{plan:?}"
        );
    }

    #[test]
    fn a_replay_on_disk_reads_what_was_written_just_before() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("chauffeur.db")).unwrap();
        let first = chat_event(&store, ChatEvent::TurnStarted);
        let filter = EventFilter {
            workspaces: vec!["w".into()],
            ..Default::default()
        };
        assert_eq!(store.events_after(0, &filter).unwrap().len(), 1);
        let second = chat_event(&store, ChatEvent::TurnStarted);
        let seqs: Vec<i64> = store
            .events_after(0, &filter)
            .unwrap()
            .iter()
            .map(|e| e.seq.unwrap())
            .collect();
        assert_eq!(seqs, vec![first, second]);
    }

    #[test]
    fn a_long_read_does_not_hold_up_writes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("chauffeur.db")).unwrap();
        let reading = store.reader.lock().unwrap();
        // The writer's connection is free while the reader's is taken.
        chat_event(&store, ChatEvent::TurnStarted);
        drop(reading);
    }

    #[test]
    fn a_workspaces_tool_calls_naming_a_file_come_newest_first() {
        use chauffeur_protocol::events::{ChatEvent, Event, EventEnvelope};
        let store = Store::open_in_memory().unwrap();
        let append = |workspace: &str, chat: &str, event: ChatEvent| {
            let envelope = EventEnvelope {
                seq: None,
                ts: "2026-11-09T00:00:00Z".into(),
                event: Event::Chat {
                    workspace_id: workspace.into(),
                    chat_id: chat.into(),
                    event,
                },
            };
            store
                .append_event(
                    &envelope.ts,
                    Some(workspace),
                    Some(chat),
                    false,
                    &serde_json::to_string(&envelope).unwrap(),
                )
                .unwrap()
        };
        let write = |path: &str| ChatEvent::ToolCall {
            tool_use_id: "t".into(),
            name: "Write".into(),
            input: serde_json::json!({ "file_path": path }),
            parent_tool_use_id: None,
        };
        let first = append("w", "a", write("/w/.chauffeur/plans/plan_1.md"));
        append("other", "b", write("/other/.chauffeur/plans/plan_1.md"));
        append(
            "w",
            "a",
            ChatEvent::AssistantText {
                message_id: "m".into(),
                text: "I wrote plan_1.md".into(),
                parent_tool_use_id: None,
            },
        );
        append("w", "a", write("/w/.chauffeur/plans/planX1.md"));
        let second = append("w", "c", write("/w/.chauffeur/plans/plan_1.md"));
        let seqs: Vec<i64> = store
            .workspace_tool_calls_mentioning("w", "plan_1.md", 10)
            .unwrap()
            .iter()
            .map(|e| e.seq.unwrap())
            .collect();
        assert_eq!(seqs, vec![second, first]);
    }
}
