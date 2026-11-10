//! Workspace shares (story 53) over HTTP: the owner's daemon creates, changes and stops a share;
//! a guest's page (or the owner's) reads its snapshot and a chat's events. What changes then goes
//! over the sockets (workspace_ws.rs). Each guest is a viewer or may suggest; the accounts of the
//! share's team domain, when it is open to one, read as viewers. The owner's daemon also comments
//! on the chats' messages here, as a guest who can suggest does on its socket.

use std::sync::Arc;

use axum::Json;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chauffeur_protocol::client::CommentAuthor;
use chauffeur_protocol::design_share::MentionPlace;
use chauffeur_protocol::events::EventEnvelope;
use chauffeur_protocol::workspace_share::{
    Guest, GuestRole, NewShareComment, ShareComment, SharedChat, SharedNote, Suggestion,
    SuggestionState, ToGuest, ToWorkspaceDaemon, WorkspaceShareParams, WorkspaceShareSnapshot,
    WorkspaceShareStatus,
};
use tokio::sync::mpsc;

use crate::mentions::Audience;
use crate::store::WorkspaceShareRow;
use crate::{
    Identity, MAX_TITLE, Reject, Service, internal, mail, now_ms, random_hex, team_domain,
};

const DEFAULT_TITLE: &str = "Workspace";
/// A suggestion's text, at most.
pub const SUGGESTION_MAX: usize = 20_000;
/// A comment's text, at most.
pub const COMMENT_MAX: usize = 5_000;
/// The passage a comment is on, at most.
pub const QUOTE_MAX: usize = 2_000;
/// A note's text, at most (the daemon sends none larger).
pub const NOTE_MAX: usize = 1024 * 1024;

/// Who is connected to a workspace share now.
#[derive(Default)]
pub struct Live {
    pub(crate) daemon: Option<(u64, mpsc::UnboundedSender<ToWorkspaceDaemon>)>,
    pub(crate) guests: Vec<GuestConnection>,
}

pub(crate) struct GuestConnection {
    pub(crate) id: u64,
    pub(crate) identity: Identity,
    pub(crate) tx: mpsc::UnboundedSender<ToGuest>,
}

/// The guests a share is opened to: e-mails lowercase, once (the first role given wins), at
/// most `max`.
pub fn valid_guests(guests: &[Guest], max: usize) -> Result<Vec<Guest>, String> {
    let mut out: Vec<Guest> = Vec::new();
    for guest in guests {
        let email = guest.email.trim().to_lowercase();
        if email.is_empty() {
            continue;
        }
        if !email.contains('@') || email.contains(char::is_whitespace) {
            return Err(format!("{email} is not an e-mail address"));
        }
        if !out.iter().any(|g| g.email == email) {
            out.push(Guest {
                email,
                role: guest.role,
            });
        }
    }
    if out.len() > max {
        return Err(format!("A share names at most {max} people: remove some"));
    }
    Ok(out)
}

fn valid_title(title: Option<&str>) -> String {
    let title = title
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or(DEFAULT_TITLE);
    title.chars().take(MAX_TITLE).collect()
}

/// A guest's suggestion for a chat, by the share's rules: only a guest who can suggest, on a
/// chat of the share, with a text.
pub fn new_suggestion(
    role: GuestRole,
    chats: &[SharedChat],
    chat: &str,
    text: &str,
    me: &CommentAuthor,
    now: u64,
) -> Result<Suggestion, String> {
    if role != GuestRole::Suggest {
        return Err(
            "Only guests who can suggest send suggestions: ask the owner to let you suggest".into(),
        );
    }
    if !chats.iter().any(|c| c.id == chat) {
        return Err("This chat is no longer in the shared workspace".into());
    }
    let text = text.trim();
    if text.is_empty() {
        return Err("A suggestion needs a text".into());
    }
    if text.chars().count() > SUGGESTION_MAX {
        return Err(format!(
            "A suggestion is at most {SUGGESTION_MAX} characters"
        ));
    }
    Ok(Suggestion {
        id: random_hex(8),
        chat: chat.to_string(),
        author: me.clone(),
        text: text.to_string(),
        at: now,
        state: SuggestionState::Pending,
        decided_at: None,
    })
}

/// The daemon's suggestions win for the ids it has, but for one its author deleted here, which
/// the daemon may not have heard of yet (it was away, or is older than deletions); a pending or
/// deleted one it lacks stays. Oldest first.
pub fn merge_suggestions(held: &[Suggestion], daemon: Vec<Suggestion>) -> Vec<Suggestion> {
    let mut out = daemon;
    for suggestion in held {
        match out.iter_mut().find(|s| s.id == suggestion.id) {
            Some(theirs) => {
                if suggestion.state == SuggestionState::Deleted
                    && matches!(
                        theirs.state,
                        SuggestionState::Pending | SuggestionState::Dismissed
                    )
                {
                    theirs.state = SuggestionState::Deleted;
                }
            }
            None => {
                if matches!(
                    suggestion.state,
                    SuggestionState::Pending | SuggestionState::Deleted
                ) {
                    out.push(suggestion.clone());
                }
            }
        }
    }
    out.sort_by_key(|s| s.at);
    out
}

/// What guests see of the suggestions: none deleted.
pub fn shown_suggestions(suggestions: Vec<Suggestion>) -> Vec<Suggestion> {
    suggestions
        .into_iter()
        .filter(|s| s.state != SuggestionState::Deleted)
        .collect()
}

/// A guest takes a suggestion of theirs back, while it waits or once dismissed: a sent one is in
/// the chat already.
pub fn delete_suggestion(
    held: &[Suggestion],
    id: &str,
    me: &CommentAuthor,
) -> Result<Suggestion, String> {
    let Some(suggestion) = held
        .iter()
        .find(|s| s.id == id && s.state != SuggestionState::Deleted)
    else {
        return Err("This suggestion is no longer there".into());
    };
    if suggestion.author.email != me.email {
        return Err(format!(
            "Only {} deletes their suggestion",
            suggestion.author.name
        ));
    }
    if !matches!(
        suggestion.state,
        SuggestionState::Pending | SuggestionState::Dismissed
    ) {
        return Err("A sent suggestion stays: it is in the chat".into());
    }
    Ok(Suggestion {
        state: SuggestionState::Deleted,
        ..suggestion.clone()
    })
}

/// A comment by the share's rules, from a guest who can suggest or from the owner: on a chat of
/// the share, a new thread on a passage of an agent's message, or a reply to a thread's first
/// comment (it takes that thread's chat and message); its mentions kept to the people the share
/// is open to.
pub fn new_comment(
    chats: &[SharedChat],
    comments: &[ShareComment],
    asked: NewShareComment,
    me: &CommentAuthor,
    audience: &Audience,
    now: u64,
) -> Result<ShareComment, String> {
    let text = asked.text.trim();
    if text.is_empty() {
        return Err("A comment needs a text".into());
    }
    if text.chars().count() > COMMENT_MAX {
        return Err(format!("A comment is at most {COMMENT_MAX} characters"));
    }
    let (chat, message, quote) = match &asked.reply_to {
        Some(thread) => {
            let Some(first) = comments.iter().find(|c| &c.id == thread) else {
                return Err("This comment is no longer there".into());
            };
            if first.reply_to.is_some() {
                return Err("Reply to the thread's first comment".into());
            }
            (first.chat.clone(), first.message.clone(), String::new())
        }
        None => {
            let quote = asked.quote.trim();
            if asked.message.is_empty() || asked.message.len() > 200 || quote.is_empty() {
                return Err("Select a passage of the agent's message to comment on it".into());
            }
            if quote.chars().count() > QUOTE_MAX {
                return Err(format!(
                    "A comment is on at most {QUOTE_MAX} characters: select a shorter passage"
                ));
            }
            (asked.chat, asked.message, quote.to_string())
        }
    };
    if !chats.iter().any(|c| c.id == chat) {
        return Err("This chat is no longer in the shared workspace".into());
    }
    Ok(ShareComment {
        id: random_hex(8),
        chat,
        message,
        quote,
        author: me.clone(),
        text: text.to_string(),
        at: now,
        reply_to: asked.reply_to,
        mentions: audience.vet(&asked.mentions, &me.email),
    })
}

/// Its author deletes a comment: the comments that go, a thread's first one with its replies.
pub fn delete_comment(
    comments: &[ShareComment],
    id: &str,
    me: &CommentAuthor,
) -> Result<Vec<String>, String> {
    let Some(comment) = comments.iter().find(|c| c.id == id) else {
        return Err("This comment is no longer there".into());
    };
    if comment.author.email != me.email {
        return Err(format!(
            "Only {} deletes their comment",
            comment.author.name
        ));
    }
    Ok(comments
        .iter()
        .filter(|c| c.id == id || c.reply_to.as_deref() == Some(id))
        .map(|c| c.id.clone())
        .collect())
}

/// A note's path as the notes folder has it: relative, inside it, markdown.
pub fn valid_note_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && path.to_lowercase().ends_with(".md")
}

impl Service {
    pub(crate) fn workspace_share(&self, id: &str) -> Result<WorkspaceShareRow, Reject> {
        self.store
            .workspace_share(id)
            .map_err(internal)?
            .ok_or_else(|| Reject(StatusCode::NOT_FOUND, "There is no such share".into()))
    }

    /// The share and its owner, from their daemon (a bearer) or their own page (the cookie).
    async fn workspace_owned(
        &self,
        headers: &HeaderMap,
        id: &str,
    ) -> Result<(Identity, WorkspaceShareRow), Reject> {
        let identity = self.viewer_identity(headers).await?;
        let share = self.workspace_share(id)?;
        if share.owner.email != identity.email {
            return Err(Reject(
                StatusCode::FORBIDDEN,
                "This share is not yours".into(),
            ));
        }
        Ok((identity, share))
    }

    /// The share and what the identity may do on it, while it lives.
    pub(crate) fn workspace_viewable(
        &self,
        id: &str,
        identity: &Identity,
    ) -> Result<(WorkspaceShareRow, GuestRole), Reject> {
        let share = self.workspace_share(id)?;
        if share.stopped() {
            return Err(Reject(StatusCode::GONE, "This share was stopped".into()));
        }
        let role = share.role_of(identity).ok_or_else(|| {
            Reject(
                StatusCode::FORBIDDEN,
                format!(
                    "This workspace was not shared with you. Ask {}.",
                    share.owner.name
                ),
            )
        })?;
        Ok((share, role))
    }

    pub(crate) fn workspace_viewers(&self, id: &str) -> Vec<String> {
        let live = self.workspace_live.lock().unwrap();
        let mut viewers: Vec<String> = live
            .get(id)
            .map(|l| l.guests.iter().map(|g| g.identity.email.clone()).collect())
            .unwrap_or_default();
        viewers.sort();
        viewers.dedup();
        viewers
    }

    pub(crate) fn workspace_online(&self, id: &str) -> bool {
        self.workspace_live
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|l| l.daemon.is_some())
    }

    pub(crate) fn workspace_status(&self, share: &WorkspaceShareRow) -> WorkspaceShareStatus {
        WorkspaceShareStatus {
            id: share.id.clone(),
            url: format!("{}/w/{}", self.workspace_url(), share.id),
            title: share.title.clone(),
            owner: share.owner.clone(),
            guests: share.guests.clone(),
            people: self.workspace_audience(share).people().to_vec(),
            team: share.team_domain.is_some(),
            viewers: self.workspace_viewers(&share.id),
            online: self.workspace_online(&share.id),
            last_push_at: share.last_push_at,
            stopped: share.stopped(),
            notice: None,
        }
    }

    pub(crate) fn workspace_snapshot(
        &self,
        share: &WorkspaceShareRow,
        me: &Identity,
        role: GuestRole,
    ) -> Result<WorkspaceShareSnapshot, Reject> {
        Ok(WorkspaceShareSnapshot {
            id: share.id.clone(),
            title: share.title.clone(),
            owner: share.owner.clone(),
            me: me.author(),
            role,
            chats: self.store.workspace_chats(&share.id).map_err(internal)?,
            notes: self.store.workspace_notes(&share.id).map_err(internal)?,
            suggestions: shown_suggestions(
                self.store
                    .workspace_suggestions(&share.id)
                    .map_err(internal)?,
            ),
            comments: self.store.workspace_comments(&share.id).map_err(internal)?,
            people: self.workspace_audience(share).people().to_vec(),
            team_domain: share.team_domain.clone(),
            unread_mentions: self
                .store
                .unread_comments(MentionPlace::Workspace, &share.id, &me.email)
                .map_err(internal)?,
            viewers: self.workspace_viewers(&share.id),
            online: self.workspace_online(&share.id),
            last_push_at: share.last_push_at,
        })
    }

    pub(crate) fn with_workspace_live<T>(&self, id: &str, f: impl FnOnce(&mut Live) -> T) -> T {
        let mut live = self.workspace_live.lock().unwrap();
        let entry = live.entry(id.to_string()).or_default();
        let out = f(entry);
        if entry.daemon.is_none() && entry.guests.is_empty() {
            live.remove(id);
        }
        out
    }

    /// Writes to the share's guests not in `already` (mail.rs), naming their role.
    fn invite_guests(
        &self,
        share: &WorkspaceShareRow,
        already: &[&str],
    ) -> Result<Option<String>, Reject> {
        let link = format!("{}/w/{}", self.workspace_url(), share.id);
        self.invite(
            &share.owner,
            mail::invitees(
                &share.owner.email,
                share.guests.iter().map(|g| g.email.as_str()),
                already,
            ),
            |to| {
                let role = share
                    .guests
                    .iter()
                    .find(|g| g.email == to)
                    .map_or(GuestRole::Viewer, |g| g.role);
                mail::invitation(
                    &share.owner,
                    to,
                    &share.title,
                    &link,
                    mail::Shared::Workspace(role),
                    &self.footer(to),
                )
            },
        )
    }

    pub(crate) fn tell_guests(&self, id: &str, message: &ToGuest) {
        let live = self.workspace_live.lock().unwrap();
        if let Some(l) = live.get(id) {
            for guest in &l.guests {
                let _ = guest.tx.send(message.clone());
            }
        }
    }

    pub(crate) fn tell_workspace_daemon(&self, id: &str, message: ToWorkspaceDaemon) {
        let live = self.workspace_live.lock().unwrap();
        if let Some((_, tx)) = live.get(id).and_then(|l| l.daemon.as_ref()) {
            let _ = tx.send(message);
        }
    }

    /// The comments changed: every page and the owner's daemon hear of them whole, and the
    /// share's mentions follow them.
    pub(crate) fn workspace_comments_changed(&self, id: &str) {
        if let Ok(comments) = self.store.workspace_comments(id) {
            self.tell_guests(
                id,
                &ToGuest::Comments {
                    comments: comments.clone(),
                },
            );
            self.tell_workspace_daemon(id, ToWorkspaceDaemon::Comments { comments });
        }
        self.sync_workspace_mentions(id);
    }

    pub(crate) fn announce_workspace_viewers(&self, id: &str) {
        let viewers = self.workspace_viewers(id);
        self.tell_guests(
            id,
            &ToGuest::Viewers {
                viewers: viewers.clone(),
            },
        );
        self.tell_workspace_daemon(id, ToWorkspaceDaemon::Viewers { viewers });
    }
}

// --- the owner's daemon ---

pub(crate) async fn create(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    Json(params): Json<WorkspaceShareParams>,
) -> Result<Json<WorkspaceShareStatus>, Reject> {
    let identity = service.daemon_identity(&headers).await?;
    let guests = valid_guests(
        params.guests.as_deref().unwrap_or_default(),
        service.max_invitees(&identity.email),
    )
    .map_err(|reason| Reject(StatusCode::BAD_REQUEST, reason))?;
    let team_domain = team_domain(&identity, params.team.unwrap_or(false))?;
    let share = service
        .store
        .create_workspace_share(
            &random_hex(16),
            &identity.author(),
            &valid_title(params.title.as_deref()),
            &guests,
            team_domain.as_deref(),
            now_ms(),
        )
        .map_err(internal)?;
    tracing::info!(
        share = share.id,
        owner = identity.email,
        guests = guests.len(),
        team = share.team_domain,
        "workspace share created"
    );
    let notice = service.invite_guests(&share, &[])?;
    Ok(Json(WorkspaceShareStatus {
        notice,
        ..service.workspace_status(&share)
    }))
}

pub(crate) async fn get(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<WorkspaceShareStatus>, Reject> {
    let (_, share) = service.workspace_owned(&headers, &id).await?;
    Ok(Json(service.workspace_status(&share)))
}

pub(crate) async fn patch(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    Json(params): Json<WorkspaceShareParams>,
) -> Result<Json<WorkspaceShareStatus>, Reject> {
    let (identity, before) = service.workspace_owned(&headers, &id).await?;
    if before.stopped() {
        return Err(Reject(
            StatusCode::GONE,
            "This share was stopped: share again".into(),
        ));
    }
    let max = service.max_invitees(&identity.email);
    let guests = params
        .guests
        .as_deref()
        .map(|guests| valid_guests(guests, max))
        .transpose()
        .map_err(|reason| Reject(StatusCode::BAD_REQUEST, reason))?;
    let title = params.title.as_deref().map(|t| valid_title(Some(t)));
    let team = params
        .team
        .map(|team| team_domain(&identity, team))
        .transpose()?;
    service
        .store
        .update_workspace_share(
            &id,
            title.as_deref(),
            guests.as_deref(),
            team.as_ref().map(Option::as_deref),
        )
        .map_err(internal)?;
    let share = service.workspace_share(&id)?;
    let notice = service.invite_guests(
        &share,
        &before
            .guests
            .iter()
            .map(|g| g.email.as_str())
            .collect::<Vec<_>>(),
    )?;
    // Someone looking who lost the share is refused at once; one whose role changed reads again.
    // The snapshots are built outside the lock they take.
    let pages: Vec<(Identity, mpsc::UnboundedSender<ToGuest>)> =
        service.with_workspace_live(&id, |live| {
            live.guests
                .iter()
                .map(|g| (g.identity.clone(), g.tx.clone()))
                .collect()
        });
    for (reader, tx) in pages {
        match (before.role_of(&reader), share.role_of(&reader)) {
            (_, None) => {
                let _ = tx.send(ToGuest::Refused {
                    reason: format!(
                        "This workspace is no longer shared with you. Ask {}.",
                        share.owner.name
                    ),
                });
            }
            (old, Some(role)) if old != Some(role) => {
                if let Ok(snapshot) = service.workspace_snapshot(&share, &reader, role) {
                    let _ = tx.send(ToGuest::Snapshot {
                        snapshot: Box::new(snapshot),
                    });
                }
            }
            _ => {}
        }
    }
    // Every page hears who the share is open to; someone no longer let in loses their mentions.
    service.tell_workspace_people(&share);
    service.sync_workspace_mentions(&id);
    Ok(Json(WorkspaceShareStatus {
        notice,
        ..service.workspace_status(&share)
    }))
}

pub(crate) async fn delete(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<WorkspaceShareStatus>, Reject> {
    service.workspace_owned(&headers, &id).await?;
    service
        .store
        .stop_workspace_share(&id, now_ms())
        .map_err(internal)?;
    let stopped = "This share was stopped".to_string();
    service.tell_guests(
        &id,
        &ToGuest::Refused {
            reason: stopped.clone(),
        },
    );
    service.tell_workspace_daemon(&id, ToWorkspaceDaemon::Refused { reason: stopped });
    service.sync_workspace_mentions(&id);
    let share = service.workspace_share(&id)?;
    tracing::info!(share = id, "workspace share stopped");
    Ok(Json(service.workspace_status(&share)))
}

/// The owner comments on a passage of a chat's agent message, or replies, from their daemon.
pub(crate) async fn add_comment(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    Json(asked): Json<NewShareComment>,
) -> Result<Json<ShareComment>, Reject> {
    let (identity, share) = service.workspace_owned(&headers, &id).await?;
    if share.stopped() {
        return Err(Reject(StatusCode::GONE, "This share was stopped".into()));
    }
    let chats = service.store.workspace_chats(&id).map_err(internal)?;
    let comments = service.store.workspace_comments(&id).map_err(internal)?;
    let comment = new_comment(
        &chats,
        &comments,
        asked,
        &identity.author(),
        &service.workspace_audience(&share),
        now_ms(),
    )
    .map_err(|reason| Reject(StatusCode::BAD_REQUEST, reason))?;
    service.store.add_comment(&id, &comment).map_err(internal)?;
    service.workspace_comments_changed(&id);
    Ok(Json(comment))
}

/// The owner deletes a comment of theirs, from their daemon.
pub(crate) async fn delete_comment_of_owner(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    UrlPath((id, comment)): UrlPath<(String, String)>,
) -> Result<StatusCode, Reject> {
    let (identity, _) = service.workspace_owned(&headers, &id).await?;
    let comments = service.store.workspace_comments(&id).map_err(internal)?;
    let gone = delete_comment(&comments, &comment, &identity.author())
        .map_err(|reason| Reject(StatusCode::BAD_REQUEST, reason))?;
    service
        .store
        .remove_comments(&id, &gone)
        .map_err(internal)?;
    service.workspace_comments_changed(&id);
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn daemon_upgrade(
    State(service): State<Arc<Service>>,
    UrlPath(id): UrlPath<String>,
    upgrade: WebSocketUpgrade,
) -> Response {
    upgrade
        .max_message_size(crate::workspace_ws::MAX_DAEMON_MESSAGE)
        .max_frame_size(crate::workspace_ws::MAX_DAEMON_MESSAGE)
        .on_upgrade(move |socket| crate::workspace_ws::daemon_socket(service, id, socket))
}

// --- the guests' page ---

pub(crate) async fn snapshot(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<WorkspaceShareSnapshot>, Reject> {
    let identity = service.viewer_identity(&headers).await?;
    let (share, role) = service.workspace_viewable(&id, &identity)?;
    Ok(Json(service.workspace_snapshot(&share, &identity, role)?))
}

/// A chat's stored events, for the page to lay the conversation out as the app does.
pub(crate) async fn chat_events(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    UrlPath((id, chat)): UrlPath<(String, String)>,
) -> Result<Json<Vec<EventEnvelope>>, Reject> {
    let identity = service.viewer_identity(&headers).await?;
    let (share, _) = service.workspace_viewable(&id, &identity)?;
    let chats = service.store.workspace_chats(&share.id).map_err(internal)?;
    if !chats.iter().any(|c| c.id == chat) {
        return Err(Reject(
            StatusCode::NOT_FOUND,
            "This chat is not in the shared workspace".into(),
        ));
    }
    Ok(Json(
        service.store.chat_events(&id, &chat).map_err(internal)?,
    ))
}

pub(crate) async fn guest_upgrade(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, Reject> {
    let identity = service.reviewer_identity(&headers)?;
    service.workspace_viewable(&id, &identity)?;
    Ok(upgrade
        .on_upgrade(move |socket| crate::workspace_ws::guest_socket(service, id, identity, socket))
        .into_response())
}

/// The notes the daemon may push: markdown of the notes folder, each within its size.
pub(crate) fn valid_notes(changed: Vec<SharedNote>) -> Vec<SharedNote> {
    changed
        .into_iter()
        .filter(|note| valid_note_path(&note.path) && note.text.len() <= NOTE_MAX)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chauffeur_protocol::model::{AgentKind, ChatKind, ChatStatus};

    fn alice() -> CommentAuthor {
        CommentAuthor {
            name: "Alice".into(),
            email: "alice@example.com".into(),
        }
    }

    fn chats() -> Vec<SharedChat> {
        vec![SharedChat {
            id: "c1".into(),
            title: Some("Architecture".into()),
            kind: ChatKind::Chat,
            agent: AgentKind::Claude,
            status: ChatStatus::Working,
            created_at: "2026-10-29T10:00:00Z".into(),
        }]
    }

    fn suggestion(id: &str, at: u64, state: SuggestionState) -> Suggestion {
        Suggestion {
            id: id.into(),
            chat: "c1".into(),
            author: alice(),
            text: id.into(),
            at,
            state,
            decided_at: None,
        }
    }

    #[test]
    fn guests_are_emails_lowercase_once_with_the_first_role_and_bounded() {
        let guests = valid_guests(
            &[
                Guest {
                    email: " Alice@example.com".into(),
                    role: GuestRole::Suggest,
                },
                Guest {
                    email: "".into(),
                    role: GuestRole::Viewer,
                },
                Guest {
                    email: "alice@example.com".into(),
                    role: GuestRole::Viewer,
                },
            ],
            20,
        )
        .unwrap();
        assert_eq!(
            guests,
            [Guest {
                email: "alice@example.com".into(),
                role: GuestRole::Suggest
            }]
        );
        assert!(
            valid_guests(
                &[Guest {
                    email: "alice".into(),
                    role: GuestRole::Viewer
                }],
                20
            )
            .is_err()
        );
        let many: Vec<Guest> = (0..21)
            .map(|i| Guest {
                email: format!("g{i}@example.com"),
                role: GuestRole::Viewer,
            })
            .collect();
        assert!(valid_guests(&many, 20).is_err());
        assert!(valid_guests(&many, 50).is_ok());
        assert_eq!(valid_title(None), "Workspace");
        assert_eq!(valid_title(Some(" Toyota ")), "Toyota");
    }

    #[test]
    fn only_a_guest_who_can_suggest_suggests_a_text_to_a_listed_chat() {
        let made = new_suggestion(
            GuestRole::Suggest,
            &chats(),
            "c1",
            "  Split the service in two  ",
            &alice(),
            7,
        )
        .unwrap();
        assert_eq!(
            (
                made.text.as_str(),
                made.chat.as_str(),
                made.at,
                made.state,
                made.author.clone()
            ),
            (
                "Split the service in two",
                "c1",
                7,
                SuggestionState::Pending,
                alice()
            )
        );
        assert_eq!(made.id.len(), 16);
        assert!(new_suggestion(GuestRole::Viewer, &chats(), "c1", "x", &alice(), 7).is_err());
        assert!(new_suggestion(GuestRole::Suggest, &chats(), "c9", "x", &alice(), 7).is_err());
        assert!(new_suggestion(GuestRole::Suggest, &chats(), "c1", "  ", &alice(), 7).is_err());
        let long = "x".repeat(SUGGESTION_MAX + 1);
        assert!(new_suggestion(GuestRole::Suggest, &chats(), "c1", &long, &alice(), 7).is_err());
    }

    #[test]
    fn a_guest_deletes_their_own_suggestion_unless_it_was_sent() {
        let held = vec![
            suggestion("a", 1, SuggestionState::Pending),
            suggestion("b", 2, SuggestionState::Dismissed),
            suggestion("c", 3, SuggestionState::Sent),
            suggestion("d", 4, SuggestionState::Deleted),
        ];
        assert_eq!(
            delete_suggestion(&held, "a", &alice()).unwrap().state,
            SuggestionState::Deleted
        );
        assert_eq!(
            delete_suggestion(&held, "b", &alice()).unwrap().state,
            SuggestionState::Deleted
        );
        assert!(delete_suggestion(&held, "c", &alice()).is_err());
        assert!(delete_suggestion(&held, "d", &alice()).is_err());
        assert!(delete_suggestion(&held, "z", &alice()).is_err());
        let carol = CommentAuthor {
            name: "Carol".into(),
            email: "carol@example.com".into(),
        };
        assert!(delete_suggestion(&held, "a", &carol).is_err());
        assert_eq!(
            shown_suggestions(held)
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
    }

    #[test]
    fn a_deletion_outlives_a_daemon_that_has_not_heard_of_it() {
        let held = vec![
            suggestion("a", 1, SuggestionState::Deleted),
            suggestion("b", 2, SuggestionState::Deleted),
            suggestion("c", 3, SuggestionState::Deleted),
        ];
        let merged = merge_suggestions(
            &held,
            vec![
                suggestion("a", 1, SuggestionState::Pending),
                suggestion("b", 2, SuggestionState::Sent),
            ],
        );
        assert_eq!(
            merged
                .iter()
                .map(|s| (s.id.as_str(), s.state))
                .collect::<Vec<_>>(),
            [
                ("a", SuggestionState::Deleted),
                ("b", SuggestionState::Sent),
                ("c", SuggestionState::Deleted)
            ]
        );
    }

    /// A share of Carol's, Alice suggesting and Bob viewing.
    fn audience() -> Audience {
        Audience::of_workspace(
            &WorkspaceShareRow {
                id: "w1".into(),
                owner: CommentAuthor {
                    name: "Carol".into(),
                    email: "carol@example.com".into(),
                },
                title: "Architecture".into(),
                created_at: 1,
                stopped_at: None,
                last_push_at: None,
                guests: vec![
                    Guest {
                        email: "alice@example.com".into(),
                        role: GuestRole::Suggest,
                    },
                    Guest {
                        email: "bob@example.com".into(),
                        role: GuestRole::Viewer,
                    },
                ],
                team_domain: None,
            },
            &std::collections::HashMap::new(),
        )
    }

    fn asked(chat: &str, message: &str, quote: &str, text: &str) -> NewShareComment {
        NewShareComment {
            chat: chat.into(),
            message: message.into(),
            quote: quote.into(),
            text: text.into(),
            ..NewShareComment::default()
        }
    }

    #[test]
    fn a_comment_is_on_a_passage_of_a_listed_chat_with_a_text() {
        let made = new_comment(
            &chats(),
            &[],
            asked("c1", "m1", " a monolith ", " Why not a service? "),
            &alice(),
            &audience(),
            7,
        )
        .unwrap();
        assert_eq!(
            (
                made.chat.as_str(),
                made.message.as_str(),
                made.quote.as_str(),
                made.text.as_str(),
                made.at,
                made.reply_to.as_deref()
            ),
            ("c1", "m1", "a monolith", "Why not a service?", 7, None)
        );
        let comment = |chat: &str, message: &str, quote: &str, text: &str| {
            new_comment(
                &chats(),
                &[],
                asked(chat, message, quote, text),
                &alice(),
                &audience(),
                7,
            )
        };
        assert!(comment("c9", "m1", "a", "b").is_err());
        assert!(comment("c1", "", "a", "b").is_err());
        assert!(comment("c1", "m1", " ", "b").is_err());
        assert!(comment("c1", "m1", "a", " ").is_err());
        let long = "x".repeat(QUOTE_MAX + 1);
        assert!(comment("c1", "m1", &long, "b").is_err());
        let long = "x".repeat(COMMENT_MAX + 1);
        assert!(comment("c1", "m1", "a", &long).is_err());
    }

    #[test]
    fn a_reply_joins_a_thread_s_first_comment_where_it_is() {
        let first = new_comment(
            &chats(),
            &[],
            asked("c1", "m1", "a monolith", "Why?"),
            &alice(),
            &audience(),
            7,
        )
        .unwrap();
        let carol = CommentAuthor {
            name: "Carol".into(),
            email: "carol@example.com".into(),
        };
        let reply = new_comment(
            &chats(),
            std::slice::from_ref(&first),
            NewShareComment {
                chat: "elsewhere".into(),
                text: "Because".into(),
                reply_to: Some(first.id.clone()),
                ..NewShareComment::default()
            },
            &carol,
            &audience(),
            8,
        )
        .unwrap();
        assert_eq!(
            (
                reply.chat.as_str(),
                reply.message.as_str(),
                reply.quote.as_str(),
                reply.reply_to.as_deref()
            ),
            ("c1", "m1", "", Some(first.id.as_str())),
            "a reply takes its thread's chat and message, and quotes nothing"
        );
        let to = |thread: &str| {
            new_comment(
                &chats(),
                &[first.clone(), reply.clone()],
                NewShareComment {
                    chat: "c1".into(),
                    text: "x".into(),
                    reply_to: Some(thread.into()),
                    ..NewShareComment::default()
                },
                &carol,
                &audience(),
                9,
            )
        };
        assert!(to(&reply.id).is_err(), "not to a reply");
        assert!(to("gone").is_err());
    }

    #[test]
    fn a_comment_mentions_the_owner_and_the_guests_viewers_too_never_its_author() {
        let mention = |email: &str| chauffeur_protocol::client::Mention {
            email: email.into(),
            name: "Whoever".into(),
        };
        let made = new_comment(
            &chats(),
            &[],
            NewShareComment {
                mentions: vec![
                    mention("Bob@example.com"),
                    mention("carol@example.com"),
                    mention("alice@example.com"),
                    mention("eve@example.com"),
                ],
                ..asked("c1", "m1", "a", "@Bob @Carol")
            },
            &alice(),
            &audience(),
            7,
        )
        .unwrap();
        assert_eq!(
            made.mentions
                .iter()
                .map(|m| (m.email.as_str(), m.name.as_str()))
                .collect::<Vec<_>>(),
            [("bob@example.com", "Bob"), ("carol@example.com", "Carol")]
        );
    }

    #[test]
    fn its_author_deletes_a_comment_and_a_thread_s_first_one_takes_its_replies() {
        let comment = |id: &str, author: CommentAuthor, reply_to: Option<&str>| ShareComment {
            id: id.into(),
            chat: "c1".into(),
            message: "m1".into(),
            quote: String::new(),
            author,
            text: "x".into(),
            at: 1,
            reply_to: reply_to.map(str::to_string),
            mentions: Vec::new(),
        };
        let carol = CommentAuthor {
            name: "Carol".into(),
            email: "carol@example.com".into(),
        };
        let comments = vec![
            comment("k1", alice(), None),
            comment("k2", carol.clone(), Some("k1")),
            comment("k3", alice(), Some("k1")),
            comment("k4", alice(), None),
        ];
        assert_eq!(
            delete_comment(&comments, "k1", &alice()).unwrap(),
            ["k1", "k2", "k3"]
        );
        assert_eq!(delete_comment(&comments, "k2", &carol).unwrap(), ["k2"]);
        assert!(delete_comment(&comments, "k2", &alice()).is_err());
        assert!(delete_comment(&comments, "nope", &alice()).is_err());
    }

    #[test]
    fn the_daemon_s_suggestions_win_and_pending_ones_it_lacks_stay() {
        let held = vec![
            suggestion("a", 1, SuggestionState::Pending),
            suggestion("b", 2, SuggestionState::Pending),
            suggestion("c", 3, SuggestionState::Dismissed),
        ];
        let merged = merge_suggestions(
            &held,
            vec![
                suggestion("a", 1, SuggestionState::Sent),
                suggestion("d", 0, SuggestionState::SentEdited),
            ],
        );
        assert_eq!(
            merged
                .iter()
                .map(|s| (s.id.as_str(), s.state))
                .collect::<Vec<_>>(),
            [
                ("d", SuggestionState::SentEdited),
                ("a", SuggestionState::Sent),
                ("b", SuggestionState::Pending)
            ]
        );
    }

    #[test]
    fn a_note_is_markdown_inside_the_notes_folder() {
        for good in ["plan.md", "adr/0001-split.MD"] {
            assert!(valid_note_path(good), "{good}");
        }
        for bad in [
            "",
            "/etc/x.md",
            "../x.md",
            "a/../x.md",
            "a//x.md",
            "notes.txt",
            "a\\x.md",
        ] {
            assert!(!valid_note_path(bad), "{bad}");
        }
        let big = SharedNote {
            path: "big.md".into(),
            text: "x".repeat(NOTE_MAX + 1),
            modified_ms: 1,
        };
        let fine = SharedNote {
            path: "plan.md".into(),
            text: "# Plan".into(),
            modified_ms: 1,
        };
        assert_eq!(valid_notes(vec![big, fine.clone()]), [fine]);
    }
}
