//! The two WebSockets of a share: the owner's daemon (its comments and its file changes come
//! down it, reviewers' comments go up it) and each reviewer's page (what changes comes down,
//! their comments go up). A reviewer's change follows the daemon's rules for comments: the
//! author alone edits or removes one, until the owner sent it to the agent; anyone resolves.
//! A comment's mentions are kept to the people the share is open to (mentions.rs).

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use chauffeur_protocol::client::{CommentAuthor, CommentResolved, DesignComment};
use chauffeur_protocol::design_share::{
    FromDaemon, FromReviewer, MentionPlace, SHARE_PROTOCOL_VERSION, ToDaemon, ToReviewer,
};
use tokio::sync::mpsc;

use crate::mentions::{self, Audience};
use crate::{Identity, Service, now_ms, random_hex};

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const PING_EVERY: Duration = Duration::from_secs(20);
/// A design's comments, whole, fit in this; a file goes over HTTP.
const MAX_MESSAGE: usize = 4 * 1024 * 1024;
const TEXT_MAX: usize = 20_000;
/// Characters of a markdown file's passage a comment is on, at most (the daemon's limit too).
const QUOTE_MAX: usize = 2_000;

/// What a reviewer's message does to the comments.
#[derive(Debug, PartialEq)]
pub enum Change {
    Upsert(Box<DesignComment>),
    Remove(Vec<String>),
    /// The reader opened this thread: nothing changes but their mentions in it are read.
    Seen(String),
}

/// Applies a reviewer's message to the comments, by the daemon's rules; `audience` says who a
/// comment may mention.
pub fn apply(
    comments: &[DesignComment],
    message: FromReviewer,
    me: &CommentAuthor,
    audience: &Audience,
    now: u64,
) -> Result<Change, String> {
    let find = |id: &str| {
        comments
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| format!("no comment {id}"))
    };
    let changeable = |comment: &DesignComment| {
        if comment.author.email != me.email {
            return Err(format!(
                "only {} can change this comment",
                comment.author.name
            ));
        }
        if comment.sent.is_some() {
            return Err("this comment was sent to the agent: reply to it instead".into());
        }
        Ok(())
    };
    match message {
        FromReviewer::Add {
            screen,
            pin,
            quote,
            text,
            reply_to,
            mentions,
        } => {
            let text = text.trim();
            if text.is_empty() {
                return Err("a comment needs a text".into());
            }
            if text.len() > TEXT_MAX {
                return Err(format!("a comment is at most {TEXT_MAX} characters"));
            }
            if let Some(pin) = &pin
                && !((0.0..=100.0).contains(&pin.x) && (0.0..=100.0).contains(&pin.y))
            {
                return Err("a pin's x and y are percents of the screen, 0 to 100".into());
            }
            let quote = quote
                .map(|q| q.trim().to_string())
                .filter(|q| !q.is_empty() && reply_to.is_none());
            if quote
                .as_ref()
                .is_some_and(|q| q.chars().count() > QUOTE_MAX)
            {
                return Err(format!(
                    "a comment is on a passage of {QUOTE_MAX} characters at most: select less"
                ));
            }
            let (screen, pin) = match &reply_to {
                Some(parent_id) => {
                    let parent = find(parent_id)?;
                    if parent.reply_to.is_some() {
                        return Err("reply to the thread's first comment, not to a reply".into());
                    }
                    (parent.screen.clone(), None)
                }
                None => {
                    let screen = screen.trim_start_matches("./").to_string();
                    if screen.is_empty() {
                        return Err("a comment names a screen".into());
                    }
                    (screen, pin)
                }
            };
            Ok(Change::Upsert(Box::new(DesignComment {
                id: random_hex(8),
                screen,
                quote,
                pin,
                author: me.clone(),
                at: now,
                text: text.to_string(),
                reply_to,
                resolved: None,
                sent: None,
                mentions: audience.vet(&mentions, &me.email),
            })))
        }
        FromReviewer::Edit { id, text, mentions } => {
            let text = text.trim();
            if text.is_empty() {
                return Err("a comment needs a text".into());
            }
            let comment = find(&id)?;
            changeable(comment)?;
            Ok(Change::Upsert(Box::new(DesignComment {
                text: text.to_string(),
                mentions: mentions.map_or_else(
                    || comment.mentions.clone(),
                    |mentions| audience.vet(&mentions, &me.email),
                ),
                ..comment.clone()
            })))
        }
        FromReviewer::Delete { id } => {
            let comment = find(&id)?;
            changeable(comment)?;
            let replies: Vec<&DesignComment> = comments
                .iter()
                .filter(|c| c.reply_to.as_deref() == Some(&id))
                .collect();
            if replies.iter().any(|r| r.sent.is_some()) {
                return Err(
                    "a reply of this comment was sent to the agent: resolve it instead".into(),
                );
            }
            Ok(Change::Remove(
                std::iter::once(id.clone())
                    .chain(replies.iter().map(|r| r.id.clone()))
                    .collect(),
            ))
        }
        FromReviewer::Resolve { id, resolved } => {
            let comment = find(&id)?;
            if comment.reply_to.is_some() {
                return Err("a reply is resolved with its thread".into());
            }
            Ok(Change::Upsert(Box::new(DesignComment {
                resolved: resolved.then(|| CommentResolved {
                    by: me.email.clone(),
                    at: now,
                }),
                ..comment.clone()
            })))
        }
        FromReviewer::Seen { thread } => Ok(Change::Seen(thread)),
    }
}

fn text<T: serde::Serialize>(message: &T) -> Message {
    Message::Text(
        serde_json::to_string(message)
            .expect("messages serialize")
            .into(),
    )
}

/// The owner's daemon: a Hello with its Google ID token first, then its comments and its file
/// changes; reviewers' comments and who is viewing go back to it.
pub async fn daemon_socket(service: Arc<Service>, share_id: String, mut socket: WebSocket) {
    let hello = tokio::time::timeout(HELLO_TIMEOUT, socket.recv()).await;
    let Ok(Some(Ok(Message::Text(first)))) = hello else {
        let _ = socket
            .send(text(&ToDaemon::Refused {
                reason: "Say hello first".into(),
            }))
            .await;
        return;
    };
    let refused = |reason: String| ToDaemon::Refused { reason };
    let identity = match serde_json::from_str::<FromDaemon>(&first) {
        Ok(FromDaemon::Hello { id_token, version }) if version == SHARE_PROTOCOL_VERSION => {
            match service.identity_from_token(&id_token).await {
                Ok(identity) => identity,
                Err(reason) => {
                    let _ = socket.send(text(&refused(reason))).await;
                    return;
                }
            }
        }
        Ok(FromDaemon::Hello { version, .. }) => {
            let _ = socket
                .send(text(&refused(format!(
                    "This daemon speaks share protocol {version}; the service speaks {SHARE_PROTOCOL_VERSION}: update Chauffeur"
                ))))
                .await;
            return;
        }
        _ => {
            let _ = socket.send(text(&refused("Say hello first".into()))).await;
            return;
        }
    };
    let share = match service.store.share(&share_id) {
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
    let (tx, mut rx) = mpsc::unbounded_channel::<ToDaemon>();
    service.with_live(&share_id, |live| {
        if let Some((_, old)) = live.daemon.replace((id, tx)) {
            let _ = old.send(ToDaemon::Refused {
                reason: "Another connection of this daemon took the share over".into(),
            });
        }
    });
    let welcome = ToDaemon::Welcome {
        share: service.status(&share),
    };
    if socket.send(text(&welcome)).await.is_err() {
        leave_daemon(&service, &share_id, id);
        return;
    }
    service.tell_reviewers(
        &share_id,
        &ToReviewer::Live {
            online: true,
            last_push_at: share.last_push_at,
        },
    );
    tracing::info!(share = share_id, owner = identity.email, "daemon connected");

    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    loop {
        tokio::select! {
            outgoing = rx.recv() => {
                let Some(message) = outgoing else { break };
                let last = matches!(message, ToDaemon::Refused { .. });
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
                if body.len() > MAX_MESSAGE {
                    break;
                }
                let Ok(message) = serde_json::from_str::<FromDaemon>(&body) else { continue };
                match message {
                    FromDaemon::Hello { .. } => {}
                    FromDaemon::Comments { comments } => {
                        let Ok(Some(share)) = service.store.share(&share_id) else { continue };
                        let stored = service.store.comments(&share_id).unwrap_or_default();
                        let comments = mentions::from_daemon(comments, &stored, &service.audience(&share));
                        if service.store.replace_comments(&share_id, &comments).is_err() {
                            continue;
                        }
                        service.sync_mentions(&share_id);
                        let now = now_ms();
                        let _ = service.store.touch_push(&share_id, now);
                        if let Ok(Some(share)) = service.store.share(&share_id) {
                            // Each page's snapshot is its reader's: their render lease, the
                            // reviewers for the owner only. Built outside the lock it takes.
                            let pages: Vec<(Identity, mpsc::UnboundedSender<ToReviewer>)> = service
                                .with_live(&share_id, |live| {
                                    live.reviewers.iter().map(|r| (r.identity.clone(), r.tx.clone())).collect()
                                });
                            for (reader, tx) in pages {
                                if let Ok(snapshot) = service.snapshot(&share, &reader) {
                                    let _ = tx.send(ToReviewer::Snapshot { snapshot });
                                }
                            }
                        }
                    }
                    FromDaemon::Changed { changed, removed } => {
                        let now = now_ms();
                        let _ = service.store.touch_push(&share_id, now);
                        service.tell_reviewers(&share_id, &ToReviewer::Files { changed, removed });
                        service.tell_reviewers(&share_id, &ToReviewer::Live { online: true, last_push_at: Some(now) });
                    }
                }
            }
        }
    }
    leave_daemon(&service, &share_id, id);
    let last_push_at = service
        .store
        .share(&share_id)
        .ok()
        .flatten()
        .and_then(|s| s.last_push_at);
    service.tell_reviewers(
        &share_id,
        &ToReviewer::Live {
            online: false,
            last_push_at,
        },
    );
    tracing::info!(share = share_id, "daemon left");
}

fn leave_daemon(service: &Service, share_id: &str, id: u64) {
    service.with_live(share_id, |live| {
        if live
            .daemon
            .as_ref()
            .is_some_and(|(current, _)| *current == id)
        {
            live.daemon = None;
        }
    });
}

impl Service {
    fn announce_viewers(&self, share_id: &str) {
        let viewers = self.viewers(share_id);
        self.tell_reviewers(
            share_id,
            &ToReviewer::Viewers {
                viewers: viewers.clone(),
            },
        );
        self.tell_daemon(share_id, ToDaemon::Viewers { viewers });
    }
}

/// A reviewer's page, already signed in and named on the share: the snapshot first, then what
/// changes; their comments come up and go to everyone.
pub async fn reviewer_socket(
    service: Arc<Service>,
    share_id: String,
    identity: Identity,
    mut socket: WebSocket,
) {
    let Ok(Some(share)) = service.store.share(&share_id) else {
        return;
    };
    let Ok(snapshot) = service.snapshot(&share, &identity) else {
        return;
    };
    let id = service.next_connection();
    let (tx, mut rx) = mpsc::unbounded_channel::<ToReviewer>();
    service.with_live(&share_id, |live| {
        live.reviewers.push(crate::ReviewerConnection {
            id,
            identity: identity.clone(),
            tx,
        });
    });
    if socket
        .send(text(&ToReviewer::Snapshot { snapshot }))
        .await
        .is_err()
    {
        leave_reviewer(&service, &share_id, id);
        return;
    }
    service.announce_viewers(&share_id);
    let me = identity.author();

    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    loop {
        tokio::select! {
            outgoing = rx.recv() => {
                let Some(message) = outgoing else { break };
                let last = matches!(message, ToReviewer::Refused { .. });
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
                if body.len() > MAX_MESSAGE {
                    break;
                }
                let Ok(message) = serde_json::from_str::<FromReviewer>(&body) else { continue };
                // The share may have been stopped, or the reviewer taken off, since the snapshot.
                if service.viewable(&share_id, &identity).is_err() {
                    let _ = socket.send(text(&ToReviewer::Refused { reason: "This share is no longer open to you".into() })).await;
                    break;
                }
                let Ok(comments) = service.store.comments(&share_id) else { continue };
                let Ok(Some(share)) = service.store.share(&share_id) else { continue };
                match apply(&comments, message, &me, &service.audience(&share), now_ms()) {
                    Ok(Change::Upsert(comment)) => {
                        let comment = *comment;
                        if service.store.upsert_comment(&share_id, &comment).is_err() {
                            continue;
                        }
                        service.tell_reviewers(&share_id, &ToReviewer::Comment { comment: comment.clone() });
                        service.tell_daemon(&share_id, ToDaemon::Comment { comment });
                        service.sync_mentions(&share_id);
                    }
                    Ok(Change::Remove(ids)) => {
                        if service.store.delete_comments(&share_id, &ids).is_err() {
                            continue;
                        }
                        for id in ids {
                            service.tell_reviewers(&share_id, &ToReviewer::CommentRemoved { id: id.clone() });
                            service.tell_daemon(&share_id, ToDaemon::CommentRemoved { id });
                        }
                        service.sync_mentions(&share_id);
                    }
                    Ok(Change::Seen(thread)) => service.see_thread(MentionPlace::Design, &share_id, &thread, &identity),
                    Err(reason) => {
                        // Refused for this message only: the page tells the reviewer and goes on.
                        let _ = socket.send(text(&ToReviewer::Error { reason })).await;
                    }
                }
            }
        }
    }
    leave_reviewer(&service, &share_id, id);
    service.announce_viewers(&share_id);
}

fn leave_reviewer(service: &Service, share_id: &str, id: u64) {
    service.with_live(share_id, |live| live.reviewers.retain(|r| r.id != id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use chauffeur_protocol::client::{CommentPin, CommentSent};

    fn alice() -> CommentAuthor {
        CommentAuthor {
            name: "Alice".into(),
            email: "alice@example.com".into(),
        }
    }

    fn carol() -> CommentAuthor {
        CommentAuthor {
            name: "Carol".into(),
            email: "carol@example.com".into(),
        }
    }

    fn thread(id: &str, author: CommentAuthor, sent: bool) -> DesignComment {
        DesignComment {
            id: id.into(),
            screen: "a.html".into(),
            quote: None,
            pin: None,
            author,
            at: 1,
            text: "first".into(),
            reply_to: None,
            resolved: None,
            sent: sent.then_some(CommentSent { at: 2 }),
            mentions: Vec::new(),
        }
    }

    /// A share of Carol's, named to Alice.
    fn audience() -> Audience {
        Audience::of(
            &crate::store::ShareRow {
                id: "s1".into(),
                owner: carol(),
                title: "CTA".into(),
                created_at: 1,
                stopped_at: None,
                last_push_at: None,
                reviewers: vec!["alice@example.com".into()],
                team_domain: None,
                document: None,
            },
            &std::collections::HashMap::new(),
        )
    }

    #[test]
    fn a_reviewer_s_passage_stays_on_a_thread_of_a_markdown_file_never_on_a_reply() {
        let add = |quote: &str, reply_to: Option<&str>| {
            apply(
                &[thread("t1", carol(), true)],
                FromReviewer::Add {
                    screen: "plan.md".into(),
                    pin: None,
                    quote: Some(quote.into()),
                    text: "why?".into(),
                    reply_to: reply_to.map(str::to_string),
                    mentions: Vec::new(),
                },
                &alice(),
                &audience(),
                5,
            )
        };
        let Ok(Change::Upsert(thread)) = add("  Ship it ", None) else {
            panic!("a thread on a passage")
        };
        assert_eq!(thread.quote.as_deref(), Some("Ship it"));
        let Ok(Change::Upsert(reply)) = add("Ship it", Some("t1")) else {
            panic!("a reply")
        };
        assert_eq!(reply.quote, None);
        assert!(add(&"x".repeat(QUOTE_MAX + 1), None).is_err());
    }

    #[test]
    fn a_reviewer_adds_a_thread_or_a_reply_on_the_thread_s_screen() {
        let existing = vec![thread("t1", carol(), true)];
        let added = apply(
            &existing,
            FromReviewer::Add {
                screen: "./b.html".into(),
                pin: Some(CommentPin {
                    x: 10.0,
                    y: 20.0,
                    element: None,
                }),
                quote: None,
                text: " too low ".into(),
                reply_to: None,
                mentions: Vec::new(),
            },
            &alice(),
            &audience(),
            5,
        )
        .unwrap();
        let Change::Upsert(comment) = added else {
            panic!("{added:?}")
        };
        assert_eq!(
            (comment.screen.as_str(), comment.text.as_str(), comment.at),
            ("b.html", "too low", 5)
        );
        assert_eq!(comment.author, alice());
        let Change::Upsert(reply) = apply(
            &existing,
            FromReviewer::Add {
                screen: "".into(),
                pin: None,
                quote: None,
                text: "seen".into(),
                reply_to: Some("t1".into()),
                mentions: Vec::new(),
            },
            &alice(),
            &audience(),
            6,
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(
            (reply.screen.as_str(), reply.reply_to.as_deref()),
            ("a.html", Some("t1"))
        );
        assert!(
            apply(
                &existing,
                FromReviewer::Add {
                    screen: "a.html".into(),
                    pin: None,
                    quote: None,
                    text: " ".into(),
                    reply_to: None,
                    mentions: Vec::new(),
                },
                &alice(),
                &audience(),
                6
            )
            .is_err()
        );
        assert!(
            apply(
                &existing,
                FromReviewer::Add {
                    screen: "a.html".into(),
                    pin: None,
                    quote: None,
                    text: "x".into(),
                    reply_to: Some("nope".into()),
                    mentions: Vec::new(),
                },
                &alice(),
                &audience(),
                6
            )
            .is_err()
        );
        assert!(
            apply(
                &existing,
                FromReviewer::Add {
                    screen: "a.html".into(),
                    pin: Some(CommentPin {
                        x: 101.0,
                        y: 0.0,
                        element: None
                    }),
                    quote: None,
                    text: "x".into(),
                    reply_to: None,
                    mentions: Vec::new(),
                },
                &alice(),
                &audience(),
                6
            )
            .is_err()
        );
    }

    #[test]
    fn only_the_author_edits_or_removes_until_sent_and_anyone_resolves() {
        let comments = vec![
            thread("mine", alice(), false),
            thread("sent", alice(), true),
            thread("theirs", carol(), false),
        ];
        let Change::Upsert(edited) = apply(
            &comments,
            FromReviewer::Edit {
                id: "mine".into(),
                text: "better".into(),
                mentions: None,
            },
            &alice(),
            &audience(),
            9,
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(edited.text, "better");
        assert!(
            apply(
                &comments,
                FromReviewer::Edit {
                    id: "sent".into(),
                    text: "x".into(),
                    mentions: None,
                },
                &alice(),
                &audience(),
                9
            )
            .is_err()
        );
        assert!(
            apply(
                &comments,
                FromReviewer::Edit {
                    id: "theirs".into(),
                    text: "x".into(),
                    mentions: None,
                },
                &alice(),
                &audience(),
                9
            )
            .is_err()
        );
        assert_eq!(
            apply(
                &comments,
                FromReviewer::Delete { id: "mine".into() },
                &alice(),
                &audience(),
                9
            )
            .unwrap(),
            Change::Remove(vec!["mine".into()])
        );
        assert!(
            apply(
                &comments,
                FromReviewer::Delete {
                    id: "theirs".into()
                },
                &alice(),
                &audience(),
                9
            )
            .is_err()
        );
        let Change::Upsert(resolved) = apply(
            &comments,
            FromReviewer::Resolve {
                id: "theirs".into(),
                resolved: true,
            },
            &alice(),
            &audience(),
            9,
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(
            resolved.resolved.as_ref().map(|r| r.by.as_str()),
            Some("alice@example.com")
        );
    }

    #[test]
    fn removing_a_thread_takes_its_replies_unless_one_was_sent() {
        let mut reply = thread("r1", carol(), false);
        reply.reply_to = Some("mine".into());
        let comments = vec![thread("mine", alice(), false), reply.clone()];
        assert_eq!(
            apply(
                &comments,
                FromReviewer::Delete { id: "mine".into() },
                &alice(),
                &audience(),
                9
            )
            .unwrap(),
            Change::Remove(vec!["mine".into(), "r1".into()])
        );
        reply.sent = Some(CommentSent { at: 3 });
        let comments = vec![thread("mine", alice(), false), reply];
        assert!(
            apply(
                &comments,
                FromReviewer::Delete { id: "mine".into() },
                &alice(),
                &audience(),
                9
            )
            .is_err()
        );
        assert!(
            apply(
                &comments,
                FromReviewer::Resolve {
                    id: "r1".into(),
                    resolved: true
                },
                &alice(),
                &audience(),
                9
            )
            .is_err()
        );
    }
}
