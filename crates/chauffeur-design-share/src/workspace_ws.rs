//! The two WebSockets of a workspace share: the owner's daemon (the chats, their events, the
//! notes and the suggestions' fates come up it; guests' suggestions, their comments and who is
//! looking go down) and each guest's page (what changes comes down; a suggestion or a comment
//! goes up, when the guest may suggest, and their deletion by its author; a thread opened, which
//! reads the guest's mentions in it).

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use chauffeur_protocol::design_share::MentionPlace;
use chauffeur_protocol::workspace_share::{
    FromGuest, FromWorkspaceDaemon, GuestRole, NewShareComment, ShareComment, SuggestionState,
    ToGuest, ToWorkspaceDaemon, WORKSPACE_SHARE_PROTOCOL_VERSION,
};
use tokio::sync::mpsc;

use crate::workspace::{
    GuestConnection, delete_comment, delete_suggestion, merge_suggestions, new_comment,
    new_suggestion, shown_suggestions, valid_notes,
};
use crate::{Identity, Service, now_ms};

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const PING_EVERY: Duration = Duration::from_secs(20);
/// A long chat's history comes in one message.
pub const MAX_DAEMON_MESSAGE: usize = 32 * 1024 * 1024;
/// A guest only ever sends a suggestion, a comment or a deletion.
const MAX_GUEST_MESSAGE: usize = 256 * 1024;

fn text<T: serde::Serialize>(message: &T) -> Message {
    Message::Text(
        serde_json::to_string(message)
            .expect("messages serialize")
            .into(),
    )
}

/// The owner's daemon: a Hello with its Google ID token first, then what it pushes.
pub async fn daemon_socket(service: Arc<Service>, share_id: String, mut socket: WebSocket) {
    let refused = |reason: String| ToWorkspaceDaemon::Refused { reason };
    let hello = tokio::time::timeout(HELLO_TIMEOUT, socket.recv()).await;
    let Ok(Some(Ok(Message::Text(first)))) = hello else {
        let _ = socket.send(text(&refused("Say hello first".into()))).await;
        return;
    };
    let (identity, revision) = match serde_json::from_str::<FromWorkspaceDaemon>(&first) {
        Ok(FromWorkspaceDaemon::Hello {
            id_token,
            version,
            revision,
        }) if version == WORKSPACE_SHARE_PROTOCOL_VERSION => {
            match service.identity_from_token(&id_token).await {
                Ok(identity) => (identity, revision),
                Err(reason) => {
                    let _ = socket.send(text(&refused(reason))).await;
                    return;
                }
            }
        }
        Ok(FromWorkspaceDaemon::Hello { version, .. }) => {
            let _ = socket
                .send(text(&refused(format!(
                    "This daemon speaks workspace share protocol {version}; the service speaks {WORKSPACE_SHARE_PROTOCOL_VERSION}: update Chauffeur"
                ))))
                .await;
            return;
        }
        _ => {
            let _ = socket.send(text(&refused("Say hello first".into()))).await;
            return;
        }
    };
    let share = match service.store.workspace_share(&share_id) {
        Ok(Some(share)) if share.owner.email == identity.email && !share.stopped() => share,
        Ok(Some(share)) if share.stopped() => {
            let _ = socket
                .send(text(&refused("This share was stopped".into())))
                .await;
            return;
        }
        Ok(Some(_)) => {
            let _ = socket
                .send(text(&refused("This share is not yours".into())))
                .await;
            return;
        }
        _ => {
            let _ = socket
                .send(text(&refused("There is no such share".into())))
                .await;
            return;
        }
    };

    let id = service.next_connection();
    let (tx, mut rx) = mpsc::unbounded_channel::<ToWorkspaceDaemon>();
    service.with_workspace_live(&share_id, |live| {
        if let Some((_, old)) = live.daemon.replace((id, tx)) {
            let _ = old.send(ToWorkspaceDaemon::Refused {
                reason: "Another connection of this daemon took the share over".into(),
            });
        }
    });
    let welcome = ToWorkspaceDaemon::Welcome {
        share: service.workspace_status(&share),
        suggestions: service
            .store
            .workspace_suggestions(&share_id)
            .unwrap_or_default(),
        comments: service
            .store
            .workspace_comments(&share_id)
            .unwrap_or_default(),
    };
    let welcome = for_revision(welcome, revision).expect("a welcome is for every daemon");
    if socket.send(text(&welcome)).await.is_err() {
        leave_daemon(&service, &share_id, id);
        return;
    }
    service.tell_guests(
        &share_id,
        &ToGuest::Live {
            online: true,
            last_push_at: share.last_push_at,
        },
    );
    tracing::info!(
        share = share_id,
        owner = identity.email,
        "workspace daemon connected"
    );

    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    loop {
        tokio::select! {
            outgoing = rx.recv() => {
                let Some(message) = outgoing else { break };
                let Some(message) = for_revision(message, revision) else { continue };
                let last = matches!(message, ToWorkspaceDaemon::Refused { .. });
                if socket.send(text(&message)).await.is_err() || last {
                    break;
                }
            }
            _ = ping.tick() => {
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => {
                let Some(Ok(message)) = incoming else { break };
                let Message::Text(body) = message else { continue };
                let message = match serde_json::from_str::<FromWorkspaceDaemon>(&body) {
                    Ok(message) => message,
                    Err(error) => {
                        tracing::warn!(%error, share = share_id, "a daemon's message could not be read");
                        continue;
                    }
                };
                if let Err(error) = take(&service, &share_id, message) {
                    tracing::warn!(%error, share = share_id, "a daemon's push could not be kept");
                }
            }
        }
    }
    leave_daemon(&service, &share_id, id);
    let last_push_at = service
        .store
        .workspace_share(&share_id)
        .ok()
        .flatten()
        .and_then(|s| s.last_push_at);
    service.tell_guests(
        &share_id,
        &ToGuest::Live {
            online: false,
            last_push_at,
        },
    );
    tracing::info!(share = share_id, "workspace daemon left");
}

/// What a daemon of this revision can read of a message: a daemon older than deletions and
/// comments gets neither (a suggestion it holds stays with it; the service keeps it deleted); one
/// older than replies gets the threads' first comments only.
fn for_revision(message: ToWorkspaceDaemon, revision: u32) -> Option<ToWorkspaceDaemon> {
    if revision >= 2 {
        return Some(message);
    }
    let first_ones = |comments: Vec<ShareComment>| -> Vec<ShareComment> {
        comments
            .into_iter()
            .filter(|c| c.reply_to.is_none())
            .collect()
    };
    if revision == 1 {
        return Some(match message {
            ToWorkspaceDaemon::Welcome {
                share,
                suggestions,
                comments,
            } => ToWorkspaceDaemon::Welcome {
                share,
                suggestions,
                comments: first_ones(comments),
            },
            ToWorkspaceDaemon::Comments { comments } => ToWorkspaceDaemon::Comments {
                comments: first_ones(comments),
            },
            message => message,
        });
    }
    match message {
        ToWorkspaceDaemon::Welcome {
            share, suggestions, ..
        } => Some(ToWorkspaceDaemon::Welcome {
            share,
            suggestions: shown_suggestions(suggestions),
            comments: Vec::new(),
        }),
        ToWorkspaceDaemon::Suggestion { suggestion }
            if suggestion.state == SuggestionState::Deleted =>
        {
            None
        }
        ToWorkspaceDaemon::Comments { .. } => None,
        message => Some(message),
    }
}

/// Keeps what the daemon pushed and passes it on to the guests looking.
fn take(service: &Service, share_id: &str, message: FromWorkspaceDaemon) -> anyhow::Result<()> {
    let store = &service.store;
    match message {
        FromWorkspaceDaemon::Hello { .. } => return Ok(()),
        FromWorkspaceDaemon::Chats { chats } => {
            let dropped = store.set_workspace_chats(share_id, &chats)?;
            service.tell_guests(share_id, &ToGuest::Chats { chats });
            if dropped {
                let suggestions = store.workspace_suggestions(share_id)?;
                service.tell_guests(
                    share_id,
                    &ToGuest::Suggestions {
                        suggestions: shown_suggestions(suggestions),
                    },
                );
                // The mentions of a chat taken out go with its comments.
                service.workspace_comments_changed(share_id);
            }
        }
        FromWorkspaceDaemon::History { chat, events } => {
            store.replace_chat_events(share_id, &chat, &events)?;
            service.tell_guests(share_id, &ToGuest::History { chat });
        }
        FromWorkspaceDaemon::Events { chat, events } => {
            let kept = store.add_chat_events(share_id, &chat, &events)?;
            service.tell_guests(share_id, &ToGuest::Events { chat, events });
            // Text being written passes by many times a second: only what is kept is a push.
            if kept == 0 {
                return Ok(());
            }
        }
        FromWorkspaceDaemon::Notes { changed, removed } => {
            let changed = valid_notes(changed);
            store.change_workspace_notes(share_id, &changed, &removed)?;
            service.tell_guests(share_id, &ToGuest::Notes { changed, removed });
        }
        FromWorkspaceDaemon::Suggestions { suggestions } => {
            let held = store.workspace_suggestions(share_id)?;
            let suggestions = merge_suggestions(&held, suggestions);
            store.replace_suggestions(share_id, &suggestions)?;
            service.tell_guests(
                share_id,
                &ToGuest::Suggestions {
                    suggestions: shown_suggestions(suggestions),
                },
            );
        }
    }
    let now = now_ms();
    store.touch_workspace_push(share_id, now)?;
    service.tell_guests(
        share_id,
        &ToGuest::Live {
            online: true,
            last_push_at: Some(now),
        },
    );
    Ok(())
}

fn leave_daemon(service: &Service, share_id: &str, id: u64) {
    service.with_workspace_live(share_id, |live| {
        if live
            .daemon
            .as_ref()
            .is_some_and(|(current, _)| *current == id)
        {
            live.daemon = None;
        }
    });
}

/// A guest's page, already signed in and let in: the snapshot first, then what changes; their
/// suggestions go to everyone and to the owner's daemon.
pub async fn guest_socket(
    service: Arc<Service>,
    share_id: String,
    identity: Identity,
    mut socket: WebSocket,
) {
    let Ok((share, role)) = service.workspace_viewable(&share_id, &identity) else {
        return;
    };
    let Ok(snapshot) = service.workspace_snapshot(&share, &identity, role) else {
        return;
    };
    let id = service.next_connection();
    let (tx, mut rx) = mpsc::unbounded_channel::<ToGuest>();
    service.with_workspace_live(&share_id, |live| {
        live.guests.push(GuestConnection {
            id,
            identity: identity.clone(),
            tx,
        });
    });
    if socket
        .send(text(&ToGuest::Snapshot {
            snapshot: Box::new(snapshot),
        }))
        .await
        .is_err()
    {
        leave_guest(&service, &share_id, id);
        return;
    }
    service.announce_workspace_viewers(&share_id);

    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    loop {
        tokio::select! {
            outgoing = rx.recv() => {
                let Some(message) = outgoing else { break };
                let last = matches!(message, ToGuest::Refused { .. });
                if socket.send(text(&message)).await.is_err() || last {
                    break;
                }
            }
            _ = ping.tick() => {
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => {
                let Some(Ok(message)) = incoming else { break };
                let Message::Text(body) = message else { continue };
                if body.len() > MAX_GUEST_MESSAGE {
                    break;
                }
                let Ok(message) = serde_json::from_str::<FromGuest>(&body) else { continue };
                // The share may have been stopped, or the guest taken off or made a viewer, since.
                let Ok((_, role)) = service.workspace_viewable(&share_id, &identity) else {
                    let _ = socket.send(text(&ToGuest::Refused { reason: "This share is no longer open to you".into() })).await;
                    break;
                };
                if let Err(reason) = guest_says(&service, &share_id, role, &identity, message) {
                    // Refused for this message only: the page tells the guest and goes on.
                    let _ = socket.send(text(&ToGuest::Error { reason })).await;
                }
            }
        }
    }
    leave_guest(&service, &share_id, id);
    service.announce_workspace_viewers(&share_id);
}

/// A guest's suggestion, comment or deletion, kept and told to everyone looking and to the owner's
/// daemon, or a thread they opened; why not, for the guest.
fn guest_says(
    service: &Service,
    share_id: &str,
    role: GuestRole,
    identity: &Identity,
    message: FromGuest,
) -> Result<(), String> {
    let store = &service.store;
    let me = &identity.author();
    let not_kept = |what: &str| format!("The {what} could not be kept; try again");
    let suggestion = match message {
        FromGuest::Suggest { chat, text } => {
            let chats = store
                .workspace_chats(share_id)
                .map_err(|_| not_kept("suggestion"))?;
            new_suggestion(role, &chats, &chat, &text, me, now_ms())?
        }
        FromGuest::DeleteSuggestion { id } => {
            let held = store
                .workspace_suggestions(share_id)
                .map_err(|_| not_kept("deletion"))?;
            delete_suggestion(&held, &id, me)?
        }
        FromGuest::Comment {
            chat,
            message,
            quote,
            text,
            reply_to,
            mentions,
        } => {
            if role != GuestRole::Suggest {
                return Err(
                    "Only guests who can suggest comment: ask the owner to let you suggest".into(),
                );
            }
            let chats = store
                .workspace_chats(share_id)
                .map_err(|_| not_kept("comment"))?;
            let comments = store
                .workspace_comments(share_id)
                .map_err(|_| not_kept("comment"))?;
            let share = service
                .workspace_share(share_id)
                .map_err(|_| not_kept("comment"))?;
            let asked = NewShareComment {
                chat,
                message,
                quote,
                text,
                reply_to,
                mentions,
            };
            let audience = service.workspace_audience(&share);
            let comment = new_comment(&chats, &comments, asked, me, &audience, now_ms())?;
            store
                .add_comment(share_id, &comment)
                .map_err(|_| not_kept("comment"))?;
            service.workspace_comments_changed(share_id);
            return Ok(());
        }
        FromGuest::DeleteComment { id } => {
            let comments = store
                .workspace_comments(share_id)
                .map_err(|_| not_kept("deletion"))?;
            let gone = delete_comment(&comments, &id, me)?;
            store
                .remove_comments(share_id, &gone)
                .map_err(|_| not_kept("deletion"))?;
            service.workspace_comments_changed(share_id);
            return Ok(());
        }
        FromGuest::Seen { thread } => {
            service.see_thread(MentionPlace::Workspace, share_id, &thread, identity);
            return Ok(());
        }
    };
    store
        .upsert_suggestion(share_id, &suggestion)
        .map_err(|_| not_kept("suggestion"))?;
    if let Ok(suggestions) = store.workspace_suggestions(share_id) {
        service.tell_guests(
            share_id,
            &ToGuest::Suggestions {
                suggestions: shown_suggestions(suggestions),
            },
        );
    }
    service.tell_workspace_daemon(share_id, ToWorkspaceDaemon::Suggestion { suggestion });
    Ok(())
}

fn leave_guest(service: &Service, share_id: &str, id: u64) {
    service.with_workspace_live(share_id, |live| live.guests.retain(|g| g.id != id));
}
