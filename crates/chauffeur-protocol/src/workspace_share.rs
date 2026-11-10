//! Workspace shares (story 53): what a daemon and the share service say to each other about a
//! shared workspace, what the service says to a guest's page, and the HTTP bodies between them.
//!
//! What crosses the network: the workspace's chats (their list and their events, trimmed by the
//! daemon), the markdown of its notes folder, the guests' suggestions and who is looking. Never
//! the terminals, the processes, the diff or the repos' files.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::client::{CommentAuthor, Mention};
use crate::events::EventEnvelope;
use crate::model::{AgentKind, ChatKind, ChatStatus};

/// Bumped on any breaking change between daemons, the share service and the guest page.
pub const WORKSPACE_SHARE_PROTOCOL_VERSION: u32 = 1;
/// Bumped on an addition an older daemon cannot read, which the service keeps from a daemon that
/// says less in its `Hello`. 1: suggestions deleted by their authors, guests' comments. 2: replies
/// to a comment, the owner's comments, mentions in comments.
pub const WORKSPACE_SHARE_PROTOCOL_REVISION: u32 = 2;

/// What a guest may do besides looking.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum GuestRole {
    /// Reads the chats and the notes.
    #[default]
    Viewer,
    /// Also suggests messages to the chats' agents, which the owner sends or not.
    Suggest,
}

/// A colleague named on a workspace share.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[ts(export)]
pub struct Guest {
    /// Lowercase.
    pub email: String,
    pub role: GuestRole,
}

/// A workspace share as its owner's daemon sees it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct WorkspaceShareStatus {
    pub id: String,
    /// Where a guest opens it: `…/w/<id>`.
    pub url: String,
    pub title: String,
    pub owner: CommentAuthor,
    pub guests: Vec<Guest>,
    /// Who a comment may mention: the owner first, then each guest with the name the service
    /// knows (their Google name once they signed in, else their e-mail's first name).
    #[serde(default)]
    pub people: Vec<CommentAuthor>,
    /// Open to its team domain too (its owner's Google Workspace domain), each account of it a
    /// viewer unless named with another role.
    #[serde(default)]
    pub team: bool,
    /// The guests looking at it now.
    pub viewers: Vec<String>,
    /// Whether the owner's daemon is connected.
    pub online: bool,
    /// When the daemon last pushed something, milliseconds since the epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub last_push_at: Option<u64>,
    /// Stopped by its owner: the page says so and serves nothing.
    pub stopped: bool,
    /// What the service held back for this request, in a sentence for the owner: invitations
    /// not e-mailed (a limit, an address that stopped Chauffeur's e-mails). The share is saved
    /// all the same.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub notice: Option<String>,
}

/// `POST /v1/workspace-shares`, and `PATCH /v1/workspace-shares/{id}` with the fields to change.
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct WorkspaceShareParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guests: Option<Vec<Guest>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<bool>,
}

/// A chat of a shared workspace, as guests see it in the list: nothing of its agent's settings.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct SharedChat {
    pub id: String,
    pub title: Option<String>,
    pub kind: ChatKind,
    pub agent: AgentKind,
    pub status: ChatStatus,
    /// RFC 3339.
    pub created_at: String,
}

/// A markdown file of the workspace's notes folder, by its path in that folder.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct SharedNote {
    pub path: String,
    pub text: String,
    #[ts(type = "number")]
    pub modified_ms: u64,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SuggestionState {
    /// Waiting for the owner.
    Pending,
    /// Sent to the agent as written.
    Sent,
    /// Sent after the owner changed it in the composer.
    SentEdited,
    Dismissed,
    /// Taken back by its author while it waited or after it was dismissed: shown to nobody.
    Deleted,
}

/// A message a guest suggests to a chat's agent; the owner sends it, edits it or dismisses it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct Suggestion {
    pub id: String,
    pub chat: String,
    pub author: CommentAuthor,
    pub text: String,
    /// Milliseconds since the epoch.
    #[ts(type = "number")]
    pub at: u64,
    pub state: SuggestionState,
    /// When the owner sent or dismissed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub decided_at: Option<u64>,
}

/// A remark on a passage of an agent's message, by the owner or a guest who can suggest, for them
/// all: the agent never reads it. It opens a thread, or replies to one. Its author deletes it (a
/// thread's first comment with its replies); nobody edits it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ShareComment {
    pub id: String,
    pub chat: String,
    /// The `message_id` of the agent's message it is on (its thread's, for a reply).
    pub message: String,
    /// The passage selected; empty on a reply.
    pub quote: String,
    pub author: CommentAuthor,
    pub text: String,
    /// Milliseconds since the epoch.
    #[ts(type = "number")]
    pub at: u64,
    /// The first comment of the thread it replies to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reply_to: Option<String>,
    /// The people its text names with `@`, each once (docs/adr/mentions-in-share-comments.md).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<Mention>>", optional)]
    pub mentions: Vec<Mention>,
}

/// A new comment, from a guest's page or from the owner's daemon
/// (`POST /v1/workspace-shares/{id}/comments`): the service gives it its id, author and time.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Default)]
#[ts(export)]
pub struct NewShareComment {
    pub chat: String,
    /// The agent's message it is on; a reply takes its thread's.
    #[serde(default)]
    pub message: String,
    /// The passage selected; none on a reply.
    #[serde(default)]
    pub quote: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<Mention>>", optional)]
    pub mentions: Vec<Mention>,
}

/// What a guest's page reads first (`GET /v1/workspace-shares/{id}/snapshot`), and again on
/// reconnection. A chat's events come apart (`…/chats/{chat}/events`): they can be long.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct WorkspaceShareSnapshot {
    pub id: String,
    pub title: String,
    pub owner: CommentAuthor,
    /// The guest reading it, as their suggestions are signed.
    pub me: CommentAuthor,
    /// What the reader may do: the owner reads as a viewer (they send from the app).
    pub role: GuestRole,
    pub chats: Vec<SharedChat>,
    pub notes: Vec<SharedNote>,
    pub suggestions: Vec<Suggestion>,
    #[serde(default)]
    pub comments: Vec<ShareComment>,
    /// The owner and the guests, names and e-mails, for every reader: who a comment may mention,
    /// and the page's "Shared with".
    #[serde(default)]
    pub people: Vec<CommentAuthor>,
    /// The share's team domain, when it is open to one: an address of it may be mentioned too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub team_domain: Option<String>,
    /// The comments that mention the reader and whose thread they have not opened yet.
    #[serde(default)]
    pub unread_mentions: Vec<String>,
    pub viewers: Vec<String>,
    pub online: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub last_push_at: Option<u64>,
}

/// What a daemon says on its WebSocket (`/v1/workspace-shares/{id}/daemon`).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum FromWorkspaceDaemon {
    /// First, within seconds: the owner's Google ID token.
    Hello {
        id_token: String,
        version: u32,
        /// `WORKSPACE_SHARE_PROTOCOL_REVISION`; absent from daemons older than revisions.
        #[serde(default)]
        revision: u32,
    },
    /// The workspace's open chats, whole, after each change: a chat no longer listed is gone,
    /// its events with it.
    Chats { chats: Vec<SharedChat> },
    /// A chat's stored events, whole: they take the place of what the service kept of it.
    History {
        chat: String,
        events: Vec<EventEnvelope>,
    },
    /// What happened in a chat since: those with a `seq` are kept, the others (text being
    /// written) only passed on to the pages looking.
    Events {
        chat: String,
        events: Vec<EventEnvelope>,
    },
    /// Notes written or removed since the last word; all of them on a first connection.
    Notes {
        changed: Vec<SharedNote>,
        removed: Vec<String>,
    },
    /// The suggestions, whole, after each change of the owner's (sent, dismissed).
    Suggestions { suggestions: Vec<Suggestion> },
}

/// What the service says to a workspace's daemon.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ToWorkspaceDaemon {
    /// With the suggestions the service holds: those made while the daemon was away are among
    /// them, for it to keep.
    Welcome {
        share: WorkspaceShareStatus,
        suggestions: Vec<Suggestion>,
        #[serde(default)]
        comments: Vec<ShareComment>,
    },
    /// A guest's new suggestion, or one its author deleted.
    Suggestion {
        suggestion: Suggestion,
    },
    /// The comments (the guests' and the owner's), whole, after each change: the service is
    /// where they are written.
    Comments {
        comments: Vec<ShareComment>,
    },
    Viewers {
        viewers: Vec<String>,
    },
    Refused {
        reason: String,
    },
}

/// What a guest's page says on its WebSocket (`/v1/workspace-shares/{id}/guest`).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum FromGuest {
    /// A guest who can suggest: a message for a chat's agent, to its owner.
    Suggest { chat: String, text: String },
    /// Its author takes a suggestion back, unless it was sent.
    DeleteSuggestion { id: String },
    /// A guest who can suggest: a remark on a passage of an agent's message, or a reply to one.
    Comment {
        chat: String,
        #[serde(default)]
        message: String,
        #[serde(default)]
        quote: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        reply_to: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[ts(as = "Option<Vec<Mention>>", optional)]
        mentions: Vec<Mention>,
    },
    /// Its author deletes a comment, a thread's first one with its replies.
    DeleteComment { id: String },
    /// The reader opened a thread: their mentions in it are read.
    Seen { thread: String },
}

/// What the service says to a guest's page.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ToGuest {
    Snapshot {
        // Boxed: the largest variant by far.
        snapshot: Box<WorkspaceShareSnapshot>,
    },
    Chats {
        chats: Vec<SharedChat>,
    },
    /// A chat's events were replaced: the page reads them again if it shows that chat.
    History {
        chat: String,
    },
    Events {
        chat: String,
        events: Vec<EventEnvelope>,
    },
    Notes {
        changed: Vec<SharedNote>,
        removed: Vec<String>,
    },
    Suggestions {
        suggestions: Vec<Suggestion>,
    },
    Comments {
        comments: Vec<ShareComment>,
    },
    /// Who the share is open to changed, or a name became known: to every page.
    People {
        people: Vec<CommentAuthor>,
    },
    Viewers {
        viewers: Vec<String>,
    },
    Live {
        online: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional, type = "number")]
        last_push_at: Option<u64>,
    },
    /// One message of the guest's refused (not allowed to suggest, an empty text): the page
    /// tells them and goes on.
    Error {
        reason: String,
    },
    /// The share was stopped, or the guest is no longer on it: the page says so.
    Refused {
        reason: String,
    },
}
