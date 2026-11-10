//! The user's mentions in the comments of shares open to them (docs/adr/mentions-in-share-comments.md).
//! While the user is signed in to Chauffeur and a share service is set, the daemon keeps one
//! WebSocket of its user's to the service (`/v1/mentions/daemon`): the mentions come whole at
//! connection and after each change, the threads the user opened from the app go back. Each
//! unread mention is told to the clients once, across restarts and reconnections: one that came
//! while the Mac slept is told when it is back.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use chauffeur_protocol::client::{MentionsResult, ShareConnection};
use chauffeur_protocol::design_share::{
    FromMentionsDaemon, Mentioned, SHARE_PROTOCOL_VERSION, ToMentionsDaemon,
};
use chauffeur_protocol::events::RegistryEvent;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{Notify, mpsc};
use tokio_tungstenite::tungstenite::Message;

use crate::{Daemon, Error, Result};

/// The mentions already told to the clients, by id: kept so that a restart tells none again.
const TOLD_KEY: &str = "mentions_told";
/// Ids kept in `TOLD_KEY`: more than the service ever lists at once.
const TOLD_MAX: usize = 500;
const MAX_BACKOFF: Duration = Duration::from_secs(60);

pub struct Mentions {
    daemon: Weak<Daemon>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The running link's stop, while there is one.
    stop: Option<Arc<Notify>>,
    /// Where the running link sends what the user opened.
    tx: Option<mpsc::UnboundedSender<FromMentionsDaemon>>,
    connection: Connection,
    mentions: Vec<Mentioned>,
    /// Opened from the app while the service had not heard of it yet: sent at each connection,
    /// and shown read meanwhile.
    pending_seen: HashSet<String>,
}

#[derive(Clone, Default, PartialEq)]
enum Connection {
    #[default]
    Off,
    Connecting,
    Online,
    Failed(String),
}

impl Connection {
    fn wire(&self) -> ShareConnection {
        match self {
            Connection::Off => ShareConnection::Off,
            Connection::Connecting => ShareConnection::Connecting,
            Connection::Online => ShareConnection::Online,
            Connection::Failed(message) => ShareConnection::Failed {
                message: message.clone(),
            },
        }
    }
}

impl Mentions {
    pub fn new(daemon: Weak<Daemon>) -> Mentions {
        Mentions {
            daemon,
            state: Mutex::new(State::default()),
        }
    }

    fn d(&self) -> Arc<Daemon> {
        self.daemon
            .upgrade()
            .expect("the daemon outlives its mentions")
    }

    /// `mentions.list`.
    pub fn list(&self) -> MentionsResult {
        let state = self.state.lock().unwrap();
        MentionsResult {
            mentions: state.mentions.clone(),
            connection: state.connection.wire(),
        }
    }

    /// `mentions.seen`: read at once here, and on the service as soon as it is reached.
    pub fn seen(&self, id: &str) -> Result<MentionsResult> {
        {
            let mut state = self.state.lock().unwrap();
            let Some(mention) = state.mentions.iter_mut().find(|m| m.id == id) else {
                return Err(Error::NotFound(format!("no mention {id}")));
            };
            mention.seen = true;
            state.pending_seen.insert(id.to_string());
            if let Some(tx) = &state.tx {
                let _ = tx.send(FromMentionsDaemon::Seen { id: id.to_string() });
            }
        }
        self.publish(Vec::new());
        Ok(self.list())
    }

    /// (Re)starts the link for the identity and settings in force: at the daemon's start and
    /// each time the user signs in or out. Signed out, or without a share service, it stops and
    /// the mentions are forgotten.
    pub fn restart(self: &Arc<Self>) {
        let d = self.d();
        let wanted = crate::identity::email(&d).is_some() && d.shares.base_url().is_some();
        let stop = Arc::new(Notify::new());
        let changed = {
            let mut state = self.state.lock().unwrap();
            if let Some(old) = state.stop.take() {
                old.notify_waiters();
            }
            let connection = if wanted {
                Connection::Connecting
            } else {
                Connection::Off
            };
            let changed = state.connection != connection || !state.mentions.is_empty();
            state.tx = None;
            state.mentions.clear();
            state.pending_seen.clear();
            state.connection = connection;
            if wanted {
                state.stop = Some(stop.clone());
            }
            changed
        };
        if changed {
            self.publish(Vec::new());
        }
        if wanted {
            let mentions = self.clone();
            tokio::spawn(async move { mentions.run(stop).await });
        }
    }

    fn publish(&self, new: Vec<String>) {
        let mentions = self.state.lock().unwrap().mentions.clone();
        self.d()
            .events
            .registry(RegistryEvent::MentionsChanged { mentions, new });
    }

    fn set_connection(&self, connection: Connection) {
        let changed = {
            let mut state = self.state.lock().unwrap();
            let changed = state.connection != connection;
            state.connection = connection;
            changed
        };
        if changed {
            self.publish(Vec::new());
        }
    }

    async fn run(self: Arc<Self>, stop: Arc<Notify>) {
        let mut backoff = Duration::from_secs(1);
        loop {
            self.set_connection(Connection::Connecting);
            let outcome = tokio::select! {
                outcome = self.session() => outcome,
                _ = stop.notified() => return,
            };
            match outcome {
                Ok(()) => {
                    backoff = Duration::from_secs(1);
                    self.set_connection(Connection::Connecting);
                }
                Err(message) => {
                    tracing::warn!(%message, "the mentions link failed");
                    self.set_connection(Connection::Failed(message));
                }
            }
            {
                let mut state = self.state.lock().unwrap();
                state.tx = None;
            }
            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = stop.notified() => return,
            }
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    }

    /// One connection: the Hello, what the app opened meanwhile, then the mentions as they
    /// change until it closes.
    async fn session(&self) -> std::result::Result<(), String> {
        let d = self.d();
        let base = d
            .shares
            .base_url()
            .ok_or("Sharing is off: set design_share.url in settings.toml")?;
        let token = d.shares.token().await?;
        let ws_base = base
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1);
        let (socket, _) = tokio_tungstenite::connect_async(format!("{ws_base}/v1/mentions/daemon"))
            .await
            .map_err(|e| format!("Could not reach the share service at {base}: {e}"))?;
        let (mut sink, mut stream) = socket.split();
        let say = |message: &FromMentionsDaemon| {
            Message::Text(
                serde_json::to_string(message)
                    .expect("mention messages serialize")
                    .into(),
            )
        };
        sink.send(say(&FromMentionsDaemon::Hello {
            id_token: token,
            version: SHARE_PROTOCOL_VERSION,
        }))
        .await
        .map_err(|e| format!("The share service closed: {e}"))?;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let pending: Vec<String> = {
            let mut state = self.state.lock().unwrap();
            state.tx = Some(tx);
            state.pending_seen.iter().cloned().collect()
        };
        for id in pending {
            sink.send(say(&FromMentionsDaemon::Seen { id }))
                .await
                .map_err(|e| format!("The share service closed: {e}"))?;
        }
        loop {
            tokio::select! {
                outgoing = rx.recv() => {
                    let Some(message) = outgoing else { return Ok(()) };
                    sink.send(say(&message))
                        .await
                        .map_err(|e| format!("The share service closed: {e}"))?;
                }
                incoming = stream.next() => {
                    let message = match incoming {
                        Some(Ok(Message::Text(text))) => {
                            match serde_json::from_str::<ToMentionsDaemon>(&text) {
                                Ok(message) => message,
                                Err(_) => continue,
                            }
                        }
                        Some(Ok(Message::Ping(payload))) => {
                            let _ = sink.send(Message::Pong(payload)).await;
                            continue;
                        }
                        Some(Ok(Message::Close(_))) | None => return Ok(()),
                        Some(Ok(_)) => continue,
                        Some(Err(e)) => return Err(format!("The share service closed: {e}")),
                    };
                    match message {
                        ToMentionsDaemon::Mentions { mentions } => self.receive(mentions),
                        ToMentionsDaemon::Refused { reason } => return Err(reason),
                    }
                }
            }
        }
    }

    /// The service's list, whole: kept, what the app opened meanwhile shown read, and the
    /// unread ones never told before told now.
    fn receive(&self, mut mentions: Vec<Mentioned>) {
        let d = self.d();
        let mut told: Vec<String> = d
            .store
            .kv_get(TOLD_KEY)
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        let new = {
            let mut state = self.state.lock().unwrap();
            settle_pending(&mut mentions, &mut state.pending_seen);
            state.mentions = mentions;
            state.connection = Connection::Online;
            arrived(&state.mentions, &mut told)
        };
        if !new.is_empty()
            && let Err(error) = d.store.kv_set(
                TOLD_KEY,
                &serde_json::to_string(&told).expect("ids serialize"),
            )
        {
            tracing::warn!(%error, "could not remember the mentions told");
        }
        self.publish(new);
    }
}

/// The unread mentions not in `told`, oldest first, added to it (the most recent `TOLD_MAX`
/// kept).
pub fn arrived(mentions: &[Mentioned], told: &mut Vec<String>) -> Vec<String> {
    let mut new: Vec<&Mentioned> = mentions
        .iter()
        .filter(|m| !m.seen && !told.contains(&m.id))
        .collect();
    new.sort_by_key(|m| m.at);
    let new: Vec<String> = new.into_iter().map(|m| m.id.clone()).collect();
    told.extend(new.iter().cloned());
    if told.len() > TOLD_MAX {
        told.drain(..told.len() - TOLD_MAX);
    }
    new
}

/// Shows read what the user opened before the service heard of it; forgets those the service
/// now says are read, or no longer lists.
pub fn settle_pending(mentions: &mut [Mentioned], pending: &mut HashSet<String>) {
    pending.retain(|id| mentions.iter().any(|m| &m.id == id && !m.seen));
    for mention in mentions.iter_mut() {
        if pending.contains(&mention.id) {
            mention.seen = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chauffeur_protocol::client::CommentAuthor;
    use chauffeur_protocol::design_share::MentionPlace;

    fn mention(id: &str, at: u64, seen: bool) -> Mentioned {
        Mentioned {
            id: id.into(),
            place: MentionPlace::Design,
            share_id: "s1".into(),
            share_title: "Pricing".into(),
            owner: CommentAuthor {
                name: "Alice".into(),
                email: "alice@example.com".into(),
            },
            url: "https://design.example/s/s1?thread=c1".into(),
            chat: None,
            comment: "c1".into(),
            thread: "c1".into(),
            screen: Some("a.html".into()),
            author: CommentAuthor {
                name: "Bob".into(),
                email: "bob@example.com".into(),
            },
            text: "@Alice look".into(),
            at,
            seen,
        }
    }

    #[test]
    fn an_unread_mention_is_told_once_and_a_read_one_never() {
        let mut told = Vec::new();
        let list = vec![
            mention("m2", 2, false),
            mention("m1", 1, false),
            mention("m0", 0, true),
        ];
        assert_eq!(arrived(&list, &mut told), ["m1", "m2"]);
        assert!(arrived(&list, &mut told).is_empty(), "told already");
        let more = vec![mention("m3", 3, false), mention("m2", 2, false)];
        assert_eq!(arrived(&more, &mut told), ["m3"]);
    }

    #[test]
    fn the_ids_told_are_bounded_to_the_most_recent() {
        let mut told: Vec<String> = (0..TOLD_MAX).map(|i| format!("old{i}")).collect();
        assert_eq!(arrived(&[mention("new", 1, false)], &mut told), ["new"]);
        assert_eq!(told.len(), TOLD_MAX);
        assert_eq!(told.last().map(String::as_str), Some("new"));
        assert!(!told.contains(&"old0".to_string()));
    }

    #[test]
    fn what_the_app_opened_shows_read_until_the_service_says_so() {
        let mut pending: HashSet<String> = ["m1".to_string(), "gone".to_string()].into();
        let mut list = vec![mention("m1", 1, false), mention("m2", 2, false)];
        settle_pending(&mut list, &mut pending);
        assert!(list[0].seen && !list[1].seen);
        assert_eq!(
            pending,
            ["m1".to_string()].into(),
            "one no longer listed is forgotten"
        );
        let mut read = vec![mention("m1", 1, true)];
        settle_pending(&mut read, &mut pending);
        assert!(pending.is_empty(), "the service has it");
    }
}
