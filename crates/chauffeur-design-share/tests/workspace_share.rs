//! Workspace shares end to end, on loopback with `dev:` identities: a daemon shares a workspace
//! and pushes its chats and notes, guests sign in, read them live, suggest, and lose access when
//! taken off or when the share stops.

use std::sync::Arc;
use std::time::Duration;

use chauffeur_design_share::{Auth, Config, Service, data_paths, store::Store};
use chauffeur_protocol::events::{ChatEvent, Event, EventEnvelope};
use chauffeur_protocol::model::{AgentKind, ChatKind, ChatStatus};
use chauffeur_protocol::workspace_share::{
    FromGuest, FromWorkspaceDaemon, GuestRole, SharedChat, SharedNote, Suggestion, SuggestionState,
    ToGuest, ToWorkspaceDaemon, WORKSPACE_SHARE_PROTOCOL_REVISION,
    WORKSPACE_SHARE_PROTOCOL_VERSION, WorkspaceShareSnapshot, WorkspaceShareStatus,
};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

struct Server {
    _dir: tempfile::TempDir,
    base: String,
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
            workspace_url: Some("https://share.example".into()),
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

/// The next message that `wanted` picks, skipping the others (presence, Live).
async fn until<T: serde::de::DeserializeOwned + std::fmt::Debug>(
    socket: &mut Socket,
    wanted: impl Fn(&T) -> bool,
) -> T {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("a message in time")
            .expect("the socket open")
            .unwrap();
        if let Message::Text(text) = message {
            let message: T = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
            if wanted(&message) {
                return message;
            }
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

fn chat(id: &str) -> SharedChat {
    SharedChat {
        id: id.into(),
        title: Some("Architecture".into()),
        kind: ChatKind::Chat,
        agent: AgentKind::Claude,
        status: ChatStatus::Working,
        created_at: "2026-10-29T10:00:00Z".into(),
    }
}

fn said(chat: &str, seq: Option<i64>, text: &str) -> EventEnvelope {
    EventEnvelope {
        seq,
        ts: "2026-10-29T10:00:00Z".into(),
        event: Event::Chat {
            workspace_id: "w".into(),
            chat_id: chat.into(),
            event: ChatEvent::AssistantText {
                message_id: text.into(),
                text: text.into(),
                parent_tool_use_id: None,
            },
        },
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_workspace_share_is_read_live_by_its_guests_and_suggested_to_by_those_who_may() {
    let s = server().await;
    let http = reqwest::Client::new();
    let carol = "Bearer dev:carol@example.com";

    let share: WorkspaceShareStatus = http
        .post(format!("{}/v1/workspace-shares", s.base))
        .header("authorization", carol)
        .json(&serde_json::json!({
            "title": "Toyota — architecture",
            "guests": [
                {"email": "Alice@example.com", "role": "suggest"},
                {"email": "bob@example.com", "role": "viewer"}
            ]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(share.url, format!("https://share.example/w/{}", share.id));
    assert_eq!(share.guests[0].email, "alice@example.com");
    assert_eq!(share.guests[0].role, GuestRole::Suggest);
    assert!(!share.online && !share.stopped);
    let other = http
        .delete(format!("{}/v1/workspace-shares/{}", s.base, share.id))
        .header("authorization", "Bearer dev:alice@example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(other.status(), 403, "not the owner");

    // The daemon connects and pushes the chats, a history, live events and the notes.
    let ws_base = s.base.replace("http://", "ws://");
    let mut daemon = connect(
        &format!("{ws_base}/v1/workspace-shares/{}/daemon", share.id),
        None,
    )
    .await;
    daemon
        .send(say(&FromWorkspaceDaemon::Hello {
            id_token: "dev:carol@example.com".into(),
            version: WORKSPACE_SHARE_PROTOCOL_VERSION,
            revision: WORKSPACE_SHARE_PROTOCOL_REVISION,
        }))
        .await
        .unwrap();
    let ToWorkspaceDaemon::Welcome {
        share: welcomed,
        suggestions,
        ..
    } = until(&mut daemon, |_: &ToWorkspaceDaemon| true).await
    else {
        panic!()
    };
    assert!(welcomed.online && suggestions.is_empty());
    let note = SharedNote {
        path: "adr/0001-split.md".into(),
        text: "# Split the API".into(),
        modified_ms: 3,
    };
    for message in [
        FromWorkspaceDaemon::Chats {
            chats: vec![chat("c1")],
        },
        FromWorkspaceDaemon::History {
            chat: "c1".into(),
            events: vec![said("c1", Some(1), "Two options"), said("c1", Some(2), "A")],
        },
        FromWorkspaceDaemon::Notes {
            changed: vec![
                note.clone(),
                SharedNote {
                    path: "../secret.md".into(),
                    text: "x".into(),
                    modified_ms: 1,
                },
            ],
            removed: vec![],
        },
    ] {
        daemon.send(say(&message)).await.unwrap();
    }

    // A stranger is refused; a guest reads the snapshot and a chat's events.
    let alice = session_cookie(&s.base, "alice@example.com").await;
    let bob = session_cookie(&s.base, "bob@example.com").await;
    let dave = session_cookie(&s.base, "dave@example.com").await;
    let snapshot_url = format!("{}/v1/workspace-shares/{}/snapshot", s.base, share.id);
    assert_eq!(
        http.get(&snapshot_url)
            .header("cookie", &dave)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let snapshot = loop {
        let snapshot: WorkspaceShareSnapshot = http
            .get(&snapshot_url)
            .header("cookie", &alice)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if !snapshot.notes.is_empty() {
            break snapshot;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(snapshot.role, GuestRole::Suggest);
    assert_eq!(snapshot.chats, [chat("c1")]);
    assert_eq!(
        snapshot.notes,
        [note],
        "a path outside the notes is dropped"
    );
    assert!(snapshot.online && snapshot.last_push_at.is_some());
    let events_url = |chat: &str| {
        format!(
            "{}/v1/workspace-shares/{}/chats/{chat}/events",
            s.base, share.id
        )
    };
    let events: Vec<EventEnvelope> = http
        .get(events_url("c1"))
        .header("cookie", &alice)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        events.iter().map(|e| e.seq).collect::<Vec<_>>(),
        [Some(1), Some(2)]
    );
    assert_eq!(
        http.get(events_url("c9"))
            .header("cookie", &alice)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );

    // Guests connect; live events reach them, text being written too.
    let mut louis_ws = connect(
        &format!("{ws_base}/v1/workspace-shares/{}/guest", share.id),
        Some(&alice),
    )
    .await;
    let ToGuest::Snapshot { snapshot } = until(&mut louis_ws, |_: &ToGuest| true).await else {
        panic!()
    };
    assert_eq!(snapshot.me.email, "alice@example.com");
    let mut ben_ws = connect(
        &format!("{ws_base}/v1/workspace-shares/{}/guest", share.id),
        Some(&bob),
    )
    .await;
    let ToGuest::Snapshot { snapshot } = until(&mut ben_ws, |_: &ToGuest| true).await else {
        panic!()
    };
    assert_eq!(snapshot.role, GuestRole::Viewer);
    let ToWorkspaceDaemon::Viewers { .. } = until(&mut daemon, |m: &ToWorkspaceDaemon| {
        matches!(m, ToWorkspaceDaemon::Viewers { viewers } if viewers.len() == 2)
    })
    .await
    else {
        panic!()
    };
    daemon
        .send(say(&FromWorkspaceDaemon::Events {
            chat: "c1".into(),
            events: vec![said("c1", None, "typ"), said("c1", Some(3), "B")],
        }))
        .await
        .unwrap();
    let ToGuest::Events { events, .. } = until(&mut ben_ws, |m: &ToGuest| {
        matches!(m, ToGuest::Events { .. })
    })
    .await
    else {
        panic!()
    };
    assert_eq!(events.len(), 2, "the text being written passes on");

    // A viewer's suggestion is refused; a suggester's reaches the daemon and every guest.
    ben_ws
        .send(say(&FromGuest::Suggest {
            chat: "c1".into(),
            text: "Take option A".into(),
        }))
        .await
        .unwrap();
    let ToGuest::Error { reason } = until(&mut ben_ws, |m: &ToGuest| {
        matches!(m, ToGuest::Error { .. })
    })
    .await
    else {
        panic!()
    };
    assert!(reason.contains("suggest"), "{reason}");
    louis_ws
        .send(say(&FromGuest::Suggest {
            chat: "c1".into(),
            text: " Take option B ".into(),
        }))
        .await
        .unwrap();
    let ToWorkspaceDaemon::Suggestion { suggestion } = until(&mut daemon, |m| {
        matches!(m, ToWorkspaceDaemon::Suggestion { .. })
    })
    .await
    else {
        panic!()
    };
    assert_eq!(
        (
            suggestion.text.as_str(),
            suggestion.author.email.as_str(),
            suggestion.state
        ),
        (
            "Take option B",
            "alice@example.com",
            SuggestionState::Pending
        )
    );
    let ToGuest::Suggestions { suggestions } = until(&mut ben_ws, |m: &ToGuest| {
        matches!(m, ToGuest::Suggestions { .. })
    })
    .await
    else {
        panic!()
    };
    assert_eq!(suggestions, std::slice::from_ref(&suggestion));

    // The owner sends it: the guests see it sent.
    let sent = Suggestion {
        state: SuggestionState::Sent,
        decided_at: Some(10),
        ..suggestion
    };
    daemon
        .send(say(&FromWorkspaceDaemon::Suggestions {
            suggestions: vec![sent.clone()],
        }))
        .await
        .unwrap();
    let ToGuest::Suggestions { suggestions } = until(&mut louis_ws, |m: &ToGuest| {
        matches!(m, ToGuest::Suggestions { suggestions } if suggestions.iter().any(|s| s.state == SuggestionState::Sent))
    })
    .await
    else {
        panic!()
    };
    assert_eq!(suggestions, [sent]);

    // Bob taken off is refused at once; Alice made a viewer reads the share again as one.
    let patched: WorkspaceShareStatus = http
        .patch(format!("{}/v1/workspace-shares/{}", s.base, share.id))
        .header("authorization", carol)
        .json(&serde_json::json!({"guests": [{"email": "alice@example.com", "role": "viewer"}]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(patched.guests.len(), 1);
    let ToGuest::Refused { .. } = until(&mut ben_ws, |m: &ToGuest| {
        matches!(m, ToGuest::Refused { .. })
    })
    .await
    else {
        panic!()
    };
    let ToGuest::Snapshot { snapshot } = until(&mut louis_ws, |m: &ToGuest| {
        matches!(m, ToGuest::Snapshot { .. })
    })
    .await
    else {
        panic!()
    };
    assert_eq!(snapshot.role, GuestRole::Viewer);

    // A chat no longer listed takes its events away.
    daemon
        .send(say(&FromWorkspaceDaemon::Chats { chats: vec![] }))
        .await
        .unwrap();
    let ToGuest::Chats { chats } = until(&mut louis_ws, |m: &ToGuest| {
        matches!(m, ToGuest::Chats { .. })
    })
    .await
    else {
        panic!()
    };
    assert!(chats.is_empty());
    assert_eq!(
        http.get(events_url("c1"))
            .header("cookie", &alice)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );

    // Stopped: guests and the daemon are told, the link is gone, the copy with it.
    let stopped: WorkspaceShareStatus = http
        .delete(format!("{}/v1/workspace-shares/{}", s.base, share.id))
        .header("authorization", carol)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(stopped.stopped);
    let ToGuest::Refused { reason } = until(&mut louis_ws, |m: &ToGuest| {
        matches!(m, ToGuest::Refused { .. })
    })
    .await
    else {
        panic!()
    };
    assert_eq!(reason, "This share was stopped");
    let ToWorkspaceDaemon::Refused { .. } = until(&mut daemon, |m| {
        matches!(m, ToWorkspaceDaemon::Refused { .. })
    })
    .await
    else {
        panic!()
    };
    assert_eq!(
        http.get(&snapshot_url)
            .header("cookie", &alice)
            .send()
            .await
            .unwrap()
            .status(),
        410
    );
    let page = http
        .get(format!("{}/w/{}", s.base, share.id))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200, "the page says the share was stopped");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_suggestion_made_while_the_daemon_was_away_reaches_it_at_its_return() {
    let s = server().await;
    let http = reqwest::Client::new();
    let share: WorkspaceShareStatus = http
        .post(format!("{}/v1/workspace-shares", s.base))
        .header("authorization", "Bearer dev:carol@example.com")
        .json(&serde_json::json!({"guests": [{"email": "alice@example.com", "role": "suggest"}], "team": true}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(share.title, "Workspace");
    let ws_base = s.base.replace("http://", "ws://");
    let hello = || {
        say(&FromWorkspaceDaemon::Hello {
            id_token: "dev:carol@example.com".into(),
            version: WORKSPACE_SHARE_PROTOCOL_VERSION,
            revision: WORKSPACE_SHARE_PROTOCOL_REVISION,
        })
    };
    let daemon_url = format!("{ws_base}/v1/workspace-shares/{}/daemon", share.id);
    let mut daemon = connect(&daemon_url, None).await;
    daemon.send(hello()).await.unwrap();
    until(&mut daemon, |m: &ToWorkspaceDaemon| {
        matches!(m, ToWorkspaceDaemon::Welcome { .. })
    })
    .await;
    daemon
        .send(say(&FromWorkspaceDaemon::Chats {
            chats: vec![chat("c1")],
        }))
        .await
        .unwrap();
    // The chats are kept before the daemon leaves.
    let dave = session_cookie(&s.base, "dave@example.com").await;
    loop {
        let snapshot: WorkspaceShareSnapshot = http
            .get(format!(
                "{}/v1/workspace-shares/{}/snapshot",
                s.base, share.id
            ))
            .header("cookie", &dave)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if !snapshot.chats.is_empty() {
            assert_eq!(
                snapshot.role,
                GuestRole::Viewer,
                "the team reads as viewers"
            );
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    daemon.close(None).await.unwrap();
    // Sending a close frame does not wait for the server to mark the daemon offline.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot: WorkspaceShareSnapshot = http
                .get(format!(
                    "{}/v1/workspace-shares/{}/snapshot",
                    s.base, share.id
                ))
                .header("cookie", &dave)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if !snapshot.online {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the server observes the daemon disconnect before the guest connects");

    let alice = session_cookie(&s.base, "alice@example.com").await;
    let mut louis_ws = connect(
        &format!("{ws_base}/v1/workspace-shares/{}/guest", share.id),
        Some(&alice),
    )
    .await;
    let ToGuest::Snapshot { snapshot } = until(&mut louis_ws, |_: &ToGuest| true).await else {
        panic!()
    };
    assert!(!snapshot.online);
    louis_ws
        .send(say(&FromGuest::Suggest {
            chat: "c1".into(),
            text: "Ask Théo about the queue".into(),
        }))
        .await
        .unwrap();
    until(&mut louis_ws, |m: &ToGuest| {
        matches!(m, ToGuest::Suggestions { .. })
    })
    .await;

    let mut daemon = connect(&daemon_url, None).await;
    daemon.send(hello()).await.unwrap();
    let ToWorkspaceDaemon::Welcome { suggestions, .. } = until(&mut daemon, |m| {
        matches!(m, ToWorkspaceDaemon::Welcome { .. })
    })
    .await
    else {
        panic!()
    };
    assert_eq!(suggestions.len(), 1);
    // The daemon's list, from before it heard of it, does not drop it.
    daemon
        .send(say(&FromWorkspaceDaemon::Suggestions {
            suggestions: vec![],
        }))
        .await
        .unwrap();
    let ToGuest::Suggestions { suggestions } = until(&mut louis_ws, |m: &ToGuest| {
        matches!(m, ToGuest::Suggestions { .. })
    })
    .await
    else {
        panic!()
    };
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].state, SuggestionState::Pending);
}

#[tokio::test]
async fn a_guest_deletes_their_suggestion_and_comments_on_a_passage_that_everyone_sees() {
    let s = server().await;
    let http = reqwest::Client::new();
    let share: WorkspaceShareStatus = http
        .post(format!("{}/v1/workspace-shares", s.base))
        .header("authorization", "Bearer dev:carol@example.com")
        .json(&serde_json::json!({"guests": [
            {"email": "alice@example.com", "role": "suggest"},
            {"email": "bob@example.com", "role": "suggest"}
        ]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ws_base = s.base.replace("http://", "ws://");
    let daemon_url = format!("{ws_base}/v1/workspace-shares/{}/daemon", share.id);
    let hello = |revision| {
        say(&FromWorkspaceDaemon::Hello {
            id_token: "dev:carol@example.com".into(),
            version: WORKSPACE_SHARE_PROTOCOL_VERSION,
            revision,
        })
    };
    let mut daemon = connect(&daemon_url, None).await;
    daemon
        .send(hello(WORKSPACE_SHARE_PROTOCOL_REVISION))
        .await
        .unwrap();
    until(&mut daemon, |m: &ToWorkspaceDaemon| {
        matches!(m, ToWorkspaceDaemon::Welcome { .. })
    })
    .await;
    daemon
        .send(say(&FromWorkspaceDaemon::Chats {
            chats: vec![chat("c1")],
        }))
        .await
        .unwrap();
    let guest = |email: &'static str| {
        let (base, ws_base, id) = (s.base.clone(), ws_base.clone(), share.id.clone());
        async move {
            let cookie = session_cookie(&base, email).await;
            let mut ws = connect(
                &format!("{ws_base}/v1/workspace-shares/{id}/guest"),
                Some(&cookie),
            )
            .await;
            until(&mut ws, |m: &ToGuest| matches!(m, ToGuest::Snapshot { .. })).await;
            ws
        }
    };
    let mut alice = guest("alice@example.com").await;
    let mut bob = guest("bob@example.com").await;
    // The chats reached the service before the guests speak of c1.
    loop {
        let message = FromGuest::Suggest {
            chat: "c1".into(),
            text: "Ask Théo".into(),
        };
        alice.send(say(&message)).await.unwrap();
        if let ToGuest::Suggestions { .. } = until(&mut alice, |m: &ToGuest| {
            matches!(m, ToGuest::Suggestions { .. } | ToGuest::Error { .. })
        })
        .await
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let ToWorkspaceDaemon::Suggestion { suggestion } = until(&mut daemon, |m| {
        matches!(m, ToWorkspaceDaemon::Suggestion { .. })
    })
    .await
    else {
        panic!()
    };

    // Bob may not delete Alice's suggestion; Alice does, and it leaves every page.
    bob.send(say(&FromGuest::DeleteSuggestion {
        id: suggestion.id.clone(),
    }))
    .await
    .unwrap();
    until(&mut bob, |m: &ToGuest| matches!(m, ToGuest::Error { .. })).await;
    alice
        .send(say(&FromGuest::DeleteSuggestion {
            id: suggestion.id.clone(),
        }))
        .await
        .unwrap();
    let ToGuest::Suggestions { suggestions } = until(&mut bob, |m: &ToGuest| {
        // Earlier broadcasts may arrive after the refusal from Bob's own connection.
        matches!(m, ToGuest::Suggestions { suggestions } if suggestions.is_empty())
    })
    .await
    else {
        panic!()
    };
    assert!(suggestions.is_empty());
    let ToWorkspaceDaemon::Suggestion { suggestion: told } = until(&mut daemon, |m| {
        matches!(m, ToWorkspaceDaemon::Suggestion { .. })
    })
    .await
    else {
        panic!()
    };
    assert_eq!(
        (told.id, told.state),
        (suggestion.id.clone(), SuggestionState::Deleted)
    );
    // The daemon's list from before it heard of the deletion does not bring it back.
    daemon
        .send(say(&FromWorkspaceDaemon::Suggestions {
            suggestions: vec![suggestion.clone()],
        }))
        .await
        .unwrap();
    let ToGuest::Suggestions { suggestions } = until(&mut bob, |m: &ToGuest| {
        matches!(m, ToGuest::Suggestions { .. })
    })
    .await
    else {
        panic!()
    };
    assert!(suggestions.is_empty());

    // Alice comments on a passage: Bob and the daemon see it; only Alice deletes it.
    alice
        .send(say(&FromGuest::Comment {
            chat: "c1".into(),
            message: "m1".into(),
            quote: "a monolith".into(),
            text: "Why not two services?".into(),
            reply_to: None,
            mentions: Vec::new(),
        }))
        .await
        .unwrap();
    let ToGuest::Comments { comments } = until(&mut bob, |m: &ToGuest| {
        matches!(m, ToGuest::Comments { .. })
    })
    .await
    else {
        panic!()
    };
    assert_eq!(comments.len(), 1);
    assert_eq!(
        (
            comments[0].quote.as_str(),
            comments[0].author.email.as_str()
        ),
        ("a monolith", "alice@example.com")
    );
    let ToWorkspaceDaemon::Comments { comments: told } = until(&mut daemon, |m| {
        matches!(m, ToWorkspaceDaemon::Comments { .. })
    })
    .await
    else {
        panic!()
    };
    assert_eq!(told, comments);
    let snapshot: WorkspaceShareSnapshot = http
        .get(format!(
            "{}/v1/workspace-shares/{}/snapshot",
            s.base, share.id
        ))
        .header("cookie", session_cookie(&s.base, "bob@example.com").await)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(snapshot.comments, comments);
    assert!(snapshot.suggestions.is_empty(), "no deleted suggestion");
    bob.send(say(&FromGuest::DeleteComment {
        id: comments[0].id.clone(),
    }))
    .await
    .unwrap();
    until(&mut bob, |m: &ToGuest| matches!(m, ToGuest::Error { .. })).await;

    // A daemon older than comments and deletions hears of neither.
    drop(daemon);
    let mut old = connect(&daemon_url, None).await;
    old.send(hello(0)).await.unwrap();
    let ToWorkspaceDaemon::Welcome {
        suggestions,
        comments: theirs,
        ..
    } = until(&mut old, |m| matches!(m, ToWorkspaceDaemon::Welcome { .. })).await
    else {
        panic!()
    };
    assert!(suggestions.is_empty() && theirs.is_empty());

    alice
        .send(say(&FromGuest::DeleteComment {
            id: comments[0].id.clone(),
        }))
        .await
        .unwrap();
    let ToGuest::Comments { comments } = until(&mut bob, |m: &ToGuest| {
        matches!(m, ToGuest::Comments { .. })
    })
    .await
    else {
        panic!()
    };
    assert!(comments.is_empty());
}
