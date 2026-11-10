//! Design shares (story 51): what a daemon and the share service say to each other, what the
//! service says to a reviewer's page, and the HTTP bodies between them.
//!
//! Only a design, or one markdown file of a workspace, crosses the network: its files, its
//! comments, who looks at it. Never the chat.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::client::{CanvasResult, CommentAuthor, CommentPin, DesignComment, Mention};

/// Bumped on any breaking change between daemons, the share service and the reviewer page.
pub const SHARE_PROTOCOL_VERSION: u32 = 1;

/// The script the service (and the daemon) append to a screen served with `?capture=1`: it
/// answers the page, over postMessage, with the element under a point and whether earlier pins'
/// selectors still match.
pub const DESIGN_CAPTURE_SCRIPT: &str = include_str!("design_capture.js");

/// A share as its owner's daemon sees it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ShareStatus {
    pub id: String,
    /// Where a reviewer opens it.
    pub url: String,
    pub title: String,
    pub owner: CommentAuthor,
    /// The e-mails named on it, lowercase.
    pub reviewers: Vec<String>,
    /// Who a comment may mention: the owner first, then each reviewer with the name the service
    /// knows (their Google name once they signed in, else their e-mail's first name).
    #[serde(default)]
    pub people: Vec<CommentAuthor>,
    /// Open to its team domain too (its owner's Google Workspace domain), and listed in their
    /// Office's Design window.
    #[serde(default)]
    pub team: bool,
    /// The reviewers looking at it now.
    pub viewers: Vec<String>,
    /// Whether the owner's daemon is connected.
    pub online: bool,
    /// When the daemon last pushed a file or the comments, milliseconds since the epoch.
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

/// `POST /v1/shares`, and `PATCH /v1/shares/{id}` with the fields to change.
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default)]
#[ts(export)]
#[ts(optional_fields)]
pub struct ShareParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewers: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<bool>,
    /// On creation only: the share holds one markdown file of a workspace (story 57), by this
    /// name, instead of a design.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<String>,
}

/// A live share someone else opened to the user, as `GET /v1/shares` lists it: named on it, or
/// open to the team. Never its reviewers.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct SharedDesign {
    pub id: String,
    pub url: String,
    pub title: String,
    pub owner: CommentAuthor,
    /// The user is named on it.
    pub named: bool,
    /// It is open to the team.
    pub team: bool,
    /// Whether the owner's daemon is connected.
    pub online: bool,
    #[ts(type = "number")]
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub last_push_at: Option<u64>,
    /// A markdown file's share (story 57): the file's name. None: a design.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub document: Option<String>,
}

/// A file of the share as the page lists it: the design folder's, relative, with when it changed.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ShareFile {
    pub path: String,
    #[ts(type = "number")]
    pub modified_ms: u64,
}

/// What a reviewer's page reads first (`GET /v1/shares/{id}/snapshot`), and again on reconnection.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ShareSnapshot {
    pub id: String,
    pub title: String,
    pub owner: CommentAuthor,
    /// The reviewer reading it, as their comments are signed.
    pub me: CommentAuthor,
    /// The design laid out, each screen's URL relative to the page's origin; empty on a
    /// markdown file's share.
    pub canvas: CanvasResult,
    /// A markdown file's share (story 57): the file's name among `files`, which the page renders
    /// and whose passages take comments. None: a design.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub document: Option<String>,
    /// The e-mails named on the share, for its owner only (a reviewer gets none).
    #[serde(default)]
    pub reviewers: Vec<String>,
    /// The owner and the invitees, names and e-mails, for every reader: who a comment may
    /// mention, and the page's "Shared with".
    #[serde(default)]
    pub people: Vec<CommentAuthor>,
    /// The share's team domain, when it is open to one: an address of it may be mentioned too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub team_domain: Option<String>,
    /// The comments that mention the reader and that they have not opened yet.
    #[serde(default)]
    pub unread_mentions: Vec<String>,
    pub files: Vec<ShareFile>,
    pub comments: Vec<DesignComment>,
    pub viewers: Vec<String>,
    pub online: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub last_push_at: Option<u64>,
}

/// `POST /v1/session`: a reviewer's Google ID token, for the cookie the page then carries.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct ShareSessionParams {
    pub id_token: String,
}

/// `GET /v1/shares/{id}/about` and `/v1/workspace-shares/{id}/about`: what the page says before
/// anyone signs in ("Alice shared a design with you"). Nothing else of the share.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct ShareAbout {
    /// The owner's name.
    pub owner: String,
}

/// `POST /v1/shares/{id}/sign-in-link` and `/v1/workspace-shares/{id}/sign-in-link`: an address
/// that asks for a sign-in link to the share. The answer is the same whether it was invited or
/// not.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct SignInLinkParams {
    pub email: String,
}

/// `POST /v1/session/link`: the token of a sign-in link, for the cookie the page then carries.
#[derive(Serialize, Deserialize, TS, Clone, Debug)]
#[ts(export)]
pub struct SignInLinkSessionParams {
    pub token: String,
}

/// What a daemon says on its WebSocket (`/v1/shares/{id}/daemon`).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum FromDaemon {
    /// First, within seconds: the owner's Google ID token.
    Hello { id_token: String, version: u32 },
    /// The whole of `comments.json`, after each change of the daemon's.
    Comments { comments: Vec<DesignComment> },
    /// Files uploaded or removed (over HTTP) since the last word: the page reloads them.
    Changed {
        changed: Vec<String>,
        removed: Vec<String>,
    },
}

/// What the service says to a daemon.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ToDaemon {
    Welcome {
        share: ShareStatus,
    },
    /// A reviewer's comment, new or changed: the daemon keeps it in `comments.json` by its id.
    Comment {
        comment: DesignComment,
    },
    CommentRemoved {
        id: String,
    },
    Viewers {
        viewers: Vec<String>,
    },
    /// The owner changed the reviewers from their page.
    Reviewers {
        reviewers: Vec<String>,
    },
    Refused {
        reason: String,
    },
}

/// What a reviewer's page says on its WebSocket (`/v1/shares/{id}/reviewer`).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum FromReviewer {
    Add {
        screen: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pin: Option<CommentPin>,
        /// On a markdown file's share: the passage commented on.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        quote: Option<String>,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_to: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[ts(as = "Option<Vec<Mention>>", optional)]
        mentions: Vec<Mention>,
    },
    Edit {
        id: String,
        text: String,
        /// The people the new text names; absent keeps the comment's.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        mentions: Option<Vec<Mention>>,
    },
    Delete {
        id: String,
    },
    Resolve {
        id: String,
        resolved: bool,
    },
    /// The reader opened a thread: their mentions in it (its first comment and its replies) are
    /// read.
    Seen {
        thread: String,
    },
}

/// What the service says to a reviewer's page.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ToReviewer {
    Snapshot {
        snapshot: ShareSnapshot,
    },
    Files {
        changed: Vec<String>,
        removed: Vec<String>,
    },
    Comment {
        comment: DesignComment,
    },
    CommentRemoved {
        id: String,
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
    /// The reviewers changed (from the app or the owner's page): to the owner's pages only.
    Reviewers {
        reviewers: Vec<String>,
    },
    /// Who the share is open to changed, or a name became known: to every page.
    People {
        people: Vec<CommentAuthor>,
    },
    /// One message of the reviewer's refused (someone else's comment, one sent already): the
    /// page tells them and goes on.
    Error {
        reason: String,
    },
    /// The share was stopped, or the reviewer is no longer named: the page says so.
    Refused {
        reason: String,
    },
}

/// Where a mention was made.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum MentionPlace {
    Design,
    Workspace,
}

/// A mention of a user in a comment of a share open to them, as the share service keeps it for
/// them (docs/adr/mentions-in-share-comments.md).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct Mentioned {
    /// The service's, stable.
    pub id: String,
    pub place: MentionPlace,
    pub share_id: String,
    pub share_title: String,
    pub owner: CommentAuthor,
    /// The share's page opened on the thread (`…/s/<id>?thread=<thread>`, or
    /// `…/w/<id>?chat=<chat>&comment=<thread>`).
    pub url: String,
    /// The workspace's chat the thread is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub chat: Option<String>,
    /// The comment that mentions the user.
    pub comment: String,
    /// Its thread's first comment (the comment itself when it is one).
    pub thread: String,
    /// The design's screen the thread is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub screen: Option<String>,
    pub author: CommentAuthor,
    /// The comment's text, cut short.
    pub text: String,
    #[ts(type = "number")]
    pub at: u64,
    /// The user opened its thread (on the page, or from the app).
    pub seen: bool,
}

/// What a signed-in daemon says on its user's WebSocket (`/v1/mentions/daemon`).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum FromMentionsDaemon {
    /// First, within seconds: the user's Google ID token.
    Hello { id_token: String, version: u32 },
    /// The user opened a mention's thread from the app.
    Seen { id: String },
}

/// What the service says to a daemon on its user's WebSocket.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ToMentionsDaemon {
    /// The user's mentions, whole, newest first (the unread ones and the latest read ones, 50 at
    /// most): at connection and after each change.
    Mentions {
        mentions: Vec<Mentioned>,
    },
    Refused {
        reason: String,
    },
}
