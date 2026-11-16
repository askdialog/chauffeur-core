//! Mentions in the comments of a design share or a workspace share
//! (docs/adr/mentions-in-share-comments.md): who a comment may name with `@` (the people the share
//! is open to), what the service keeps of each mention, and the user's own WebSocket
//! (`/v1/mentions/daemon`) on which a signed-in daemon hears of theirs.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use chauffeur_protocol::client::{CommentAuthor, DesignComment, Mention};
use chauffeur_protocol::design_share::{
    CommentsDigest, FromMentionsDaemon, MentionPlace, Mentioned, SHARE_PROTOCOL_VERSION,
    ToMentionsDaemon, ToReviewer,
};
use chauffeur_protocol::workspace_share::{ShareComment, ToGuest};
use tokio::sync::mpsc;

use crate::store::{ShareRow, WantedDigest, WantedMention, WorkspaceShareRow};
use crate::{Identity, Service, first_name, now_ms};

/// Mentions one comment names, at most.
const MAX_MENTIONS: usize = 20;
/// Mentions a user hears of, at most: every unread one first, then the latest read.
const MAX_LISTED: usize = 50;
/// A mention's text, in characters, before it is cut.
const EXCERPT: usize = 280;
/// How long a share stays quiet before its owner is told of the comments that do not mention them.
const DIGEST_QUIET: Duration = Duration::from_secs(30 * 60);
/// A comment older than this when the service first sees it is not told: a share synced after a
/// long absence does not announce its history.
const DIGEST_MAX_AGE_MS: u64 = 24 * 60 * 60 * 1000;
/// How often the service looks for shares that have been quiet long enough.
const DIGEST_EVERY: Duration = Duration::from_secs(60);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const PING_EVERY: Duration = Duration::from_secs(20);
const MAX_MESSAGE: usize = 64 * 1024;

/// The people a share is open to, by name: who its comments may mention.
pub struct Audience {
    /// The owner first, then the reviewers in their order.
    people: Vec<CommentAuthor>,
    team_domain: Option<String>,
}

impl Audience {
    /// The owner, then the invitees; `names`: the names the service knows, by lowercase e-mail.
    fn new<'a>(
        owner: &CommentAuthor,
        invitees: impl Iterator<Item = &'a str>,
        team_domain: Option<&str>,
        names: &HashMap<String, String>,
    ) -> Audience {
        let owner = CommentAuthor {
            name: names
                .get(&owner.email.to_lowercase())
                .cloned()
                .unwrap_or_else(|| owner.name.clone()),
            email: owner.email.to_lowercase(),
        };
        let invitees: Vec<CommentAuthor> = invitees
            .map(str::to_lowercase)
            .filter(|email| *email != owner.email)
            .map(|email| CommentAuthor {
                name: names
                    .get(&email)
                    .cloned()
                    .unwrap_or_else(|| first_name(&email)),
                email,
            })
            .collect();
        Audience {
            people: std::iter::once(owner).chain(invitees).collect(),
            team_domain: team_domain.map(str::to_string),
        }
    }

    /// A design share's: its owner and reviewers.
    pub fn of(share: &ShareRow, names: &HashMap<String, String>) -> Audience {
        Audience::new(
            &share.owner,
            share.reviewers.iter().map(String::as_str),
            share.team_domain.as_deref(),
            names,
        )
    }

    /// A workspace share's: its owner and guests, viewers included (reading is enough to be
    /// mentioned).
    pub fn of_workspace(share: &WorkspaceShareRow, names: &HashMap<String, String>) -> Audience {
        Audience::new(
            &share.owner,
            share.guests.iter().map(|g| g.email.as_str()),
            share.team_domain.as_deref(),
            names,
        )
    }

    pub fn people(&self) -> &[CommentAuthor] {
        &self.people
    }

    /// Whether a comment may mention this address: the owner, a reviewer, or an address of the
    /// share's team domain.
    fn admits(&self, email: &str) -> bool {
        self.people.iter().any(|p| p.email == email)
            || self.team_domain.as_deref().is_some_and(|team| {
                email
                    .rsplit_once('@')
                    .is_some_and(|(_, domain)| domain == team)
            })
    }

    fn name_of(&self, email: &str) -> String {
        self.people
            .iter()
            .find(|p| p.email == email)
            .map_or_else(|| first_name(email), |p| p.name.clone())
    }

    /// The mentions a comment by `author` may keep: those the share is open to, never its
    /// author, each once, `MAX_MENTIONS` at most, named as the service knows them.
    pub fn vet(&self, mentions: &[Mention], author: &str) -> Vec<Mention> {
        let mut seen = HashSet::new();
        mentions
            .iter()
            .map(|m| m.email.trim().to_lowercase())
            .filter(|email| !email.eq_ignore_ascii_case(author))
            .filter(|email| self.admits(email))
            .filter(|email| seen.insert(email.clone()))
            .take(MAX_MENTIONS)
            .map(|email| Mention {
                name: self.name_of(&email),
                email,
            })
            .collect()
    }

    /// Each comment's mentions vetted, and pairs naming someone the share is no longer open to
    /// left out: what the share's mentions must be.
    pub fn wanted<C: Mentioning>(&self, comments: &[C]) -> Vec<WantedMention> {
        comments
            .iter()
            .flat_map(|comment| {
                self.vet(comment.mentions(), comment.author())
                    .into_iter()
                    .map(move |m| WantedMention {
                        comment_id: comment.id().to_string(),
                        thread_id: comment.thread().to_string(),
                        email: m.email,
                    })
            })
            .collect()
    }

    /// The comments the share's owner is to hear of once the share is quiet: those by someone
    /// else, recent, that do not mention the owner and that the owner has not answered in their
    /// thread since. A mention is told at once, by `wanted`.
    pub fn untagged<C: Mentioning>(&self, comments: &[C], now: u64) -> Vec<WantedDigest> {
        let owner = &self.people[0].email;
        let answered = |comment: &C| {
            comments.iter().any(|c| {
                c.author() == owner && c.thread() == comment.thread() && c.at() > comment.at()
            })
        };
        comments
            .iter()
            .filter(|c| c.author() != owner)
            .filter(|c| c.at() + DIGEST_MAX_AGE_MS >= now)
            .filter(|c| {
                !c.mentions()
                    .iter()
                    .any(|m| m.email.eq_ignore_ascii_case(owner))
            })
            .filter(|c| !answered(c))
            .map(|c| WantedDigest {
                comment_id: c.id().to_string(),
                author: c.author_name().to_string(),
            })
            .collect()
    }
}

/// A comment as far as its mentions go: a design's or a workspace's.
pub trait Mentioning {
    fn id(&self) -> &str;
    /// Its thread's first comment: itself when it opens one.
    fn thread(&self) -> &str;
    fn author(&self) -> &str;
    fn author_name(&self) -> &str;
    /// Milliseconds since the epoch.
    fn at(&self) -> u64;
    fn mentions(&self) -> &[Mention];
}

impl Mentioning for DesignComment {
    fn id(&self) -> &str {
        &self.id
    }
    fn thread(&self) -> &str {
        self.reply_to.as_deref().unwrap_or(&self.id)
    }
    fn author(&self) -> &str {
        &self.author.email
    }
    fn author_name(&self) -> &str {
        &self.author.name
    }
    fn at(&self) -> u64 {
        self.at
    }
    fn mentions(&self) -> &[Mention] {
        &self.mentions
    }
}

impl Mentioning for ShareComment {
    fn id(&self) -> &str {
        &self.id
    }
    fn thread(&self) -> &str {
        self.reply_to.as_deref().unwrap_or(&self.id)
    }
    fn author(&self) -> &str {
        &self.author.email
    }
    fn author_name(&self) -> &str {
        &self.author.name
    }
    fn at(&self) -> u64 {
        self.at
    }
    fn mentions(&self) -> &[Mention] {
        &self.mentions
    }
}

/// The daemon's comments, whole, as the service keeps them: each one's mentions vetted, and an
/// older daemon's (which drops the field) given back those it had while its text is the same.
pub fn from_daemon(
    incoming: Vec<DesignComment>,
    stored: &[DesignComment],
    audience: &Audience,
) -> Vec<DesignComment> {
    incoming
        .into_iter()
        .map(|mut comment| {
            if comment.mentions.is_empty()
                && let Some(before) = stored
                    .iter()
                    .find(|c| c.id == comment.id && c.text == comment.text)
            {
                comment.mentions = before.mentions.clone();
            }
            comment.mentions = audience.vet(&comment.mentions, &comment.author.email);
            comment
        })
        .collect()
}

fn excerpt(text: &str) -> String {
    let mut chars = text.chars();
    let cut: String = chars.by_ref().take(EXCERPT).collect();
    if chars.next().is_some() {
        format!("{}…", cut.trim_end())
    } else {
        cut
    }
}

/// A signed-in daemon listening for its user's mentions.
pub(crate) struct UserConnection {
    id: u64,
    identity: Identity,
    tx: mpsc::UnboundedSender<ToMentionsDaemon>,
}

impl Service {
    /// Who a share's comments may mention, with the names the service knows.
    pub(crate) fn audience(&self, share: &ShareRow) -> Audience {
        let emails: Vec<String> = std::iter::once(share.owner.email.clone())
            .chain(share.reviewers.iter().cloned())
            .collect();
        Audience::of(share, &self.store.names(&emails).unwrap_or_default())
    }

    /// Who a workspace share's comments may mention, with the names the service knows.
    pub(crate) fn workspace_audience(&self, share: &WorkspaceShareRow) -> Audience {
        let emails: Vec<String> = std::iter::once(share.owner.email.clone())
            .chain(share.guests.iter().map(|g| g.email.clone()))
            .collect();
        Audience::of_workspace(share, &self.store.names(&emails).unwrap_or_default())
    }

    /// Keeps the name of someone who signed in with Google; the pages of the live shares open
    /// to them hear of it when it changes the list of people.
    pub(crate) fn learn_name(&self, identity: &Identity) {
        if identity.name == first_name(&identity.email) {
            return;
        }
        if !self
            .store
            .remember_name(&identity.email, &identity.name, now_ms())
            .unwrap_or(false)
        {
            return;
        }
        let live: Vec<String> = self.live.lock().unwrap().keys().cloned().collect();
        for share_id in live {
            if let Ok(Some(share)) = self.store.share(&share_id)
                && (share.owner.email == identity.email || share.named(&identity.email))
            {
                self.tell_people(&share);
            }
        }
        let live: Vec<String> = self
            .workspace_live
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        for share_id in live {
            if let Ok(Some(share)) = self.store.workspace_share(&share_id)
                && (share.owner.email == identity.email
                    || share.guests.iter().any(|g| g.email == identity.email))
            {
                self.tell_workspace_people(&share);
            }
        }
    }

    /// Every page of the workspace share hears who it is open to now.
    pub(crate) fn tell_workspace_people(&self, share: &WorkspaceShareRow) {
        self.tell_guests(
            &share.id,
            &ToGuest::People {
                people: self.workspace_audience(share).people().to_vec(),
            },
        );
    }

    /// Every page of the share hears who it is open to now.
    pub(crate) fn tell_people(&self, share: &ShareRow) {
        self.tell_reviewers(
            &share.id,
            &ToReviewer::People {
                people: self.audience(share).people().to_vec(),
            },
        );
    }

    /// Makes the share's mentions what its comments say now, and tells each user whose changed.
    pub(crate) fn sync_mentions(&self, share_id: &str) {
        let Ok(Some(share)) = self.store.share(share_id) else {
            return;
        };
        let place = MentionPlace::Design;
        let changed = if share.stopped() {
            self.store.drop_digests(place, share_id).ok();
            self.store.drop_mentions(place, share_id)
        } else {
            let comments = self.store.comments(share_id).unwrap_or_default();
            let audience = self.audience(&share);
            let now = now_ms();
            if let Err(error) =
                self.store
                    .sync_digests(place, share_id, &audience.untagged(&comments, now), now)
            {
                tracing::warn!(share = share_id, %error, "comments to tell not synced");
            }
            self.store
                .sync_mentions(place, share_id, &audience.wanted(&comments), now)
        };
        match changed {
            Ok(emails) => self.tell_users(&emails),
            Err(error) => tracing::warn!(share = share_id, %error, "mentions not synced"),
        }
    }

    /// Makes a workspace share's mentions what its comments say now (those of a chat taken out
    /// of the share went with its comments), and tells each user whose changed.
    pub(crate) fn sync_workspace_mentions(&self, share_id: &str) {
        let Ok(Some(share)) = self.store.workspace_share(share_id) else {
            return;
        };
        let place = MentionPlace::Workspace;
        let changed = if share.stopped() {
            self.store.drop_digests(place, share_id).ok();
            self.store.drop_mentions(place, share_id)
        } else {
            let comments = self.store.workspace_comments(share_id).unwrap_or_default();
            let audience = self.workspace_audience(&share);
            let now = now_ms();
            if let Err(error) =
                self.store
                    .sync_digests(place, share_id, &audience.untagged(&comments, now), now)
            {
                tracing::warn!(share = share_id, %error, "comments to tell not synced");
            }
            self.store
                .sync_mentions(place, share_id, &audience.wanted(&comments), now)
        };
        match changed {
            Ok(emails) => self.tell_users(&emails),
            Err(error) => tracing::warn!(share = share_id, %error, "mentions not synced"),
        }
    }

    /// Someone opened a thread on a share's page: their mentions in it are read.
    pub(crate) fn see_thread(
        &self,
        place: MentionPlace,
        share_id: &str,
        thread: &str,
        who: &Identity,
    ) {
        if self
            .store
            .see_thread(place, share_id, thread, &who.email, now_ms())
            .unwrap_or(false)
        {
            self.tell_users(&HashSet::from([who.email.clone()]));
        }
    }

    /// Someone's mentions, as their daemon hears of them: of shares still open to them, every
    /// unread one first, then the latest read, newest first.
    pub(crate) fn mentions_for(&self, who: &Identity) -> Vec<Mentioned> {
        let rows = self
            .store
            .mentions_of(&who.email, MAX_LISTED)
            .unwrap_or_default();
        let mut designs: HashMap<String, Option<(ShareRow, Vec<DesignComment>)>> = HashMap::new();
        let mut workspaces: HashMap<String, Option<(WorkspaceShareRow, Vec<ShareComment>)>> =
            HashMap::new();
        let mut mentions: Vec<Mentioned> = Vec::new();
        for row in rows {
            let mentioned = match row.place {
                MentionPlace::Design => {
                    let entry = designs.entry(row.share_id.clone()).or_insert_with(|| {
                        let share = self.store.share(&row.share_id).ok().flatten()?;
                        if !share.can_view(who) {
                            return None;
                        }
                        let comments = self.store.comments(&share.id).ok()?;
                        Some((share, comments))
                    });
                    let Some((share, comments)) = entry else {
                        continue;
                    };
                    let Some(comment) = comments.iter().find(|c| c.id == row.comment_id) else {
                        continue;
                    };
                    Mentioned {
                        id: row.id,
                        place: MentionPlace::Design,
                        share_id: share.id.clone(),
                        share_title: share.title.clone(),
                        owner: share.owner.clone(),
                        url: format!("{}?thread={}", self.share_url(share), row.thread_id),
                        chat: None,
                        comment: row.comment_id,
                        thread: row.thread_id,
                        screen: Some(comment.screen.clone()),
                        author: comment.author.clone(),
                        text: excerpt(&comment.text),
                        at: row.at,
                        seen: row.seen,
                    }
                }
                MentionPlace::Workspace => {
                    let entry = workspaces.entry(row.share_id.clone()).or_insert_with(|| {
                        let share = self.store.workspace_share(&row.share_id).ok().flatten()?;
                        share.role_of(who)?;
                        let comments = self.store.workspace_comments(&share.id).ok()?;
                        Some((share, comments))
                    });
                    let Some((share, comments)) = entry else {
                        continue;
                    };
                    let Some(comment) = comments.iter().find(|c| c.id == row.comment_id) else {
                        continue;
                    };
                    Mentioned {
                        id: row.id,
                        place: MentionPlace::Workspace,
                        share_id: share.id.clone(),
                        share_title: share.title.clone(),
                        owner: share.owner.clone(),
                        url: format!(
                            "{}/w/{}?chat={}&comment={}",
                            self.workspace_url(),
                            share.id,
                            comment.chat,
                            row.thread_id
                        ),
                        chat: Some(comment.chat.clone()),
                        comment: row.comment_id,
                        thread: row.thread_id,
                        screen: None,
                        author: comment.author.clone(),
                        text: excerpt(&comment.text),
                        at: row.at,
                        seen: row.seen,
                    }
                }
            };
            mentions.push(mentioned);
        }
        // Unread first in what is kept, newest first in what is told.
        mentions.sort_by_key(|m| m.seen);
        mentions.truncate(MAX_LISTED);
        mentions.sort_by_key(|m| std::cmp::Reverse(m.at));
        mentions
    }

    /// Tells each owner whose share has been quiet for `DIGEST_QUIET` of the comments waiting for
    /// them, once, if a daemon of theirs is connected; else they wait for the next connection.
    pub(crate) fn tell_digests(&self) {
        let now = now_ms();
        let due = match self.store.due_digests(now, DIGEST_QUIET.as_millis() as u64) {
            Ok(due) => due,
            Err(error) => {
                tracing::warn!(%error, "comments to tell not read");
                return;
            }
        };
        for digest in due {
            let (owner, title, url) = match digest.place {
                MentionPlace::Design => {
                    let Ok(Some(share)) = self.store.share(&digest.share_id) else {
                        continue;
                    };
                    (
                        share.owner.email.clone(),
                        share.title.clone(),
                        self.share_url(&share),
                    )
                }
                MentionPlace::Workspace => {
                    let Ok(Some(share)) = self.store.workspace_share(&digest.share_id) else {
                        continue;
                    };
                    (
                        share.owner.email.clone(),
                        share.title.clone(),
                        format!("{}/w/{}", self.workspace_url(), share.id),
                    )
                }
            };
            let connections: Vec<mpsc::UnboundedSender<ToMentionsDaemon>> = self
                .users
                .lock()
                .unwrap()
                .get(&owner)
                .map(|c| c.iter().map(|c| c.tx.clone()).collect())
                .unwrap_or_default();
            if connections.is_empty() {
                continue;
            }
            let message = ToMentionsDaemon::Digest {
                digests: vec![CommentsDigest {
                    place: digest.place,
                    share_id: digest.share_id.clone(),
                    share_title: title,
                    url,
                    count: digest.count,
                    authors: digest.authors,
                }],
            };
            for tx in connections {
                let _ = tx.send(message.clone());
            }
            if let Err(error) = self.store.digested(digest.place, &digest.share_id, now) {
                tracing::warn!(share = digest.share_id, %error, "comments told not marked");
            }
        }
    }

    /// Looks every minute for shares quiet for long enough; for the service's whole life.
    pub fn watch_digests(self: &Arc<Self>) {
        let service = Arc::clone(self);
        tokio::spawn(async move {
            let mut every = tokio::time::interval(DIGEST_EVERY);
            loop {
                every.tick().await;
                service.tell_digests();
            }
        });
    }

    /// Each connected daemon of these users hears of their mentions, whole.
    pub(crate) fn tell_users(&self, emails: &HashSet<String>) {
        let connections: Vec<(Identity, mpsc::UnboundedSender<ToMentionsDaemon>)> = {
            let users = self.users.lock().unwrap();
            emails
                .iter()
                .filter_map(|email| users.get(email))
                .flatten()
                .map(|c| (c.identity.clone(), c.tx.clone()))
                .collect()
        };
        for (identity, tx) in connections {
            let _ = tx.send(ToMentionsDaemon::Mentions {
                mentions: self.mentions_for(&identity),
            });
        }
    }
}

fn text<T: serde::Serialize>(message: &T) -> Message {
    Message::Text(
        serde_json::to_string(message)
            .expect("messages serialize")
            .into(),
    )
}

/// A signed-in daemon: a Hello with its user's Google ID token first; then their mentions, whole,
/// at once and after each change; mentions read from the app come up.
pub async fn user_socket(service: Arc<Service>, mut socket: WebSocket) {
    let refused = |reason: String| text(&ToMentionsDaemon::Refused { reason });
    let hello = tokio::time::timeout(HELLO_TIMEOUT, socket.recv()).await;
    let Ok(Some(Ok(Message::Text(first)))) = hello else {
        let _ = socket.send(refused("Say hello first".into())).await;
        return;
    };
    let identity = match serde_json::from_str::<FromMentionsDaemon>(&first) {
        Ok(FromMentionsDaemon::Hello { id_token, version })
            if version == SHARE_PROTOCOL_VERSION =>
        {
            match service.identity_from_token(&id_token).await {
                Ok(identity) => identity,
                Err(reason) => {
                    let _ = socket.send(refused(reason)).await;
                    return;
                }
            }
        }
        Ok(FromMentionsDaemon::Hello { version, .. }) => {
            let _ = socket
                .send(refused(format!(
                    "This daemon speaks share protocol {version}; the service speaks {SHARE_PROTOCOL_VERSION}: update Chauffeur"
                )))
                .await;
            return;
        }
        _ => {
            let _ = socket.send(refused("Say hello first".into())).await;
            return;
        }
    };

    let id = service.next_connection();
    let (tx, mut rx) = mpsc::unbounded_channel::<ToMentionsDaemon>();
    service
        .users
        .lock()
        .unwrap()
        .entry(identity.email.clone())
        .or_default()
        .push(UserConnection {
            id,
            identity: identity.clone(),
            tx,
        });
    let first = ToMentionsDaemon::Mentions {
        mentions: service.mentions_for(&identity),
    };
    let connected = socket.send(text(&first)).await.is_ok();
    if connected {
        service.tell_digests();
        let mut ping = tokio::time::interval(PING_EVERY);
        ping.tick().await;
        loop {
            tokio::select! {
                outgoing = rx.recv() => {
                    let Some(message) = outgoing else { break };
                    if socket.send(text(&message)).await.is_err() {
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
                    if let Ok(FromMentionsDaemon::Seen { id }) = serde_json::from_str(&body)
                        && service.store.see_mention(&id, &identity.email, now_ms()).unwrap_or(false)
                    {
                        service.tell_users(&HashSet::from([identity.email.clone()]));
                    }
                }
            }
        }
    }
    let mut users = service.users.lock().unwrap();
    if let Some(connections) = users.get_mut(&identity.email) {
        connections.retain(|c| c.id != id);
        if connections.is_empty() {
            users.remove(&identity.email);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn share(team_domain: Option<&str>) -> ShareRow {
        ShareRow {
            id: "s1".into(),
            owner: CommentAuthor {
                name: "Carol".into(),
                email: "carol@example.com".into(),
            },
            title: "CTA".into(),
            created_at: 1,
            stopped_at: None,
            last_push_at: None,
            reviewers: vec!["alice@example.com".into(), "bob.l@other.com".into()],
            team_domain: team_domain.map(str::to_string),
            document: None,
        }
    }

    fn mention(email: &str, name: &str) -> Mention {
        Mention {
            email: email.into(),
            name: name.into(),
        }
    }

    fn comment(
        id: &str,
        author: &str,
        reply_to: Option<&str>,
        mentions: Vec<Mention>,
    ) -> DesignComment {
        DesignComment {
            id: id.into(),
            screen: "a.html".into(),
            quote: None,
            pin: None,
            author: CommentAuthor {
                name: author.into(),
                email: format!("{}@example.com", author.to_lowercase()),
            },
            at: 1,
            text: "look @Alice".into(),
            reply_to: reply_to.map(str::to_string),
            resolved: None,
            sent: None,
            mentions,
        }
    }

    #[test]
    fn the_people_are_the_owner_then_the_reviewers_by_their_known_names() {
        let names = HashMap::from([("alice@example.com".to_string(), "Alice Martin".to_string())]);
        let audience = Audience::of(&share(None), &names);
        assert_eq!(
            audience
                .people()
                .iter()
                .map(|p| (p.name.as_str(), p.email.as_str()))
                .collect::<Vec<_>>(),
            [
                ("Carol", "carol@example.com"),
                ("Alice Martin", "alice@example.com"),
                ("Bob", "bob.l@other.com"),
            ]
        );
    }

    #[test]
    fn a_comment_mentions_only_people_the_share_is_open_to_each_once_but_never_its_author() {
        let audience = Audience::of(&share(Some("example.com")), &HashMap::new());
        let vetted = audience.vet(
            &[
                mention("ALICE@example.com", "Whoever"),
                mention("alice@example.com", "Alice"),
                mention("carol@example.com", "Carol"),
                mention("dave@example.com", "Dave"),
                mention("eve@elsewhere.com", "Eve"),
                mention("bob.l@other.com", "B"),
            ],
            "Carol@example.com",
        );
        assert_eq!(
            vetted,
            [
                mention("alice@example.com", "Alice"),
                // Of the team domain: may be mentioned, named from the address.
                mention("dave@example.com", "Dave"),
                mention("bob.l@other.com", "Bob"),
            ]
        );
        // Without the team, only the people named on it.
        let closed = Audience::of(&share(None), &HashMap::new());
        assert!(
            closed
                .vet(&[mention("dave@example.com", "Dave")], "carol@example.com")
                .is_empty()
        );
        let many: Vec<Mention> = (0..30)
            .map(|i| mention(&format!("p{i}@example.com"), "P"))
            .collect();
        assert_eq!(audience.vet(&many, "carol@example.com").len(), MAX_MENTIONS);
    }

    #[test]
    fn a_reply_s_mentions_belong_to_its_thread() {
        let audience = Audience::of(&share(None), &HashMap::new());
        let wanted = audience.wanted(&[
            comment(
                "c1",
                "Carol",
                None,
                vec![mention("alice@example.com", "Alice")],
            ),
            comment(
                "c2",
                "Alice",
                Some("c1"),
                vec![
                    mention("carol@example.com", "Carol"),
                    mention("dave@example.com", "Dave"),
                ],
            ),
        ]);
        assert_eq!(
            wanted,
            [
                WantedMention {
                    comment_id: "c1".into(),
                    thread_id: "c1".into(),
                    email: "alice@example.com".into()
                },
                WantedMention {
                    comment_id: "c2".into(),
                    thread_id: "c1".into(),
                    email: "carol@example.com".into()
                },
            ]
        );
    }

    #[test]
    fn the_owner_hears_later_only_of_recent_comments_that_do_not_name_them_or_that_they_have_not_answered()
     {
        let audience = Audience::of(&share(None), &HashMap::new());
        let now = 10 * DIGEST_MAX_AGE_MS;
        let at = |mut c: DesignComment, at: u64| {
            c.at = at;
            c
        };
        let comments = [
            // Untagged, by someone else: told.
            at(comment("c1", "Alice", None, vec![]), now - 1_000),
            // Names the owner: told at once, not later.
            at(
                comment(
                    "c2",
                    "Alice",
                    None,
                    vec![mention("carol@example.com", "Carol")],
                ),
                now - 1_000,
            ),
            // The owner's own.
            at(comment("c3", "Carol", None, vec![]), now - 1_000),
            // Too old to announce.
            at(
                comment("c4", "Alice", None, vec![]),
                now - DIGEST_MAX_AGE_MS - 1,
            ),
            // Answered by the owner in its thread.
            at(comment("c5", "Alice", None, vec![]), now - 3_000),
            at(comment("c6", "Carol", Some("c5"), vec![]), now - 2_000),
            // Names someone else: still untagged for the owner.
            at(
                comment("c7", "Alice", None, vec![mention("bob.l@other.com", "Bob")]),
                now - 500,
            ),
        ];
        let ids: Vec<String> = audience
            .untagged(&comments, now)
            .into_iter()
            .map(|w| w.comment_id)
            .collect();
        assert_eq!(ids, ["c1", "c7"]);
    }

    #[test]
    fn an_older_daemon_s_comments_keep_the_mentions_of_an_unchanged_text() {
        let audience = Audience::of(&share(None), &HashMap::new());
        let stored = [comment(
            "c1",
            "Carol",
            None,
            vec![mention("alice@example.com", "Alice")],
        )];
        let kept = from_daemon(
            vec![comment("c1", "Carol", None, vec![])],
            &stored,
            &audience,
        );
        assert_eq!(kept[0].mentions, [mention("alice@example.com", "Alice")]);
        let mut edited = comment("c1", "Carol", None, vec![]);
        edited.text = "no one".into();
        assert!(
            from_daemon(vec![edited], &stored, &audience)[0]
                .mentions
                .is_empty()
        );
    }

    #[test]
    fn a_long_text_is_cut_with_an_ellipsis() {
        assert_eq!(excerpt("short"), "short");
        let long = "a".repeat(EXCERPT + 10);
        let cut = excerpt(&long);
        assert!(cut.ends_with('…') && cut.chars().count() == EXCERPT + 1);
    }
}
