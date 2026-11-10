//! Mentions end to end, on loopback with `dev:` identities: the people a share is open to see
//! each other, a comment names some of them, each hears of it on their daemon's own socket, and
//! it is read once its thread is opened (docs/adr/mentions-in-share-comments.md). On a design's
//! comments, and on a shared workspace's, where the owner comments from their daemon and anyone
//! who comments replies.

use std::sync::Arc;
use std::time::Duration;

use chauffeur_design_share::{Auth, Config, Service, data_paths, store::Store};
use chauffeur_protocol::client::{CommentAuthor, DesignComment, Mention};
use chauffeur_protocol::design_share::{
    FromDaemon, FromMentionsDaemon, FromReviewer, MentionPlace, Mentioned, SHARE_PROTOCOL_VERSION,
    ShareSnapshot, ShareStatus, ToDaemon, ToMentionsDaemon, ToReviewer,
};
use chauffeur_protocol::model::{AgentKind, ChatKind, ChatStatus};
use chauffeur_protocol::workspace_share::{
    FromGuest, FromWorkspaceDaemon, NewShareComment, ShareComment, SharedChat, ToGuest,
    ToWorkspaceDaemon, WORKSPACE_SHARE_PROTOCOL_REVISION, WORKSPACE_SHARE_PROTOCOL_VERSION,
    WorkspaceShareSnapshot, WorkspaceShareStatus,
};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

struct Server {
    _dir: tempfile::TempDir,
    base: String,
    ws: String,
    _service: Arc<Service>,
}

async fn server() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let (db, files) = data_paths(dir.path());
    let service = Service::new(
        Store::open(&db).unwrap(),
        files,
        Auth::InsecureDev,
        Config {
            public_url: "https://design.example".into(),
            workspace_url: None,
            cookie_domain: None,
            member_domain: Some("example.com".into()),
            site_url: None,
            session_key: None,
            web_client_id: None,
            web_dir: None,
            secure_cookies: false,
            mailer: chauffeur_design_share::mail::Mailer::Off,
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = service.router();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Server {
        _dir: dir,
        ws: base.replace("http://", "ws://"),
        base,
        _service: service,
    }
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(url: &str, cookie: Option<&str>) -> Socket {
    let mut request = url.into_client_request().unwrap();
    if let Some(cookie) = cookie {
        request
            .headers_mut()
            .insert("cookie", cookie.parse().unwrap());
    }
    tokio_tungstenite::connect_async(request).await.unwrap().0
}

async fn next<T: serde::de::DeserializeOwned>(socket: &mut Socket) -> T {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("a message in time")
            .expect("the socket open")
            .unwrap();
        if let Message::Text(text) = message {
            return serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
        }
    }
}

/// The first message of a page that `pick` takes, the others skipped.
async fn until<T>(socket: &mut Socket, pick: impl Fn(ToReviewer) -> Option<T>) -> T {
    loop {
        if let Some(found) = pick(next::<ToReviewer>(socket).await) {
            return found;
        }
    }
}

async fn session_cookie(base: &str, email: &str) -> String {
    let r = reqwest::Client::new()
        .post(format!("{base}/v1/session"))
        .json(&serde_json::json!({"id_token": format!("dev:{email}")}))
        .send()
        .await
        .unwrap();
    r.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

fn say<T: serde::Serialize>(message: &T) -> Message {
    Message::Text(serde_json::to_string(message).unwrap().into())
}

fn mention(email: &str) -> Mention {
    Mention {
        email: email.into(),
        name: "whatever the client said".into(),
    }
}

/// A user's daemon, listening for their mentions: what it hears first.
async fn user(s: &Server, email: &str) -> (Socket, Vec<Mentioned>) {
    let mut socket = connect(&format!("{}/v1/mentions/daemon", s.ws), None).await;
    socket
        .send(say(&FromMentionsDaemon::Hello {
            id_token: format!("dev:{email}"),
            version: SHARE_PROTOCOL_VERSION,
        }))
        .await
        .unwrap();
    let ToMentionsDaemon::Mentions { mentions } = next(&mut socket).await else {
        panic!()
    };
    (socket, mentions)
}

async fn mentions(socket: &mut Socket) -> Vec<Mentioned> {
    let ToMentionsDaemon::Mentions { mentions } = next(socket).await else {
        panic!()
    };
    mentions
}

async fn page(s: &Server, share: &str, email: &str) -> (Socket, ShareSnapshot) {
    let cookie = session_cookie(&s.base, email).await;
    let mut socket = connect(
        &format!("{}/v1/shares/{share}/reviewer", s.ws),
        Some(&cookie),
    )
    .await;
    let ToReviewer::Snapshot { snapshot } = next(&mut socket).await else {
        panic!()
    };
    (socket, snapshot)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mention_reaches_the_daemon_of_whom_it_names_until_they_open_its_thread() {
    let s = server().await;
    let http = reqwest::Client::new();
    let carol = "Bearer dev:carol@example.com";
    let share: ShareStatus = http
        .post(format!("{}/v1/shares", s.base))
        .header("authorization", carol)
        .json(&serde_json::json!({"title": "CTA", "reviewers": ["alice@example.com", "bob@example.com"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let everyone = |names: &[(&str, &str)]| -> Vec<CommentAuthor> {
        names
            .iter()
            .map(|(name, email)| CommentAuthor {
                name: (*name).into(),
                email: (*email).into(),
            })
            .collect()
    };
    assert_eq!(
        share.people,
        everyone(&[
            ("Carol", "carol@example.com"),
            ("Alice", "alice@example.com"),
            ("Bob", "bob@example.com"),
        ])
    );

    // A reviewer sees who else the share is open to, and nothing unread yet.
    let (mut alice_page, snapshot) = page(&s, &share.id, "alice@example.com").await;
    assert_eq!(snapshot.people, share.people);
    assert!(snapshot.reviewers.is_empty() && snapshot.unread_mentions.is_empty());
    let (mut alice, first) = user(&s, "alice@example.com").await;
    assert!(first.is_empty());
    let (mut carol_user, _) = user(&s, "carol@example.com").await;

    // The owner's daemon sends a thread naming Alice and Dave, whom the share is not open to.
    let mut daemon = connect(&format!("{}/v1/shares/{}/daemon", s.ws, share.id), None).await;
    daemon
        .send(say(&FromDaemon::Hello {
            id_token: "dev:carol@example.com".into(),
            version: SHARE_PROTOCOL_VERSION,
        }))
        .await
        .unwrap();
    let ToDaemon::Welcome { .. } = next(&mut daemon).await else {
        panic!()
    };
    let thread = DesignComment {
        id: "c1".into(),
        screen: "a.html".into(),
        quote: None,
        pin: None,
        author: CommentAuthor {
            name: "Carol".into(),
            email: "carol@example.com".into(),
        },
        at: 10,
        text: "@Alice @Dave too low?".into(),
        reply_to: None,
        resolved: None,
        sent: None,
        mentions: vec![mention("alice@example.com"), mention("dave@example.com")],
    };
    daemon
        .send(say(&FromDaemon::Comments {
            comments: vec![thread.clone()],
        }))
        .await
        .unwrap();
    let told = mentions(&mut alice).await;
    assert_eq!(told.len(), 1);
    let m = &told[0];
    assert_eq!(
        (
            m.comment.as_str(),
            m.thread.as_str(),
            m.screen.as_deref(),
            m.seen
        ),
        ("c1", "c1", Some("a.html"), false)
    );
    assert_eq!(
        m.url,
        format!("https://design.example/s/{}?thread=c1", share.id)
    );
    assert_eq!(
        (m.share_title.as_str(), m.author.email.as_str()),
        ("CTA", "carol@example.com")
    );
    let snapshot = until(&mut alice_page, |m| match m {
        ToReviewer::Snapshot { snapshot } => Some(snapshot),
        _ => None,
    })
    .await;
    assert_eq!(snapshot.unread_mentions, ["c1"]);
    assert_eq!(
        snapshot.comments[0].mentions,
        [Mention {
            email: "alice@example.com".into(),
            name: "Alice".into()
        }]
    );

    // An older daemon sends the same comment without the field: the mention stays.
    let mut old = thread.clone();
    old.mentions.clear();
    daemon
        .send(say(&FromDaemon::Comments {
            comments: vec![old],
        }))
        .await
        .unwrap();
    let snapshot = until(&mut alice_page, |m| match m {
        ToReviewer::Snapshot { snapshot } => Some(snapshot),
        _ => None,
    })
    .await;
    assert_eq!(snapshot.unread_mentions, ["c1"]);

    // Bob replies, naming Carol and Alice: both hear of it.
    let (mut bob_page, _) = page(&s, &share.id, "bob@example.com").await;
    bob_page
        .send(say(&FromReviewer::Add {
            screen: String::new(),
            pin: None,
            quote: None,
            text: "@Carol @Alice agreed".into(),
            reply_to: Some("c1".into()),
            mentions: vec![mention("carol@example.com"), mention("alice@example.com")],
        }))
        .await
        .unwrap();
    let reply = until(&mut bob_page, |m| match m {
        ToReviewer::Comment { comment } => Some(comment),
        _ => None,
    })
    .await;
    assert_eq!(mentions(&mut alice).await.len(), 2);
    let carols = mentions(&mut carol_user).await;
    assert_eq!(carols.len(), 1);
    assert_eq!(
        (carols[0].thread.as_str(), carols[0].author.email.as_str()),
        ("c1", "bob@example.com")
    );

    // Alice opens the thread on her page: both of hers are read.
    alice_page
        .send(say(&FromReviewer::Seen {
            thread: "c1".into(),
        }))
        .await
        .unwrap();
    let read = mentions(&mut alice).await;
    assert!(read.len() == 2 && read.iter().all(|m| m.seen));

    // Bob edits Alice out of his reply: hers goes.
    bob_page
        .send(say(&FromReviewer::Edit {
            id: reply.id.clone(),
            text: "@Carol agreed".into(),
            mentions: Some(vec![mention("carol@example.com")]),
        }))
        .await
        .unwrap();
    let left = mentions(&mut alice).await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].comment, "c1");

    // Carol reads hers from the app; someone else's id does nothing.
    alice
        .send(say(&FromMentionsDaemon::Seen {
            id: carols[0].id.clone(),
        }))
        .await
        .unwrap();
    carol_user
        .send(say(&FromMentionsDaemon::Seen {
            id: carols[0].id.clone(),
        }))
        .await
        .unwrap();
    let carols = mentions(&mut carol_user).await;
    assert!(carols[0].seen);

    // Alice is taken off the share: her page is refused, her mentions go, Bob hears who is left.
    let patched: ShareStatus = http
        .patch(format!("{}/v1/shares/{}", s.base, share.id))
        .header("authorization", carol)
        .json(&serde_json::json!({"reviewers": ["bob@example.com"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        patched.people,
        everyone(&[("Carol", "carol@example.com"), ("Bob", "bob@example.com")])
    );
    assert!(mentions(&mut alice).await.is_empty());
    let people = until(&mut bob_page, |m| match m {
        ToReviewer::People { people } => Some(people),
        _ => None,
    })
    .await;
    assert_eq!(people, patched.people);
    until(&mut alice_page, |m| match m {
        ToReviewer::Refused { .. } => Some(()),
        _ => None,
    })
    .await;

    // Stopped: Carol's mention goes too.
    http.delete(format!("{}/v1/shares/{}", s.base, share.id))
        .header("authorization", carol)
        .send()
        .await
        .unwrap();
    assert!(mentions(&mut carol_user).await.is_empty());
}

async fn guest_page(s: &Server, share: &str, email: &str) -> (Socket, WorkspaceShareSnapshot) {
    let cookie = session_cookie(&s.base, email).await;
    let mut socket = connect(
        &format!("{}/v1/workspace-shares/{share}/guest", s.ws),
        Some(&cookie),
    )
    .await;
    let ToGuest::Snapshot { snapshot } = next(&mut socket).await else {
        panic!()
    };
    (socket, *snapshot)
}

/// A workspace share's daemon of this revision, its chats pushed: what it hears, from the welcome.
async fn workspace_daemon(s: &Server, share: &str, revision: u32, chats: &[&str]) -> Socket {
    let mut socket = connect(
        &format!("{}/v1/workspace-shares/{share}/daemon", s.ws),
        None,
    )
    .await;
    socket
        .send(say(&FromWorkspaceDaemon::Hello {
            id_token: "dev:carol@example.com".into(),
            version: WORKSPACE_SHARE_PROTOCOL_VERSION,
            revision,
        }))
        .await
        .unwrap();
    let ToWorkspaceDaemon::Welcome { .. } = next(&mut socket).await else {
        panic!()
    };
    let chats = chats
        .iter()
        .map(|id| SharedChat {
            id: id.to_string(),
            title: None,
            kind: ChatKind::Chat,
            agent: AgentKind::Claude,
            status: ChatStatus::Idle,
            created_at: "2026-11-09T10:00:00Z".into(),
        })
        .collect();
    socket
        .send(say(&FromWorkspaceDaemon::Chats { chats }))
        .await
        .unwrap();
    socket
}

/// The comments a daemon hears next.
async fn daemon_comments(socket: &mut Socket) -> Vec<ShareComment> {
    loop {
        if let ToWorkspaceDaemon::Comments { comments } = next(socket).await {
            return comments;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_workspace_comment_s_mentions_reach_whom_they_name_and_go_with_its_chat() {
    let s = server().await;
    let http = reqwest::Client::new();
    let carol = "Bearer dev:carol@example.com";
    let share: WorkspaceShareStatus = http
        .post(format!("{}/v1/workspace-shares", s.base))
        .header("authorization", carol)
        .json(&serde_json::json!({
            "title": "Architecture",
            "guests": [
                {"email": "alice@example.com", "role": "suggest"},
                {"email": "bob@example.com", "role": "viewer"}
            ]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        share
            .people
            .iter()
            .map(|p| p.email.as_str())
            .collect::<Vec<_>>(),
        ["carol@example.com", "alice@example.com", "bob@example.com"],
        "the owner, then every guest, viewers too"
    );
    let mut daemon = workspace_daemon(
        &s,
        &share.id,
        WORKSPACE_SHARE_PROTOCOL_REVISION,
        &["c1", "c2"],
    )
    .await;
    let (mut bob_daemon, before) = user(&s, "bob@example.com").await;
    assert!(before.is_empty());

    // Alice, who can suggest, comments on a passage and mentions Bob, a viewer.
    let (mut alice, snapshot) = guest_page(&s, &share.id, "alice@example.com").await;
    assert_eq!(snapshot.people.len(), 3, "invitees see each other");
    alice
        .send(say(&FromGuest::Comment {
            chat: "c1".into(),
            message: "m1".into(),
            quote: "a monolith".into(),
            text: "@Bob what do you think?".into(),
            reply_to: None,
            mentions: vec![mention("bob@example.com"), mention("eve@elsewhere.com")],
        }))
        .await
        .unwrap();
    let comments = daemon_comments(&mut daemon).await;
    let thread = comments[0].clone();
    assert_eq!(
        thread.mentions,
        [Mention {
            email: "bob@example.com".into(),
            name: "Bob".into()
        }]
    );
    let told = mentions(&mut bob_daemon).await;
    assert_eq!(told.len(), 1);
    let m = &told[0];
    assert_eq!(
        (
            m.place,
            m.chat.as_deref(),
            m.thread.as_str(),
            m.seen,
            m.author.email.as_str()
        ),
        (
            MentionPlace::Workspace,
            Some("c1"),
            thread.id.as_str(),
            false,
            "alice@example.com"
        )
    );
    assert_eq!(
        m.url,
        format!(
            "https://design.example/w/{}?chat=c1&comment={}",
            share.id, thread.id
        )
    );

    // The owner replies from the daemon and mentions Alice: it is a thread's reply, for everyone.
    let reply: ShareComment = http
        .post(format!(
            "{}/v1/workspace-shares/{}/comments",
            s.base, share.id
        ))
        .header("authorization", carol)
        .json(&NewShareComment {
            chat: "c1".into(),
            text: "@Alice agreed".into(),
            reply_to: Some(thread.id.clone()),
            mentions: vec![mention("alice@example.com")],
            ..NewShareComment::default()
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        (
            reply.author.email.as_str(),
            reply.message.as_str(),
            reply.reply_to.as_deref()
        ),
        ("carol@example.com", "m1", Some(thread.id.as_str()))
    );
    // Alice's page hears of it (after her own comment).
    loop {
        if let ToGuest::Comments { comments } = next::<ToGuest>(&mut alice).await
            && comments.iter().any(|c| c.id == reply.id)
        {
            break;
        }
    }
    let refused = http
        .post(format!(
            "{}/v1/workspace-shares/{}/comments",
            s.base, share.id
        ))
        .header("authorization", "Bearer dev:alice@example.com")
        .json(&NewShareComment {
            chat: "c1".into(),
            text: "x".into(),
            reply_to: Some(thread.id.clone()),
            ..NewShareComment::default()
        })
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403, "only the owner comments over HTTP");
    // A viewer neither comments nor replies.
    let (mut bob, bob_snapshot) = guest_page(&s, &share.id, "bob@example.com").await;
    assert_eq!(
        bob_snapshot.unread_mentions,
        std::slice::from_ref(&thread.id)
    );
    bob.send(say(&FromGuest::Comment {
        chat: "c1".into(),
        message: String::new(),
        quote: String::new(),
        text: "me too".into(),
        reply_to: Some(thread.id.clone()),
        mentions: Vec::new(),
    }))
    .await
    .unwrap();
    while !matches!(next::<ToGuest>(&mut bob).await, ToGuest::Error { .. }) {}
    // Bob opens the thread: his mention is read.
    bob.send(say(&FromGuest::Seen {
        thread: thread.id.clone(),
    }))
    .await
    .unwrap();
    let read = mentions(&mut bob_daemon).await;
    assert!(read[0].seen);
    let (_, bob_again) = guest_page(&s, &share.id, "bob@example.com").await;
    assert!(bob_again.unread_mentions.is_empty());

    // A daemon older than replies hears the threads' first comments only.
    let mut old = workspace_daemon(&s, &share.id, 1, &["c1", "c2"]).await;
    alice
        .send(say(&FromGuest::Comment {
            chat: "c1".into(),
            message: String::new(),
            quote: String::new(),
            text: "thanks".into(),
            reply_to: Some(thread.id.clone()),
            mentions: Vec::new(),
        }))
        .await
        .unwrap();
    let heard = daemon_comments(&mut old).await;
    assert_eq!(
        heard.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        [thread.id.as_str()]
    );

    // The chat leaves the share: its comments go, and the mentions with them.
    old.send(say(&FromWorkspaceDaemon::Chats { chats: Vec::new() }))
        .await
        .unwrap();
    assert!(mentions(&mut bob_daemon).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_mentions_socket_wants_a_hello_in_the_right_version() {
    let s = server().await;
    let mut socket = connect(&format!("{}/v1/mentions/daemon", s.ws), None).await;
    socket
        .send(say(&FromMentionsDaemon::Hello {
            id_token: "dev:alice@example.com".into(),
            version: SHARE_PROTOCOL_VERSION + 1,
        }))
        .await
        .unwrap();
    let ToMentionsDaemon::Refused { reason } = next(&mut socket).await else {
        panic!()
    };
    assert!(reason.contains("update Chauffeur"), "{reason}");
    let mut socket = connect(&format!("{}/v1/mentions/daemon", s.ws), None).await;
    socket
        .send(say(&FromMentionsDaemon::Seen { id: "x".into() }))
        .await
        .unwrap();
    assert!(matches!(
        next::<ToMentionsDaemon>(&mut socket).await,
        ToMentionsDaemon::Refused { .. }
    ));
}
