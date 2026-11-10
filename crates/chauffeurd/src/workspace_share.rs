//! Workspace shares (story 53): a workspace's chats and notes on the share service, live, for
//! named guests who read them and may suggest messages. The daemon creates the share over HTTPS
//! with the user's Google ID token (the design shares' sign-in, `share.rs`), then keeps one
//! WebSocket per shared workspace: the open chats, their events (trimmed: no images, no shell
//! output, short tool results, no permission's input), the notes' markdown and the suggestions'
//! fates go up; the guests' suggestions come down into the store, for the owner to send, edit or
//! dismiss, and so do those their authors deleted and the comments on the agents' messages, for
//! the owner to read. The owner's own comments and replies go to the service over HTTPS, which
//! writes them as it writes the guests'. The link runs while the share lives, through restarts,
//! and reconnects with backoff.
//!
//! Never shared: the terminals, the processes, the diff, the repos' files.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use chauffeur_protocol::client::{
    SendParams, ShareCommentAddParams, ShareCommentParams, ShareCommentsResult, ShareConnection,
    SuggestionParams, SuggestionsResult, WorkspaceSharePreview, WorkspaceShareStartParams,
    WorkspaceShareState,
};
use chauffeur_protocol::events::{ChatEvent, Event, EventEnvelope, RegistryEvent, WorkspaceEvent};
use chauffeur_protocol::model::{ChatInfo, SendMode};
use chauffeur_protocol::workspace_share::{
    FromWorkspaceDaemon, NewShareComment, ShareComment, SharedChat, SharedNote, Suggestion,
    SuggestionState, ToWorkspaceDaemon, WORKSPACE_SHARE_PROTOCOL_REVISION,
    WORKSPACE_SHARE_PROTOCOL_VERSION, WorkspaceShareParams, WorkspaceShareStatus,
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::Notify;
use tokio::sync::broadcast::error::RecvError;
use tokio_tungstenite::tungstenite::Message;

use crate::paths::{DESIGNS_DIR, NOTES_DIR};
use crate::share::{Listing, accepted, held_back, modified_ms, refusal};
use crate::store::WorkspaceShareRow;
use crate::{Daemon, Error, Result};

const MAX_BACKOFF: Duration = Duration::from_secs(60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(60);
/// How often the notes folder is looked at for changes.
const NOTES_EVERY: Duration = Duration::from_secs(3);
/// A note larger than this stays on the Mac.
const MAX_NOTE_BYTES: u64 = 1024 * 1024;
const NOTES_DEPTH: usize = 6;
/// Never read for notes: the design chats' screens and the chats' saved images.
const SKIPPED_NOTE_DIRS: [&str; 2] = [DESIGNS_DIR, crate::attachments::DIR];
/// A tool's result is cut to this many bytes for the guests: they see it folded anyway.
const TOOL_OUTPUT_MAX: usize = 8 * 1024;

/// A share's snapshot as `read_snapshot` checked it, with where and how it was read.
struct SnapshotRead {
    base: String,
    id: String,
    token: String,
    snapshot: chauffeur_protocol::workspace_share::WorkspaceShareSnapshot,
}

/// What an import of `snapshot` brings, for the creation dialog.
fn preview_of(
    snapshot: &chauffeur_protocol::workspace_share::WorkspaceShareSnapshot,
) -> WorkspaceSharePreview {
    WorkspaceSharePreview {
        title: snapshot.title.clone(),
        owner: snapshot.owner.clone(),
        chats: snapshot.chats.len() as u32,
        notes: snapshot.notes.len() as u32,
        last_push_at: snapshot.last_push_at,
    }
}

fn say(message: &FromWorkspaceDaemon) -> Message {
    Message::Text(
        serde_json::to_string(message)
            .expect("share messages serialize")
            .into(),
    )
}

fn closed(error: tokio_tungstenite::tungstenite::Error) -> String {
    format!("The share service closed: {error}")
}

pub struct SharedWorkspaceCopy {
    pub snapshot: chauffeur_protocol::workspace_share::WorkspaceShareSnapshot,
    pub histories: Vec<Vec<EventEnvelope>>,
}

impl SharedWorkspaceCopy {
    /// The share's chat and its history when it holds only one, taken out of the copy.
    pub fn take_single(&mut self) -> Option<(SharedChat, Vec<EventEnvelope>)> {
        if self.snapshot.chats.len() != 1 || self.histories.len() != 1 {
            return None;
        }
        Some((self.snapshot.chats.remove(0), self.histories.remove(0)))
    }
}

pub struct WorkspaceShares {
    daemon: Weak<Daemon>,
    http: reqwest::Client,
    links: Mutex<HashMap<String, Link>>,
}

/// One shared workspace's link, while the share lives.
struct Link {
    stop: Arc<Notify>,
    /// The suggestions changed on this side (sent, dismissed): the link pushes them.
    suggestions_changed: Arc<Notify>,
    /// The owner changed the chats shared: the link sends the guests' list again.
    chats_changed: Arc<Notify>,
    connection: ShareConnection,
    status: Option<WorkspaceShareStatus>,
}

/// A workspace this machine may share: its id, and the title guests know it by.
struct Shareable {
    id: String,
    title: String,
}

/// How a session ended.
enum Ended {
    /// The connection dropped: connect again.
    Closed,
    /// The service says the share is gone.
    Stopped,
}

impl WorkspaceShares {
    pub fn new(daemon: Weak<Daemon>) -> WorkspaceShares {
        WorkspaceShares {
            daemon,
            http: reqwest::Client::builder()
                .timeout(HTTP_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("an HTTP client"),
            links: Mutex::new(HashMap::new()),
        }
    }

    fn d(&self) -> Arc<Daemon> {
        self.daemon
            .upgrade()
            .expect("the daemon outlives its shares")
    }

    fn base_url(&self) -> Result<String> {
        self.d().shares.base_url().ok_or_else(|| {
            Error::Unavailable("Sharing is off: set design_share.url in settings.toml".into())
        })
    }

    /// What importing `link` brings, read as the import reads it, before anything is created.
    pub async fn preview(&self, link: &str) -> Result<WorkspaceSharePreview> {
        Ok(preview_of(&self.read_snapshot(link).await?.snapshot))
    }

    /// Read with the recipient's identity, and only from the configured share service.
    pub async fn read_copy(&self, link: &str) -> Result<SharedWorkspaceCopy> {
        let SnapshotRead {
            base,
            id,
            token,
            snapshot,
        } = self.read_snapshot(link).await?;
        let mut histories = Vec::new();
        let mut total = snapshot
            .notes
            .iter()
            .map(|note| note.text.len())
            .sum::<usize>();
        for chat in &snapshot.chats {
            if chat.id.is_empty()
                || !chat
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            {
                return Err(Error::BadRequest(
                    "This share contains an invalid chat id".into(),
                ));
            }
            let (history, size): (Vec<EventEnvelope>, _) = self
                .read_json(
                    &format!("{base}/v1/workspace-shares/{id}/chats/{}/events", chat.id),
                    &token,
                )
                .await?;
            total += size;
            if total > 64 * 1024 * 1024 {
                return Err(Error::BadRequest(
                    "This workspace share is too large to import".into(),
                ));
            }
            histories.push(history);
        }
        Ok(SharedWorkspaceCopy {
            snapshot,
            histories,
        })
    }

    /// The share's snapshot, its link checked against the configured service and its contents
    /// against what an import accepts.
    async fn read_snapshot(&self, link: &str) -> Result<SnapshotRead> {
        let base = self.base_url()?;
        let link_urls = self.d().settings.get().design_share.link_urls.clone();
        let id = workspace_share_id(link, std::iter::once(&base).chain(&link_urls)).ok_or_else(
            || Error::BadRequest(format!("Paste a workspace share link from {base}/w/<id>")),
        )?;
        let token = self.d().shares.token().await.map_err(Error::Unavailable)?;
        let (snapshot, _): (
            chauffeur_protocol::workspace_share::WorkspaceShareSnapshot,
            _,
        ) = self
            .read_json(&format!("{base}/v1/workspace-shares/{id}/snapshot"), &token)
            .await?;
        if snapshot.chats.is_empty() || snapshot.chats.len() > 100 {
            return Err(Error::BadRequest(
                "This share must contain between 1 and 100 chats to import".into(),
            ));
        }
        for note in &snapshot.notes {
            if !safe_note_path(&note.path) || note.text.len() > MAX_NOTE_BYTES as usize {
                return Err(Error::BadRequest(
                    "This share contains an invalid note: ask its owner to share it again".into(),
                ));
            }
        }
        Ok(SnapshotRead {
            id: id.to_string(),
            base,
            token,
            snapshot,
        })
    }

    /// The decoded body, and its size in bytes.
    async fn read_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        token: &str,
    ) -> Result<(T, usize)> {
        let mut response = self
            .http
            .get(url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| Error::Unavailable(format!("Could not read the workspace share: {e}")))?
            .error_for_status()
            .map_err(|e| Error::Unavailable(refusal(e)))?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| Error::Unavailable(e.to_string()))?
        {
            if bytes.len() + chunk.len() > 32 * 1024 * 1024 {
                return Err(Error::BadRequest(
                    "This shared conversation is too large to import".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let value = serde_json::from_slice(&bytes).map_err(|e| {
            Error::BadRequest(format!("Could not read the shared conversation: {e}"))
        })?;
        Ok((value, bytes.len()))
    }

    /// Called while a new workspace is still being created, before any agent can start.
    pub async fn copy_into(
        &self,
        workspace: &crate::store::WorkspaceRow,
        copy: SharedWorkspaceCopy,
    ) -> Result<()> {
        let notes = Path::new(&workspace.path)
            .join(NOTES_DIR)
            .join("shared-notes");
        for note in copy.snapshot.notes {
            let path = notes.join(&note.path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(anyhow::Error::from)?;
            }
            std::fs::write(path, note.text).map_err(anyhow::Error::from)?;
        }
        for (source, history) in copy.snapshot.chats.into_iter().zip(copy.histories) {
            // Design assets are not part of a workspace share: its transcript becomes a normal chat.
            let params = chauffeur_protocol::client::CreateChatParams {
                workspace: workspace.id.clone(),
                agent: Some(source.agent),
                title: source.title,
                ..Default::default()
            };
            self.import_chat(params, &history).await?;
        }
        Ok(())
    }

    /// A new chat holding a shared chat's conversation, which its agent gets on its first
    /// message.
    pub async fn import_chat(
        &self,
        params: chauffeur_protocol::client::CreateChatParams,
        history: &[EventEnvelope],
    ) -> Result<ChatInfo> {
        let d = self.d();
        let mut chat = d.chats.create(params).await?;
        let events = d.store.import_shared_history(&chat, history)?;
        chat.forked_from_agent = Some(chat.agent);
        d.events.announce(events);
        d.events
            .registry(RegistryEvent::ChatUpserted { chat: chat.clone() });
        Ok(chat)
    }

    /// The share's state as the app shows it.
    pub fn state(&self, workspace_key: &str) -> Result<WorkspaceShareState> {
        let workspace = self.d().workspaces.row(workspace_key)?;
        Ok(self.state_of(&workspace.id))
    }

    fn state_of(&self, workspace_id: &str) -> WorkspaceShareState {
        let d = self.d();
        let links = self.links.lock().unwrap();
        let link = links.get(workspace_id);
        let row = d.store.workspace_share(workspace_id).ok().flatten();
        let chat_ids = row.as_ref().and_then(|r| r.chat_ids.clone());
        let share = link.and_then(|l| l.status.clone()).or_else(|| {
            // Before the service answered (or while it is away): what the daemon knows.
            row.filter(|r| r.stopped_at.is_none())
                .map(|r| WorkspaceShareStatus {
                    id: r.share_id,
                    url: r.url,
                    title: String::new(),
                    owner: chauffeur_protocol::client::CommentAuthor {
                        name: String::new(),
                        email: d.shares.email().unwrap_or_default(),
                    },
                    guests: r.guests,
                    people: Vec::new(),
                    team: r.team,
                    viewers: Vec::new(),
                    online: false,
                    last_push_at: None,
                    stopped: false,
                    notice: None,
                })
        });
        WorkspaceShareState {
            chat_ids,
            workspace: workspace_id.to_string(),
            configured: d.shares.base_url().is_some(),
            email: d.shares.email(),
            share,
            connection: link
                .map(|l| l.connection.clone())
                .unwrap_or(ShareConnection::Off),
            past_reviewers: d.shares.past_reviewers(),
        }
    }

    fn publish(&self, workspace_id: &str) {
        let state = self.state_of(workspace_id);
        self.d().events.workspace(
            workspace_id,
            WorkspaceEvent::Share {
                state: Box::new(state),
            },
        );
    }

    fn update_link(&self, workspace_id: &str, edit: impl FnOnce(&mut Link)) {
        if let Some(link) = self.links.lock().unwrap().get_mut(workspace_id) {
            edit(link);
        }
    }

    /// Starts the links of the shares alive in the store: at the daemon's start.
    pub fn resume(self: &Arc<Self>) {
        for row in self.d().store.workspace_shares_live().unwrap_or_default() {
            self.spawn(row);
        }
    }

    fn spawn(self: &Arc<Self>, row: WorkspaceShareRow) {
        let stop = Arc::new(Notify::new());
        let suggestions_changed = Arc::new(Notify::new());
        let chats_changed = Arc::new(Notify::new());
        {
            let mut links = self.links.lock().unwrap();
            if let Some(old) = links.insert(
                row.workspace_id.clone(),
                Link {
                    stop: stop.clone(),
                    suggestions_changed: suggestions_changed.clone(),
                    chats_changed: chats_changed.clone(),
                    connection: ShareConnection::Connecting,
                    status: None,
                },
            ) {
                old.stop.notify_waiters();
            }
        }
        let shares = self.clone();
        tokio::spawn(async move {
            shares
                .run(row, stop, suggestions_changed, chats_changed)
                .await
        });
    }

    /// A workspace that moved to another machine runs there: it is shared from there.
    fn shareable(&self, workspace_key: &str) -> Result<Shareable> {
        let d = self.d();
        let row = d.workspaces.row(workspace_key)?;
        if let Some(machine) = crate::cloud_transfer_driver::moved_to(&d, &row.id) {
            return Err(Error::Conflict(format!(
                "This workspace runs on {machine} now: share it from there"
            )));
        }
        Ok(Shareable {
            id: row.id,
            title: row.title.unwrap_or(row.slug),
        })
    }

    /// The chats a share start or update asks for: `None` when it does not say, `Some(None)` for
    /// all chats, new ones too, or the open chats picked.
    fn selection(
        &self,
        workspace_id: &str,
        params: &WorkspaceShareStartParams,
    ) -> Result<Option<Option<Vec<String>>>> {
        let Some(ids) = &params.chat_ids else {
            return Ok(params.all_chats.filter(|all| *all).map(|_| None));
        };
        if params.all_chats == Some(true) {
            return Err(Error::BadRequest(
                "Share all chats or pick some, not both".into(),
            ));
        }
        let open = self.d().chats.list(workspace_id, false)?;
        if ids.is_empty() || ids.iter().any(|id| !open.iter().any(|chat| &chat.id == id)) {
            return Err(Error::BadRequest(
                "Select at least one open chat of this workspace to share".into(),
            ));
        }
        Ok(Some(Some(ids.clone())))
    }

    /// Shares a workspace: a new share on the service, a link for it.
    pub async fn start(
        self: &Arc<Self>,
        params: &WorkspaceShareStartParams,
    ) -> Result<WorkspaceShareState> {
        let d = self.d();
        let workspace = self.shareable(&params.workspace)?;
        let chat_ids = self.selection(&workspace.id, params)?.unwrap_or(None);
        let base = self.base_url()?;
        let token = d.shares.token().await.map_err(Error::Unavailable)?;
        if let Some(row) = d.store.workspace_share(&workspace.id)?
            && row.stopped_at.is_none()
        {
            return Err(Error::Conflict(
                "This workspace is shared already: change its guests, or stop the share first"
                    .into(),
            ));
        }
        let response = self
            .http
            .post(format!("{base}/v1/workspace-shares"))
            .bearer_auth(&token)
            .json(&WorkspaceShareParams {
                title: Some(workspace.title.clone()),
                guests: Some(params.guests.clone()),
                team: Some(params.team.unwrap_or(false)),
            })
            .send()
            .await
            .map_err(|e| Error::Unavailable(format!("Could not reach the share service: {e}")))?;
        let mut status: WorkspaceShareStatus = accepted(response)
            .await
            .map_err(Error::Unavailable)?
            .json()
            .await
            .map_err(|e| Error::Internal(anyhow::anyhow!("the share service's answer: {e}")))?;
        let notice = status.notice.take();
        let row = WorkspaceShareRow {
            chat_ids,
            workspace_id: workspace.id.clone(),
            share_id: status.id.clone(),
            url: status.url.clone(),
            guests: status.guests.clone(),
            team: status.team,
            created_at: crate::store::now(),
            stopped_at: None,
        };
        d.store.workspace_share_put(&row)?;
        d.shares.remember_reviewers(&guest_emails(&status));
        self.spawn(row);
        self.update_link(&workspace.id, |link| link.status = Some(status));
        self.publish(&workspace.id);
        tracing::info!(workspace = workspace.id, "workspace shared");
        held_back(notice)?;
        Ok(self.state_of(&workspace.id))
    }

    /// The guests of a live share and their roles, and whether the team sees it.
    pub async fn update(&self, params: &WorkspaceShareStartParams) -> Result<WorkspaceShareState> {
        let d = self.d();
        let workspace = self.shareable(&params.workspace)?;
        let row = d
            .store
            .workspace_share(&workspace.id)?
            .filter(|r| r.stopped_at.is_none())
            .ok_or_else(|| Error::NotFound("This workspace is not shared".into()))?;
        let chat_ids = self
            .selection(&workspace.id, params)?
            .unwrap_or_else(|| row.chat_ids.clone());
        let base = self.base_url()?;
        let token = d.shares.token().await.map_err(Error::Unavailable)?;
        let response = self
            .http
            .patch(format!("{base}/v1/workspace-shares/{}", row.share_id))
            .bearer_auth(&token)
            .json(&WorkspaceShareParams {
                title: None,
                guests: Some(params.guests.clone()),
                team: params.team,
            })
            .send()
            .await
            .map_err(|e| Error::Unavailable(format!("Could not reach the share service: {e}")))?;
        let mut status: WorkspaceShareStatus = accepted(response)
            .await
            .map_err(Error::Unavailable)?
            .json()
            .await
            .map_err(|e| Error::Internal(anyhow::anyhow!("the share service's answer: {e}")))?;
        let notice = status.notice.take();
        let reselected = chat_ids != row.chat_ids;
        d.store.workspace_share_put(&WorkspaceShareRow {
            guests: status.guests.clone(),
            team: status.team,
            chat_ids,
            ..row
        })?;
        d.shares.remember_reviewers(&guest_emails(&status));
        self.update_link(&workspace.id, |link| {
            link.status = Some(status);
            if reselected {
                link.chats_changed.notify_one();
            }
        });
        self.publish(&workspace.id);
        held_back(notice)?;
        Ok(self.state_of(&workspace.id))
    }

    /// Stops a share: the service drops its copy, the link ends; the suggestions stay local.
    pub async fn stop(&self, workspace_key: &str) -> Result<WorkspaceShareState> {
        let d = self.d();
        let workspace = d.workspaces.row(workspace_key)?;
        let row = d
            .store
            .workspace_share(&workspace.id)?
            .filter(|r| r.stopped_at.is_none())
            .ok_or_else(|| Error::NotFound("This workspace is not shared".into()))?;
        self.end(&row).await;
        Ok(self.state_of(&workspace.id))
    }

    async fn end(&self, row: &WorkspaceShareRow) {
        let d = self.d();
        if let (Some(base), Ok(token)) = (d.shares.base_url(), d.shares.token().await) {
            let deleted = self
                .http
                .delete(format!("{base}/v1/workspace-shares/{}", row.share_id))
                .bearer_auth(&token)
                .send()
                .await;
            if let Err(error) = deleted {
                tracing::warn!(%error, workspace = row.workspace_id, "the share service did not take the stop; the link ends anyway");
            }
        }
        if let Some(link) = self.links.lock().unwrap().remove(&row.workspace_id) {
            link.stop.notify_waiters();
        }
        let _ = d.store.workspace_share_put(&WorkspaceShareRow {
            stopped_at: Some(crate::store::now()),
            ..row.clone()
        });
        self.publish(&row.workspace_id);
        tracing::info!(workspace = row.workspace_id, "workspace share stopped");
    }

    /// A workspace archived: its share stops (best effort, in the background).
    pub fn stop_for_workspace(self: &Arc<Self>, workspace_id: &str) {
        let Ok(Some(row)) = self.d().store.workspace_share(workspace_id) else {
            return;
        };
        if row.stopped_at.is_some() {
            return;
        }
        let shares = self.clone();
        tokio::spawn(async move { shares.end(&row).await });
    }

    // --- suggestions ---

    pub fn suggestions(&self, chat_key: &str) -> Result<SuggestionsResult> {
        let d = self.d();
        let chat = d.chats.resolve(chat_key)?;
        Ok(SuggestionsResult {
            suggestions: d.store.suggestions_of_chat(&chat.id)?,
        })
    }

    /// A pending suggestion of the chat's, or why not.
    fn pending(&self, chat: &ChatInfo, id: &str) -> Result<Suggestion> {
        let suggestion = self
            .d()
            .store
            .suggestion(id)?
            .filter(|s| s.chat == chat.id)
            .ok_or_else(|| Error::not_found(format!("suggestion {id} of this chat")))?;
        if suggestion.state != SuggestionState::Pending {
            return Err(Error::Conflict(format!(
                "{}'s suggestion was already {}",
                suggestion.author.name,
                match suggestion.state {
                    SuggestionState::Dismissed => "dismissed",
                    SuggestionState::Deleted => "deleted by its author",
                    _ => "sent",
                }
            )));
        }
        Ok(suggestion)
    }

    pub fn dismiss(&self, params: &SuggestionParams) -> Result<SuggestionsResult> {
        let d = self.d();
        let chat = d.chats.resolve(&params.chat)?;
        self.pending(&chat, &params.id)?;
        if !d
            .store
            .suggestion_decide(&params.id, SuggestionState::Dismissed, now_ms())?
        {
            return Err(Error::Conflict(
                "This suggestion was decided meanwhile".into(),
            ));
        }
        self.suggestions_changed(&chat);
        self.suggestions(&chat.id)
    }

    /// Sends a guest's suggestion as the user's message, with its author shown to people only:
    /// marked sent, or sent edited when the user changed its text. It is marked before it goes,
    /// so that two clicks never send it twice, and put back when the sending fails.
    pub async fn send(&self, params: &SendParams, id: &str) -> Result<ChatInfo> {
        let d = self.d();
        let chat = d.chats.resolve(&params.chat)?;
        let suggestion = self.pending(&chat, id)?;
        let state = if suggestion.text.trim() == params.text.trim() {
            SuggestionState::Sent
        } else {
            SuggestionState::SentEdited
        };
        if !d.store.suggestion_decide(id, state, now_ms())? {
            return Err(Error::Conflict(
                "This suggestion was decided meanwhile".into(),
            ));
        }
        let sent = d
            .chats
            .send_from(
                &chat.id,
                &params.text,
                params.images.as_deref().unwrap_or_default(),
                SendMode::Steer,
                crate::chats::Sender::Guest(suggestion.author.clone()),
            )
            .await;
        if sent.is_err() {
            d.store.suggestion_reopen(id)?;
        }
        self.suggestions_changed(&chat);
        sent
    }

    fn suggestions_changed(&self, chat: &ChatInfo) {
        let d = self.d();
        let suggestions = d.store.suggestions_of_chat(&chat.id).unwrap_or_default();
        d.events.chat(
            &chat.workspace_id,
            &chat.id,
            ChatEvent::Suggestions { suggestions },
        );
        if let Some(link) = self.links.lock().unwrap().get(&chat.workspace_id) {
            link.suggestions_changed.notify_one();
        }
    }

    /// Suggestions from the service (one new or deleted, or those of a Welcome): those the store
    /// lacks are kept, those their authors deleted leave the owner's cards, and their chats are
    /// told.
    fn receive(&self, workspace_id: &str, suggestions: &[Suggestion]) {
        let d = self.d();
        let mut chats = HashSet::new();
        for suggestion in suggestions {
            // Only the workspace's own chats take suggestions.
            let Ok(Some(chat)) = d.store.chat(&suggestion.chat) else {
                continue;
            };
            if chat.workspace_id != workspace_id
                || !self
                    .shared_chats(workspace_id)
                    .iter()
                    .any(|c| c.id == chat.id)
            {
                continue;
            }
            let kept = d
                .store
                .suggestion_insert(workspace_id, suggestion)
                .and_then(|new| {
                    if !new && suggestion.state == SuggestionState::Deleted {
                        d.store.suggestion_delete(&suggestion.id)
                    } else {
                        Ok(new)
                    }
                });
            match kept {
                Ok(true) => {
                    chats.insert(chat.id);
                }
                Ok(false) => {}
                Err(error) => tracing::warn!(%error, "a guest's suggestion could not be kept"),
            }
        }
        for chat_id in chats {
            let suggestions = d.store.suggestions_of_chat(&chat_id).unwrap_or_default();
            d.events.chat(
                workspace_id,
                &chat_id,
                ChatEvent::Suggestions { suggestions },
            );
        }
    }

    // --- comments ---

    pub fn comments(&self, chat_key: &str) -> Result<ShareCommentsResult> {
        let d = self.d();
        let chat = d.chats.resolve(chat_key)?;
        Ok(ShareCommentsResult {
            comments: d.store.share_comments_of_chat(&chat.id)?,
        })
    }

    /// The live share a chat is in, or why its owner cannot comment there.
    fn share_of_chat(&self, chat: &ChatInfo) -> Result<WorkspaceShareRow> {
        let row = self
            .d()
            .store
            .workspace_share(&chat.workspace_id)?
            .filter(|r| r.stopped_at.is_none())
            .ok_or_else(|| {
                Error::Conflict(
                    "This workspace is not shared: share it to comment for its guests".into(),
                )
            })?;
        if !self
            .shared_chats(&chat.workspace_id)
            .iter()
            .any(|c| c.id == chat.id)
        {
            return Err(Error::Conflict(
                "This chat is not in the workspace's share: add it to the share to comment on it"
                    .into(),
            ));
        }
        Ok(row)
    }

    /// The owner comments on a passage of a shared chat's agent message, or replies to a
    /// comment: the share service writes it and tells the guests; the store keeps it at once.
    pub async fn add_comment(&self, params: &ShareCommentAddParams) -> Result<ShareCommentsResult> {
        let d = self.d();
        let chat = d.chats.resolve(&params.chat)?;
        let row = self.share_of_chat(&chat)?;
        let base = self.base_url()?;
        let token = d.shares.token().await.map_err(Error::Unavailable)?;
        let response = self
            .http
            .post(format!(
                "{base}/v1/workspace-shares/{}/comments",
                row.share_id
            ))
            .bearer_auth(&token)
            .json(&NewShareComment {
                chat: chat.id.clone(),
                message: params.message.clone().unwrap_or_default(),
                quote: params.quote.clone().unwrap_or_default(),
                text: params.text.clone(),
                reply_to: params.reply_to.clone(),
                mentions: params.mentions.clone().unwrap_or_default(),
            })
            .send()
            .await
            .map_err(|e| Error::Unavailable(format!("Could not reach the share service: {e}")))?;
        let comment: ShareComment = accepted(response)
            .await
            .map_err(Error::BadRequest)?
            .json()
            .await
            .map_err(|e| Error::Internal(anyhow::anyhow!("the share service's answer: {e}")))?;
        // The service tells the daemon too: whichever comes first is kept once.
        let mut comments = d.store.share_comments_of_chat(&chat.id)?;
        if !comments.iter().any(|c| c.id == comment.id) {
            comments.push(comment);
        }
        self.keep_comments(&chat, comments)
    }

    /// The owner deletes a comment of theirs (a thread's first one with its replies).
    pub async fn delete_comment(&self, params: &ShareCommentParams) -> Result<ShareCommentsResult> {
        let d = self.d();
        let chat = d.chats.resolve(&params.chat)?;
        let row = self.share_of_chat(&chat)?;
        let base = self.base_url()?;
        let token = d.shares.token().await.map_err(Error::Unavailable)?;
        let response = self
            .http
            .delete(format!(
                "{base}/v1/workspace-shares/{}/comments/{}",
                row.share_id, params.id
            ))
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| Error::Unavailable(format!("Could not reach the share service: {e}")))?;
        accepted(response).await.map_err(Error::BadRequest)?;
        let comments = d
            .store
            .share_comments_of_chat(&chat.id)?
            .into_iter()
            .filter(|c| c.id != params.id && c.reply_to.as_deref() != Some(params.id.as_str()))
            .collect();
        self.keep_comments(&chat, comments)
    }

    /// A chat's comments in place of the store's, told to the app.
    fn keep_comments(
        &self,
        chat: &ChatInfo,
        comments: Vec<ShareComment>,
    ) -> Result<ShareCommentsResult> {
        let d = self.d();
        d.store.share_comments_replace(
            &chat.workspace_id,
            std::slice::from_ref(&chat.id),
            &comments,
        )?;
        d.events.chat(
            &chat.workspace_id,
            &chat.id,
            ChatEvent::ShareComments {
                comments: comments.clone(),
            },
        );
        Ok(ShareCommentsResult { comments })
    }

    /// The guests' comments from the service, whole: kept in place of the workspace's, and the
    /// chats whose comments changed told.
    fn receive_comments(&self, workspace_id: &str, comments: Vec<ShareComment>) {
        let d = self.d();
        let shared: HashSet<String> = self
            .shared_chats(workspace_id)
            .into_iter()
            .map(|c| c.id)
            .collect();
        let comments: Vec<ShareComment> = comments
            .into_iter()
            .filter(|c| shared.contains(&c.chat))
            .collect();
        let changed: Vec<(&String, Vec<ShareComment>)> = shared
            .iter()
            .map(|chat| {
                let after = comments.iter().filter(|c| &c.chat == chat).cloned();
                (chat, after.collect())
            })
            .filter(|(chat, after)| {
                d.store.share_comments_of_chat(chat).unwrap_or_default() != *after
            })
            .collect();
        let chats: Vec<String> = shared.iter().cloned().collect();
        if let Err(error) = d
            .store
            .share_comments_replace(workspace_id, &chats, &comments)
        {
            tracing::warn!(%error, "the guests' comments could not be kept");
            return;
        }
        for (chat, comments) in changed {
            d.events
                .chat(workspace_id, chat, ChatEvent::ShareComments { comments });
        }
    }

    // --- the link ---

    async fn run(
        self: Arc<Self>,
        row: WorkspaceShareRow,
        stop: Arc<Notify>,
        suggestions_changed: Arc<Notify>,
        chats_changed: Arc<Notify>,
    ) {
        let workspace_id = row.workspace_id.clone();
        let mut backoff = Duration::from_secs(1);
        loop {
            self.update_link(&workspace_id, |l| {
                l.connection = ShareConnection::Connecting
            });
            self.publish(&workspace_id);
            let outcome = tokio::select! {
                outcome = self.session(&row, &suggestions_changed, &chats_changed) => outcome,
                _ = stop.notified() => return,
            };
            match outcome {
                Ok(Ended::Stopped) => {
                    // The service says the share is gone: nothing to reconnect to.
                    self.links.lock().unwrap().remove(&workspace_id);
                    let _ = self.d().store.workspace_share_put(&WorkspaceShareRow {
                        stopped_at: Some(crate::store::now()),
                        ..row.clone()
                    });
                    self.publish(&workspace_id);
                    return;
                }
                Ok(Ended::Closed) => backoff = Duration::from_secs(1),
                Err(message) => {
                    tracing::warn!(%message, workspace = workspace_id, "the workspace share link failed");
                    self.update_link(&workspace_id, |l| {
                        l.connection = ShareConnection::Failed { message };
                    });
                    self.publish(&workspace_id);
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = stop.notified() => return,
            }
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    }

    /// One connection: hello, the workspace pushed whole, then what changes both ways until it
    /// closes.
    async fn session(
        &self,
        row: &WorkspaceShareRow,
        suggestions_changed: &Notify,
        chats_changed: &Notify,
    ) -> std::result::Result<Ended, String> {
        let d = self.d();
        let workspace_id = &row.workspace_id;
        let base = d
            .shares
            .base_url()
            .ok_or("Sharing is off: set design_share.url in settings.toml")?;
        let token = d.shares.token().await?;
        let ws_base = base
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1);
        let (socket, _) = tokio_tungstenite::connect_async(format!(
            "{ws_base}/v1/workspace-shares/{}/daemon",
            row.share_id
        ))
        .await
        .map_err(|e| format!("Could not reach the share service at {base}: {e}"))?;
        let (mut sink, mut stream) = socket.split();
        sink.send(say(&FromWorkspaceDaemon::Hello {
            id_token: token,
            version: WORKSPACE_SHARE_PROTOCOL_VERSION,
            revision: WORKSPACE_SHARE_PROTOCOL_REVISION,
        }))
        .await
        .map_err(closed)?;
        let (welcome, theirs, comments) = loop {
            match stream.next().await {
                Some(Ok(Message::Text(text))) => {
                    match serde_json::from_str::<ToWorkspaceDaemon>(&text) {
                        Ok(ToWorkspaceDaemon::Welcome {
                            share,
                            suggestions,
                            comments,
                        }) => {
                            break (share, suggestions, comments);
                        }
                        Ok(ToWorkspaceDaemon::Refused { reason })
                            if reason.contains("stopped") || reason.contains("no such share") =>
                        {
                            return Ok(Ended::Stopped);
                        }
                        Ok(ToWorkspaceDaemon::Refused { reason }) => return Err(reason),
                        _ => {}
                    }
                }
                Some(Ok(_)) => {}
                _ => return Err("The share service closed before its welcome".into()),
            }
        };
        self.receive(workspace_id, &theirs);
        self.receive_comments(workspace_id, comments);
        self.update_link(workspace_id, |l| {
            l.connection = ShareConnection::Online;
            l.status = Some(welcome);
        });
        self.publish(workspace_id);

        // Subscribed before the histories are read: nothing logged in between is missed (the
        // service keeps an event once by its seq).
        let mut events = d.events.subscribe();
        let mut chats = self.shared_chats(workspace_id);
        sink.send(say(&FromWorkspaceDaemon::Chats {
            chats: chats.clone(),
        }))
        .await
        .map_err(closed)?;
        for chat in &chats {
            sink.send(say(&self.history(&chat.id)))
                .await
                .map_err(closed)?;
        }
        let notes_dir = d
            .workspaces
            .row(workspace_id)
            .map(|w| Path::new(&w.path).join(NOTES_DIR))
            .map_err(|e| e.to_string())?;
        let mut listing = Listing::new();
        if let Some(notes) = notes_changed(&notes_dir, &mut listing) {
            sink.send(say(&notes)).await.map_err(closed)?;
        }
        sink.send(say(&FromWorkspaceDaemon::Suggestions {
            suggestions: self.shared_suggestions(workspace_id, &chats),
        }))
        .await
        .map_err(closed)?;

        let mut notes_tick = tokio::time::interval(NOTES_EVERY);
        notes_tick.tick().await;
        loop {
            tokio::select! {
                received = events.recv() => {
                    let mut batch = Vec::new();
                    let mut lagged = false;
                    match received {
                        Ok(envelope) => batch.push(envelope),
                        Err(RecvError::Lagged(_)) => lagged = true,
                        Err(RecvError::Closed) => return Err("The daemon is stopping".into()),
                    }
                    // What arrived together goes in one message per chat.
                    loop {
                        match events.try_recv() {
                            Ok(envelope) => batch.push(envelope),
                            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => lagged = true,
                            Err(_) => break,
                        }
                    }
                    let mut chats_touched = lagged;
                    let mut by_chat: Vec<(String, Vec<EventEnvelope>)> = Vec::new();
                    for envelope in &batch {
                        if envelope.event.workspace_id() != Some(workspace_id.as_str()) {
                            continue;
                        }
                        match &envelope.event {
                            Event::Registry {
                                event: RegistryEvent::ChatUpserted { .. } | RegistryEvent::ChatRemoved { .. },
                            } => chats_touched = true,
                            Event::Chat { chat_id, event, .. } => {
                                if matches!(event, ChatEvent::StatusChanged { .. }) {
                                    chats_touched = true;
                                }
                                if lagged {
                                    continue;
                                }
                                let Some(trimmed) = trimmed(envelope) else { continue };
                                match by_chat.last_mut() {
                                    Some((last, events)) if last == chat_id => events.push(trimmed),
                                    _ => by_chat.push((chat_id.clone(), vec![trimmed])),
                                }
                            }
                            _ => {}
                        }
                    }
                    if chats_touched {
                        // A new chat's history (or every one, after events were missed) holds
                        // the batch's events of it: they are not sent again.
                        let new = self.send_chats(workspace_id, &mut chats, lagged, &mut sink).await?;
                        by_chat.retain(|(chat, _)| !new.contains(chat));
                    }
                    let shared: HashSet<&str> = chats.iter().map(|c| c.id.as_str()).collect();
                    for (chat, events) in by_chat {
                        if !shared.contains(chat.as_str()) {
                            continue;
                        }
                        sink.send(say(&FromWorkspaceDaemon::Events { chat, events }))
                            .await
                            .map_err(closed)?;
                    }
                }
                _ = notes_tick.tick() => {
                    if let Some(notes) = notes_changed(&notes_dir, &mut listing) {
                        sink.send(say(&notes)).await.map_err(closed)?;
                    }
                }
                _ = chats_changed.notified() => {
                    self.send_chats(workspace_id, &mut chats, false, &mut sink).await?;
                    sink.send(say(&FromWorkspaceDaemon::Suggestions {
                        suggestions: self.shared_suggestions(workspace_id, &chats),
                    }))
                    .await
                    .map_err(closed)?;
                }
                _ = suggestions_changed.notified() => {
                    sink.send(say(&FromWorkspaceDaemon::Suggestions {
                        suggestions: self.shared_suggestions(workspace_id, &chats),
                    }))
                    .await
                    .map_err(closed)?;
                }
                incoming = stream.next() => {
                    let message = match incoming {
                        Some(Ok(Message::Text(text))) => match serde_json::from_str::<ToWorkspaceDaemon>(&text) {
                            Ok(message) => message,
                            Err(_) => continue,
                        },
                        Some(Ok(Message::Ping(payload))) => {
                            let _ = sink.send(Message::Pong(payload)).await;
                            continue;
                        }
                        Some(Ok(Message::Close(_))) | None => return Ok(Ended::Closed),
                        Some(Ok(_)) => continue,
                        Some(Err(e)) => return Err(format!("The share service closed: {e}")),
                    };
                    match message {
                        ToWorkspaceDaemon::Welcome { share, suggestions, comments } => {
                            self.receive(workspace_id, &suggestions);
                            self.receive_comments(workspace_id, comments);
                            self.update_link(workspace_id, |l| l.status = Some(share));
                            self.publish(workspace_id);
                        }
                        ToWorkspaceDaemon::Suggestion { suggestion } => {
                            self.receive(workspace_id, std::slice::from_ref(&suggestion));
                        }
                        ToWorkspaceDaemon::Comments { comments } => {
                            self.receive_comments(workspace_id, comments);
                        }
                        ToWorkspaceDaemon::Viewers { viewers } => {
                            self.update_link(workspace_id, |l| {
                                if let Some(status) = &mut l.status {
                                    status.viewers = viewers;
                                }
                            });
                            self.publish(workspace_id);
                        }
                        ToWorkspaceDaemon::Refused { reason } => {
                            if reason.contains("stopped") {
                                return Ok(Ended::Stopped);
                            }
                            return Err(reason);
                        }
                    }
                }
            }
        }
    }

    /// Sends the chats guests see when they changed since `chats`, and the history of each one
    /// they did not see (of every one with `all`); returns the chats whose history went.
    async fn send_chats<S>(
        &self,
        workspace_id: &str,
        chats: &mut Vec<SharedChat>,
        all: bool,
        sink: &mut S,
    ) -> std::result::Result<Vec<String>, String>
    where
        S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
    {
        let now = self.shared_chats(workspace_id);
        let known: HashSet<&str> = chats.iter().map(|c| c.id.as_str()).collect();
        let new: Vec<String> = now
            .iter()
            .filter(|c| all || !known.contains(c.id.as_str()))
            .map(|c| c.id.clone())
            .collect();
        if now != *chats {
            sink.send(say(&FromWorkspaceDaemon::Chats { chats: now.clone() }))
                .await
                .map_err(closed)?;
        }
        for chat in &new {
            sink.send(say(&self.history(chat))).await.map_err(closed)?;
        }
        *chats = now;
        Ok(new)
    }

    /// The suggestions made on the chats a guest can see, and on no other.
    fn shared_suggestions(&self, workspace_id: &str, chats: &[SharedChat]) -> Vec<Suggestion> {
        self.d()
            .store
            .suggestions_of_workspace(workspace_id)
            .unwrap_or_default()
            .into_iter()
            .filter(|s| chats.iter().any(|c| c.id == s.chat))
            .collect()
    }

    /// The workspace's open chats, as guests list them.
    fn shared_chats(&self, workspace_id: &str) -> Vec<SharedChat> {
        let d = self.d();
        // A failed selection read must never widen a share to every chat.
        let Ok(Some(row)) = d.store.workspace_share(workspace_id) else {
            return Vec::new();
        };
        d.chats
            .list(workspace_id, false)
            .unwrap_or_default()
            .into_iter()
            .filter(|chat| {
                row.chat_ids
                    .as_ref()
                    .is_none_or(|ids| ids.contains(&chat.id))
            })
            .map(|chat| SharedChat {
                id: chat.id,
                title: chat.title,
                kind: chat.kind,
                agent: chat.agent,
                status: chat.status,
                created_at: chat.created_at,
            })
            .collect()
    }

    fn history(&self, chat_id: &str) -> FromWorkspaceDaemon {
        let events = self
            .d()
            .store
            .events_of_chat(chat_id)
            .unwrap_or_default()
            .iter()
            .filter_map(trimmed)
            .collect();
        FromWorkspaceDaemon::History {
            chat: chat_id.to_string(),
            events,
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn guest_emails(status: &WorkspaceShareStatus) -> Vec<String> {
    status.guests.iter().map(|g| g.email.clone()).collect()
}

/// A chat event as guests get it, or none for what stays on the Mac: the user's shell commands
/// and their output, images and files, a permission's input, a tool's long result, and what the
/// app keeps beside the conversation (tasks' progress, context, MCP servers, shares).
pub fn trimmed(envelope: &EventEnvelope) -> Option<EventEnvelope> {
    let Event::Chat {
        workspace_id,
        chat_id,
        event,
    } = &envelope.event
    else {
        return None;
    };
    let event = match event.clone() {
        ChatEvent::Suggestions { .. }
        | ChatEvent::ShareComments { .. }
        | ChatEvent::DesignShare { .. }
        | ChatEvent::PermissionCheck { .. }
        | ChatEvent::TaskProgress { .. }
        | ChatEvent::PromptSuggestion { .. }
        | ChatEvent::ContextUsage { .. }
        | ChatEvent::McpStatus { .. }
        | ChatEvent::AccountLost { .. }
        | ChatEvent::PermissionModeEffective { .. }
        | ChatEvent::ShellStarted { .. }
        | ChatEvent::ShellEnded { .. } => return None,
        ChatEvent::UserMessage {
            text,
            queued,
            message_id,
            suggested_by,
            delegated_answer,
            ..
        } => ChatEvent::UserMessage {
            text,
            queued,
            attachments: None,
            message_id,
            suggested_by,
            delegated_answer,
        },
        ChatEvent::ToolResult {
            tool_use_id,
            output,
            is_error,
            ..
        } => ChatEvent::ToolResult {
            tool_use_id,
            output: cut(output, TOOL_OUTPUT_MAX),
            is_error,
            images: None,
            attachments: None,
        },
        ChatEvent::PermissionRequest {
            request_id,
            tool_name,
            title,
            agent,
            ..
        } => ChatEvent::PermissionRequest {
            request_id,
            tool_name,
            input: serde_json::Value::Null,
            title,
            agent,
        },
        ChatEvent::McpElicitation {
            request_id,
            server_name,
            message,
            error,
            rule,
            ..
        } => ChatEvent::McpElicitation {
            request_id,
            server_name,
            message,
            schema: None,
            url: None,
            error,
            rule,
        },
        other => other,
    };
    Some(EventEnvelope {
        seq: envelope.seq,
        ts: envelope.ts.clone(),
        event: Event::Chat {
            workspace_id: workspace_id.clone(),
            chat_id: chat_id.clone(),
            event,
        },
    })
}

/// At most `max` bytes of a text, cut on a character, saying how much was left out.
fn cut(text: String, max: usize) -> String {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n… {} more bytes on the owner's Mac",
        &text[..end],
        text.len() - end
    )
}

/// The notes written or removed since `listing`, which it updates; none when nothing changed.
fn notes_changed(dir: &Path, listing: &mut Listing) -> Option<FromWorkspaceDaemon> {
    let now = scan_notes(dir);
    let (changed, removed) = crate::share::diff(Some(listing), &now);
    *listing = now;
    if changed.is_empty() && removed.is_empty() {
        return None;
    }
    let changed = changed
        .into_iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(dir.join(&path)).ok()?;
            Some(SharedNote {
                modified_ms: listing.get(&path).copied().unwrap_or(0),
                path,
                text,
            })
        })
        .collect();
    Some(FromWorkspaceDaemon::Notes { changed, removed })
}

/// The markdown files of a notes folder, by relative path, with when they changed: dotfiles,
/// the design folders, the saved images and files too large are left out.
pub fn scan_notes(dir: &Path) -> Listing {
    let mut out = BTreeMap::new();
    walk_notes(dir, Path::new(""), 0, &mut out);
    out
}

fn walk_notes(root: &Path, relative: &Path, depth: usize, out: &mut Listing) {
    if depth > NOTES_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root.join(relative)) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = relative.join(&*name);
        if kind.is_dir() {
            if depth == 0 && SKIPPED_NOTE_DIRS.contains(&&*name) {
                continue;
            }
            walk_notes(root, &path, depth + 1, out);
        } else if kind.is_file()
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("md"))
            && entry.metadata().is_ok_and(|m| m.len() <= MAX_NOTE_BYTES)
        {
            out.insert(
                path.to_string_lossy().into_owned(),
                modified_ms(&root.join(&path)),
            );
        }
    }
}

/// Shared notes are data, never agent configuration or executable hooks.
fn safe_note_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', '\0', ':'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.'))
        && path.to_ascii_lowercase().ends_with(".md")
}

/// The id of a workspace share's link, `<service>/w/<id>`, when the link is on one of the
/// service's addresses (`origins`, `https://…`); none for any other link.
fn workspace_share_id<'a>(
    link: &str,
    origins: impl IntoIterator<Item = &'a String>,
) -> Option<String> {
    let url = reqwest::Url::parse(link.trim()).ok()?;
    let id = url
        .path()
        .trim_end_matches('/')
        .strip_prefix("/w/")
        .filter(|id| {
            id.len() >= 8 && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
        })?;
    let ours = origins.into_iter().any(|origin| {
        reqwest::Url::parse(origin.trim()).is_ok_and(|origin| origin.origin() == url.origin())
    });
    (ours && url.username().is_empty() && url.password().is_none()).then(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chauffeur_protocol::model::{Attachment, ImageInput};

    #[test]
    fn a_workspace_link_is_taken_on_any_of_the_services_addresses_only() {
        let origins = [
            "https://design.example".to_string(),
            "https://share.example/".to_string(),
            "https://old.example".to_string(),
        ];
        for (link, id) in [
            (
                "https://design.example/w/0123456789abcdef",
                "0123456789abcdef",
            ),
            (
                " https://share.example/w/0123456789ABCDEF/ ",
                "0123456789ABCDEF",
            ),
            ("https://old.example/w/0123456789abcdef", "0123456789abcdef"),
        ] {
            assert_eq!(
                workspace_share_id(link, &origins).as_deref(),
                Some(id),
                "{link}"
            );
        }
        for link in [
            "https://other.example/w/0123456789abcdef",
            "http://share.example/w/0123456789abcdef",
            "https://user:pw@share.example/w/0123456789abcdef",
            "https://share.example/s/0123456789abcdef",
            "https://share.example/w/not-hex",
            "https://share.example/w/0123",
            "not a link",
        ] {
            assert!(workspace_share_id(link, &origins).is_none(), "{link}");
        }
    }

    #[test]
    fn a_share_preview_counts_what_the_import_copies() {
        let chat = |id: &str| serde_json::json!({ "id": id, "title": null, "kind": "chat", "agent": "claude", "status": "idle", "created_at": "2026-11-04T10:00:00Z" });
        let author = serde_json::json!({ "name": "Alice", "email": "alice@example.com" });
        let snapshot: chauffeur_protocol::workspace_share::WorkspaceShareSnapshot =
            serde_json::from_value(serde_json::json!({
                "id": "abcdef12", "title": "Checkout redesign", "owner": author, "me": author,
                "role": "viewer", "chats": [chat("a"), chat("b"), chat("c")],
                "notes": [{ "path": "plan.md", "text": "# Plan", "modified_ms": 1 }],
                "suggestions": [], "viewers": [], "online": false, "last_push_at": 1_762_300_000_000u64,
            }))
            .unwrap();
        let preview = preview_of(&snapshot);
        assert_eq!(preview.title, "Checkout redesign");
        assert_eq!(preview.owner.name, "Alice");
        assert_eq!((preview.chats, preview.notes), (3, 1));
        assert_eq!(preview.last_push_at, Some(1_762_300_000_000));
    }

    fn envelope(event: ChatEvent) -> EventEnvelope {
        EventEnvelope {
            seq: Some(7),
            ts: "2026-10-29T10:00:00Z".into(),
            event: Event::Chat {
                workspace_id: "w".into(),
                chat_id: "c".into(),
                event,
            },
        }
    }

    fn event_of(envelope: Option<EventEnvelope>) -> ChatEvent {
        match envelope.expect("shared").event {
            Event::Chat { event, .. } => event,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn guests_get_no_shell_output_no_images_and_no_permission_input() {
        for kept_home in [
            ChatEvent::ShellStarted {
                shell_id: "s".into(),
                command: "env".into(),
            },
            ChatEvent::PromptSuggestion { text: "x".into() },
            ChatEvent::Suggestions {
                suggestions: Vec::new(),
            },
        ] {
            assert!(trimmed(&envelope(kept_home)).is_none());
        }
        let attachment: Attachment =
            serde_json::from_value(serde_json::json!({"file": "a.png", "name": "a.png", "media_type": "image/png", "size": 3, "path": "/w/.chauffeur/attachments/a.png"}))
                .unwrap();
        let ChatEvent::UserMessage {
            attachments, text, ..
        } = event_of(trimmed(&envelope(ChatEvent::UserMessage {
            text: "look".into(),
            queued: false,
            attachments: Some(vec![attachment.clone()]),
            message_id: None,
            suggested_by: None,
            delegated_answer: None,
        })))
        else {
            panic!()
        };
        assert_eq!((text.as_str(), attachments), ("look", None));
        let ChatEvent::ToolResult {
            output,
            images,
            attachments,
            ..
        } = event_of(trimmed(&envelope(ChatEvent::ToolResult {
            tool_use_id: "t".into(),
            output: "é".repeat(TOOL_OUTPUT_MAX),
            is_error: false,
            images: Some(vec![ImageInput::default()]),
            attachments: Some(vec![attachment]),
        })))
        else {
            panic!()
        };
        assert!(images.is_none() && attachments.is_none());
        assert!(output.len() < TOOL_OUTPUT_MAX + 100);
        assert!(output.ends_with("more bytes on the owner's Mac"));
        let ChatEvent::PermissionRequest {
            input, tool_name, ..
        } = event_of(trimmed(&envelope(ChatEvent::PermissionRequest {
            request_id: "r".into(),
            tool_name: "Bash".into(),
            input: serde_json::json!({"command": "cat .env"}),
            title: None,
            agent: None,
        })))
        else {
            panic!()
        };
        assert_eq!(
            (tool_name.as_str(), input),
            ("Bash", serde_json::Value::Null)
        );
        let text = ChatEvent::AssistantText {
            message_id: "m".into(),
            text: "Here is the plan".into(),
            parent_tool_use_id: None,
        };
        let shared = trimmed(&envelope(text.clone())).unwrap();
        assert_eq!(shared.seq, Some(7));
        assert_eq!(event_of(Some(shared)), text);
    }

    #[test]
    fn imported_notes_cannot_write_configuration_or_escape_their_folder() {
        for valid in ["plan.md", "adr/0001.MD"] {
            assert!(safe_note_path(valid));
        }
        for invalid in [
            "../escape.md",
            "/absolute.md",
            ".claude/settings.md",
            "a/../x.md",
            "a//x.md",
            "a\\x.md",
            "hooks.sh",
            "a\0.md",
            "C:/x.md",
        ] {
            assert!(!safe_note_path(invalid), "{invalid}");
        }
    }

    #[test]
    fn a_short_result_is_kept_whole_and_a_long_one_cut_on_a_character() {
        assert_eq!(cut("abc".into(), 8), "abc");
        let cut_text = cut("aé".repeat(10), 4);
        assert!(cut_text.starts_with("aéa\n"), "{cut_text}");
    }

    #[test]
    fn the_notes_shared_are_the_markdown_outside_designs_and_attachments() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        for folder in ["plans", "designs/c1", "attachments", ".claude"] {
            std::fs::create_dir_all(d.join(folder)).unwrap();
        }
        for (path, body) in [
            ("adr.md", "# ADR"),
            ("plans/split.MD", "plan"),
            ("notes.txt", "x"),
            ("designs/c1/brief.md", "brief"),
            ("attachments/a.md", "a"),
            (".claude/memory.md", "m"),
        ] {
            std::fs::write(d.join(path), body).unwrap();
        }
        std::fs::write(d.join("huge.md"), vec![b'a'; MAX_NOTE_BYTES as usize + 1]).unwrap();
        assert_eq!(
            scan_notes(d).keys().collect::<Vec<_>>(),
            ["adr.md", "plans/split.MD"]
        );
        let mut listing = Listing::new();
        let Some(FromWorkspaceDaemon::Notes { changed, removed }) = notes_changed(d, &mut listing)
        else {
            panic!("the first scan sends every note")
        };
        assert_eq!(changed.len(), 2);
        assert!(removed.is_empty());
        assert!(notes_changed(d, &mut listing).is_none(), "nothing changed");
        std::fs::remove_file(d.join("adr.md")).unwrap();
        let Some(FromWorkspaceDaemon::Notes { changed, removed }) = notes_changed(d, &mut listing)
        else {
            panic!()
        };
        assert!(changed.is_empty());
        assert_eq!(removed, ["adr.md"]);
    }
}
