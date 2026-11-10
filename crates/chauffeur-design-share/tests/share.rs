//! The share service end to end, on loopback with `dev:` identities: a daemon creates a share
//! and uploads a design, reviewers sign in, read it, comment, and hear what changes.

use std::sync::Arc;
use std::time::Duration;

use chauffeur_design_share::{Auth, Config, Service, data_paths, store::Store};
use chauffeur_protocol::client::DesignComment;
use chauffeur_protocol::design_share::{
    FromDaemon, FromReviewer, SHARE_PROTOCOL_VERSION, ShareSnapshot, ShareStatus, SharedDesign,
    ToDaemon, ToReviewer,
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

/// A page's session cookie for a `dev:` identity.
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

#[tokio::test(flavor = "multi_thread")]
async fn a_share_is_read_and_commented_by_its_reviewers_only() {
    let s = server().await;
    let http = reqwest::Client::new();
    let carol = "Bearer dev:carol@example.com";

    // Without a token, nothing.
    let refused = http
        .post(format!("{}/v1/shares", s.base))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 401);

    let share: ShareStatus = http
        .post(format!("{}/v1/shares", s.base))
        .header("authorization", carol)
        .json(&serde_json::json!({"title": "CTA", "reviewers": ["Alice@example.com", "bob@example.com"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(share.url, format!("https://design.example/s/{}", share.id));
    assert_eq!(share.reviewers, ["alice@example.com", "bob@example.com"]);
    assert_eq!(share.owner.email, "carol@example.com");
    assert!(!share.online && share.viewers.is_empty() && !share.stopped);

    // The owner uploads the design; a path outside the folder is refused.
    let put = |path: &str, body: &'static str| {
        http.put(format!("{}/v1/shares/{}/files/{}", s.base, share.id, path))
            .header("authorization", carol)
            .body(body)
            .send()
    };
    assert_eq!(
        put("sign%20in/empty.html", "<p>empty</p>")
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(
        put("canvas.json", r#"{"pages": []}"#)
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(put("..%2Fout.html", "x").await.unwrap().status(), 400);
    let other = http
        .put(format!("{}/v1/shares/{}/files/a.html", s.base, share.id))
        .header("authorization", "Bearer dev:alice@example.com")
        .body("x")
        .send()
        .await
        .unwrap();
    assert_eq!(other.status(), 403, "not the owner");

    // A reviewer signs in (a cookie), a stranger is refused the design, a reviewer reads it.
    let session = |email: &str| {
        http.post(format!("{}/v1/session", s.base))
            .json(&serde_json::json!({"id_token": format!("dev:{email}")}))
            .send()
    };
    let alice = session("alice@example.com").await.unwrap();
    assert_eq!(alice.status(), 200);
    let louis_cookie = alice.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let dave = session("dave@example.com").await.unwrap();
    let theo_cookie = dave.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    // Any Google account signs in; what it opens is another matter.
    assert_eq!(session("x@gmail.com").await.unwrap().status(), 200);
    assert_eq!(session("not-an-address").await.unwrap().status(), 401);
    let snapshot_url = format!("{}/v1/shares/{}/snapshot", s.base, share.id);
    assert_eq!(http.get(&snapshot_url).send().await.unwrap().status(), 401);
    let stranger = http
        .get(&snapshot_url)
        .header("cookie", &theo_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(stranger.status(), 403);
    let snapshot: ShareSnapshot = http
        .get(&snapshot_url)
        .header("cookie", &louis_cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(snapshot.me.email, "alice@example.com");
    assert_eq!(
        snapshot
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>(),
        ["canvas.json", "sign in/empty.html"]
    );
    assert!(!snapshot.online);
    let file_url = format!(
        "{}/v1/shares/{}/files/sign%20in/empty.html",
        s.base, share.id
    );
    let plain = http
        .get(&file_url)
        .header("cookie", &louis_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(plain.status(), 200);
    assert!(
        plain.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'self'")
    );
    assert_eq!(plain.text().await.unwrap(), "<p>empty</p>");
    let framed = http
        .get(format!("{file_url}?capture=1"))
        .header("cookie", &louis_cookie)
        .send()
        .await
        .unwrap();
    assert!(framed.text().await.unwrap().contains("chauffeur:element"));
    assert_eq!(
        http.get(&file_url)
            .header("cookie", &theo_cookie)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );

    // The daemon connects; the reviewer connects; each hears of the other.
    let ws_base = s.base.replace("http://", "ws://");
    let mut daemon = connect(&format!("{ws_base}/v1/shares/{}/daemon", share.id), None).await;
    daemon
        .send(say(&FromDaemon::Hello {
            id_token: "dev:carol@example.com".into(),
            version: SHARE_PROTOCOL_VERSION,
        }))
        .await
        .unwrap();
    let ToDaemon::Welcome { share: welcomed } = next(&mut daemon).await else {
        panic!()
    };
    assert!(welcomed.online);
    let mut louis_ws = connect(
        &format!("{ws_base}/v1/shares/{}/reviewer", share.id),
        Some(&louis_cookie),
    )
    .await;
    let ToReviewer::Snapshot { snapshot } = next(&mut louis_ws).await else {
        panic!()
    };
    assert!(snapshot.online);
    let ToReviewer::Viewers { viewers } = next(&mut louis_ws).await else {
        panic!()
    };
    assert_eq!(viewers, ["alice@example.com"]);
    let ToDaemon::Viewers { viewers } = next(&mut daemon).await else {
        panic!()
    };
    assert_eq!(viewers, ["alice@example.com"]);

    // The reviewer comments: the daemon gets it, so does the reviewer (confirmed).
    louis_ws
        .send(say(&FromReviewer::Add {
            screen: "sign in/empty.html".into(),
            pin: None,
            quote: None,
            text: "Too empty".into(),
            reply_to: None,
            mentions: Vec::new(),
        }))
        .await
        .unwrap();
    let ToDaemon::Comment { comment } = next(&mut daemon).await else {
        panic!()
    };
    assert_eq!(
        (comment.author.email.as_str(), comment.text.as_str()),
        ("alice@example.com", "Too empty")
    );
    let ToReviewer::Comment { comment: echoed } = next(&mut louis_ws).await else {
        panic!()
    };
    assert_eq!(echoed, comment);
    // A reviewer cannot edit what is not theirs: told, and nothing else happens.
    louis_ws
        .send(say(&FromReviewer::Edit {
            id: "nope".into(),
            text: "x".into(),
            mentions: None,
        }))
        .await
        .unwrap();
    let error: serde_json::Value = next(&mut louis_ws).await;
    assert_eq!(error["type"], "error");

    // The daemon pushes its comments.json (the reviewer's comment, now sent) and a file change.
    let mut sent = comment.clone();
    sent.sent = Some(chauffeur_protocol::client::CommentSent { at: 9 });
    let justin_comment = DesignComment {
        id: "j1".into(),
        screen: "sign in/empty.html".into(),
        quote: None,
        pin: None,
        author: share.owner.clone(),
        at: 10,
        text: "Mine".into(),
        reply_to: None,
        resolved: None,
        sent: None,
        mentions: Vec::new(),
    };
    daemon
        .send(say(&FromDaemon::Comments {
            comments: vec![sent.clone(), justin_comment.clone()],
        }))
        .await
        .unwrap();
    let ToReviewer::Snapshot { snapshot } = next(&mut louis_ws).await else {
        panic!()
    };
    assert_eq!(snapshot.comments, [sent.clone(), justin_comment]);
    assert!(snapshot.last_push_at.is_some());
    daemon
        .send(say(&FromDaemon::Changed {
            changed: vec!["sign in/empty.html".into()],
            removed: vec![],
        }))
        .await
        .unwrap();
    let ToReviewer::Files { changed, .. } = next(&mut louis_ws).await else {
        panic!()
    };
    assert_eq!(changed, ["sign in/empty.html"]);
    // Once sent, the reviewer no longer edits their comment.
    louis_ws
        .send(say(&FromReviewer::Edit {
            id: sent.id.clone(),
            text: "x".into(),
            mentions: None,
        }))
        .await
        .unwrap();
    let ToReviewer::Live { .. } = next(&mut louis_ws).await else {
        panic!()
    };
    let error: serde_json::Value = next(&mut louis_ws).await;
    assert_eq!(error["type"], "error");

    // Taken off the list, the reviewer is told; stopped, the share serves nothing.
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
    assert_eq!(patched.reviewers, ["bob@example.com"]);
    let ToReviewer::Refused { reason } = next(&mut louis_ws).await else {
        panic!()
    };
    assert!(reason.contains("no longer shared"), "{reason}");
    let stopped: ShareStatus = http
        .delete(format!("{}/v1/shares/{}", s.base, share.id))
        .header("authorization", carol)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(stopped.stopped);
    let bob = session("bob@example.com").await.unwrap();
    let ben_cookie = bob.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    assert_eq!(
        http.get(&snapshot_url)
            .header("cookie", &ben_cookie)
            .send()
            .await
            .unwrap()
            .status(),
        410
    );
    assert_eq!(put("again.html", "x").await.unwrap().status(), 410);
    // The daemon's socket is still up: the live list is dropped with the next connection attempt.
    let mut late = connect(&format!("{ws_base}/v1/shares/{}/daemon", share.id), None).await;
    late.send(say(&FromDaemon::Hello {
        id_token: "dev:carol@example.com".into(),
        version: SHARE_PROTOCOL_VERSION,
    }))
    .await
    .unwrap();
    let ToDaemon::Refused { reason } = next(&mut late).await else {
        panic!()
    };
    assert!(reason.contains("stopped"), "{reason}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_daemon_must_say_hello_as_the_owner_in_the_right_version() {
    let s = server().await;
    let http = reqwest::Client::new();
    let share: ShareStatus = http
        .post(format!("{}/v1/shares", s.base))
        .header("authorization", "Bearer dev:carol@example.com")
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(share.title, "Design");
    let ws_base = s.base.replace("http://", "ws://");
    let url = format!("{ws_base}/v1/shares/{}/daemon", share.id);
    let mut other = connect(&url, None).await;
    other
        .send(say(&FromDaemon::Hello {
            id_token: "dev:alice@example.com".into(),
            version: SHARE_PROTOCOL_VERSION,
        }))
        .await
        .unwrap();
    let ToDaemon::Refused { reason } = next(&mut other).await else {
        panic!()
    };
    assert_eq!(reason, "This share is not yours");
    let mut old = connect(&url, None).await;
    old.send(say(&FromDaemon::Hello {
        id_token: "dev:carol@example.com".into(),
        version: SHARE_PROTOCOL_VERSION + 1,
    }))
    .await
    .unwrap();
    let ToDaemon::Refused { reason } = next(&mut old).await else {
        panic!()
    };
    assert!(reason.contains("update Chauffeur"), "{reason}");
    let mut mute = connect(&url, None).await;
    mute.send(say(&FromDaemon::Changed {
        changed: vec![],
        removed: vec![],
    }))
    .await
    .unwrap();
    let ToDaemon::Refused { reason } = next(&mut mute).await else {
        panic!()
    };
    assert_eq!(reason, "Say hello first");
    assert_eq!(
        http.get(format!("{}/s/{}", s.base, share.id))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        http.get(format!("{}/health", s.base))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "ok"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reviewer_s_page_gets_the_canvas_laid_out_and_a_refusal_of_one_message_typed() {
    let s = server().await;
    let http = reqwest::Client::new();
    let carol = "Bearer dev:carol@example.com";
    let share: ShareStatus = http
        .post(format!("{}/v1/shares", s.base))
        .header("authorization", carol)
        .json(&serde_json::json!({"title": "CTA", "reviewers": ["alice@example.com"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for (path, body) in [
        ("a/hover.html", "<button>Go</button>"),
        ("stray.html", "<p>stray</p>"),
        (
            "canvas.json",
            r#"{"pages": [{"name": "Login", "sections": [{"title": "A", "screens": [{"file": "a/hover.html", "label": "Hover", "width": 400, "height": 300}, {"file": "gone.html"}]}]}]}"#,
        ),
    ] {
        let put = http
            .put(format!("{}/v1/shares/{}/files/{}", s.base, share.id, path))
            .header("authorization", carol)
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(put.status(), 204, "{path}");
    }
    let alice = http
        .post(format!("{}/v1/session", s.base))
        .json(&serde_json::json!({"id_token": "dev:alice@example.com"}))
        .send()
        .await
        .unwrap();
    let cookie = alice.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    // The snapshot lays the design out as the daemon would, each screen's URL on the page's own
    // origin; the VM's folder is not told.
    let snapshot: ShareSnapshot = http
        .get(format!("{}/v1/shares/{}/snapshot", s.base, share.id))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let canvas = &snapshot.canvas;
    assert_eq!(canvas.dir, "");
    assert_eq!(canvas.error, None);
    assert_eq!(canvas.pages[0].name, "Login");
    let screens = &canvas.pages[0].sections[0].screens;
    let screen_url = screens[0].url.as_deref().unwrap();
    assert!(screen_url.starts_with(&format!("/v1/shares/{}/render/", share.id)));
    assert!(screen_url.ends_with("/a/hover.html"));
    assert_eq!((screens[0].width, screens[0].height), (400, 300));
    assert!(screens[0].modified_ms.is_some());
    assert!(screens[1].url.is_none(), "a missing file");
    assert_eq!(canvas.unsorted[0].file, "stray.html");
    // Read again after `Files`: the same, and only for a reviewer.
    let again = http
        .get(format!("{}/v1/shares/{}/canvas", s.base, share.id))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 200);
    assert_eq!(
        again
            .json::<chauffeur_protocol::client::CanvasResult>()
            .await
            .unwrap(),
        *canvas
    );
    assert_eq!(
        http.get(format!("{}/v1/shares/{}/canvas", s.base, share.id))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );

    // A message the rules refuse comes back as an error of its own; the socket stays open.
    let ws_base = s.base.replace("http://", "ws://");
    let mut page = connect(
        &format!("{ws_base}/v1/shares/{}/reviewer", share.id),
        Some(&cookie),
    )
    .await;
    assert!(matches!(
        next::<ToReviewer>(&mut page).await,
        ToReviewer::Snapshot { .. }
    ));
    page.send(say(&FromReviewer::Edit {
        id: "nope".into(),
        text: "x".into(),
        mentions: None,
    }))
    .await
    .unwrap();
    let mut seen_error = false;
    for _ in 0..3 {
        match next::<ToReviewer>(&mut page).await {
            ToReviewer::Error { reason } => {
                assert_eq!(reason, "no comment nope");
                seen_error = true;
                break;
            }
            ToReviewer::Viewers { .. } => continue,
            other => panic!("{other:?}"),
        }
    }
    assert!(seen_error);
    page.send(say(&FromReviewer::Add {
        screen: "a/hover.html".into(),
        pin: None,
        quote: None,
        text: "still here".into(),
        reply_to: None,
        mentions: Vec::new(),
    }))
    .await
    .unwrap();
    loop {
        if let ToReviewer::Comment { comment } = next::<ToReviewer>(&mut page).await {
            assert_eq!(comment.text, "still here");
            break;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owner_changes_the_reviewers_from_their_own_page() {
    let s = server().await;
    let http = reqwest::Client::new();
    let carol = "Bearer dev:carol@example.com";
    let share: ShareStatus = http
        .post(format!("{}/v1/shares", s.base))
        .header("authorization", carol)
        .json(&serde_json::json!({"title": "CTA", "reviewers": ["alice@example.com"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let owner = session_cookie(&s.base, "carol@example.com").await;
    let alice = session_cookie(&s.base, "alice@example.com").await;

    // The owner's snapshot names the reviewers; a reviewer's does not.
    let snapshot_url = format!("{}/v1/shares/{}/snapshot", s.base, share.id);
    let mine: ShareSnapshot = http
        .get(&snapshot_url)
        .header("cookie", &owner)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(mine.reviewers, ["alice@example.com"]);
    let his: ShareSnapshot = http
        .get(&snapshot_url)
        .header("cookie", &alice)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(his.reviewers.is_empty());

    // The daemon and the pages are connected.
    let ws_base = s.base.replace("http://", "ws://");
    let mut daemon = connect(&format!("{ws_base}/v1/shares/{}/daemon", share.id), None).await;
    daemon
        .send(say(&FromDaemon::Hello {
            id_token: "dev:carol@example.com".into(),
            version: SHARE_PROTOCOL_VERSION,
        }))
        .await
        .unwrap();
    assert!(matches!(
        next::<ToDaemon>(&mut daemon).await,
        ToDaemon::Welcome { .. }
    ));
    let mut owner_page = connect(
        &format!("{ws_base}/v1/shares/{}/reviewer", share.id),
        Some(&owner),
    )
    .await;
    assert!(matches!(
        next::<ToReviewer>(&mut owner_page).await,
        ToReviewer::Snapshot { .. }
    ));
    let mut louis_page = connect(
        &format!("{ws_base}/v1/shares/{}/reviewer", share.id),
        Some(&alice),
    )
    .await;
    assert!(matches!(
        next::<ToReviewer>(&mut louis_page).await,
        ToReviewer::Snapshot { .. }
    ));

    // A reviewer may not change the list; the owner may, with their cookie alone.
    let refused = http
        .patch(format!("{}/v1/shares/{}", s.base, share.id))
        .header("cookie", &alice)
        .json(&serde_json::json!({"reviewers": ["alice@example.com", "dave@example.com"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
    let changed: ShareStatus = http
        .patch(format!("{}/v1/shares/{}", s.base, share.id))
        .header("cookie", &owner)
        .json(&serde_json::json!({"reviewers": ["bob@example.com"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(changed.reviewers, ["bob@example.com"]);

    // Alice is told at once; the owner's page and the daemon get the list.
    loop {
        if let ToReviewer::Refused { reason } = next::<ToReviewer>(&mut louis_page).await {
            assert!(reason.contains("no longer shared"), "{reason}");
            break;
        }
    }
    loop {
        if let ToReviewer::Reviewers { reviewers } = next::<ToReviewer>(&mut owner_page).await {
            assert_eq!(reviewers, ["bob@example.com"]);
            break;
        }
    }
    loop {
        if let ToDaemon::Reviewers { reviewers } = next::<ToDaemon>(&mut daemon).await {
            assert_eq!(reviewers, ["bob@example.com"]);
            break;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_named_on_a_share_reads_it_with_its_token_to_take_it_over() {
    let s = server().await;
    let http = reqwest::Client::new();
    let share: ShareStatus = http
        .post(format!("{}/v1/shares", s.base))
        .header("authorization", "Bearer dev:carol@example.com")
        .json(&serde_json::json!({"title": "CTA", "reviewers": ["alice@example.com"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let put = http
        .put(format!("{}/v1/shares/{}/files/a.html", s.base, share.id))
        .header("authorization", "Bearer dev:carol@example.com")
        .body("<p>a</p>")
        .send()
        .await
        .unwrap();
    assert_eq!(put.status(), 204);
    // Alice's daemon, with his Google token: the snapshot and the files; a reviewer's list is not his.
    let snapshot = http
        .get(format!("{}/v1/shares/{}/snapshot", s.base, share.id))
        .header("authorization", "Bearer dev:alice@example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(snapshot.status(), 200);
    let snapshot: ShareSnapshot = snapshot.json().await.unwrap();
    assert_eq!(snapshot.me.email, "alice@example.com");
    assert!(snapshot.reviewers.is_empty());
    let file = http
        .get(format!("{}/v1/shares/{}/files/a.html", s.base, share.id))
        .header("authorization", "Bearer dev:alice@example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(file.status(), 200);
    assert_eq!(file.text().await.unwrap(), "<p>a</p>");
    // Not Théo's daemon.
    let stranger = http
        .get(format!("{}/v1/shares/{}/snapshot", s.base, share.id))
        .header("authorization", "Bearer dev:dave@example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(stranger.status(), 403);
}

#[tokio::test]
async fn sandboxed_resources_load_without_cookies_but_never_outlive_the_reviewers_access() {
    let s = server().await;
    let http = reqwest::Client::new();
    let owner = "Bearer dev:owner@example.com";
    let create = || {
        http.post(format!("{}/v1/shares", s.base))
            .header("authorization", owner)
            .json(&serde_json::json!({"reviewers": ["alice@example.com"]}))
            .send()
    };
    let share: ShareStatus = create().await.unwrap().json().await.unwrap();
    let other: ShareStatus = create().await.unwrap().json().await.unwrap();
    for (path, body) in [
        ("screens/a.html", "<script src=../app.js></script>"),
        ("app.js", "document.body.textContent='loaded'"),
        ("style.css", "body{color:red}"),
        ("font.woff2", "font"),
    ] {
        assert_eq!(
            http.put(format!("{}/v1/shares/{}/files/{path}", s.base, share.id))
                .header("authorization", owner)
                .body(body)
                .send()
                .await
                .unwrap()
                .status(),
            204
        );
    }
    let cookie = session_cookie(&s.base, "alice@example.com").await;
    let canvas_url = format!("{}/v1/shares/{}/canvas", s.base, share.id);
    let canvas: chauffeur_protocol::client::CanvasResult = http
        .get(&canvas_url)
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let screen = canvas.unsorted[0].url.as_deref().unwrap();
    let url = reqwest::Url::parse(&format!("{}{screen}", s.base)).unwrap();
    for path in [
        "a.html?capture=1",
        "../app.js",
        "../style.css",
        "../font.woff2",
    ] {
        let response = http
            .get(url.join(path).unwrap())
            .header("origin", "null")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{path}");
        assert_eq!(response.headers()["access-control-allow-origin"], "*");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.headers()["referrer-policy"], "no-referrer");
        assert!(
            response
                .headers()
                .get("access-control-allow-credentials")
                .is_none()
        );
        if path.starts_with("a.html") {
            assert!(response.text().await.unwrap().contains("chauffeur:element"));
        }
    }
    // The lease neither replaces a session nor grants a write or access to another share.
    assert_eq!(
        http.get(format!("{}/v1/shares/{}/files/app.js", s.base, share.id))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        http.get(&canvas_url)
            .header(
                "authorization",
                format!("Bearer {}", screen.split('/').nth(5).unwrap())
            )
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        http.put(url.clone())
            .body("changed")
            .send()
            .await
            .unwrap()
            .status(),
        405
    );
    assert_eq!(
        http.get(url.as_str().replace(&share.id, &other.id))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let mut invalid = url.clone();
    invalid.set_path(&format!("/v1/shares/{}/render/invalid/app.js", share.id));
    assert_eq!(http.get(invalid).send().await.unwrap().status(), 401);
    assert_eq!(
        http.get(url.join("../../../files/app.js").unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let renewed: chauffeur_protocol::client::CanvasResult = http
        .get(&canvas_url)
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(renewed.unsorted[0].url, canvas.unsorted[0].url);
    assert_eq!(
        http.patch(format!("{}/v1/shares/{}", s.base, share.id))
            .header("authorization", owner)
            .json(&serde_json::json!({"reviewers": []}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(http.get(url.clone()).send().await.unwrap().status(), 403);
    assert_eq!(
        http.delete(format!("{}/v1/shares/{}", s.base, share.id))
            .header("authorization", owner)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(http.get(url).send().await.unwrap().status(), 410);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_share_open_to_the_team_is_read_and_listed_by_every_member() {
    let s = server().await;
    let http = reqwest::Client::new();
    let create = |owner: &'static str, body: serde_json::Value| {
        http.post(format!("{}/v1/shares", s.base))
            .header("authorization", format!("Bearer dev:{owner}"))
            .json(&body)
            .send()
    };
    let team: ShareStatus = create(
        "alice@example.com",
        serde_json::json!({"title": "Onboarding", "team": true}),
    )
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert!(team.team && team.reviewers.is_empty());
    let named: ShareStatus = create(
        "carol@example.com",
        serde_json::json!({"title": "CTA", "reviewers": ["bob@example.com", "dave@example.com"]}),
    )
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert!(!named.team);
    create(
        "carol@example.com",
        serde_json::json!({"title": "Private", "reviewers": ["dave@example.com"]}),
    )
    .await
    .unwrap();

    // Bob's daemon lists what is open to him, and never who else is named.
    let list = |who: &'static str| {
        http.get(format!("{}/v1/shares", s.base))
            .header("authorization", format!("Bearer dev:{who}"))
            .send()
    };
    let listed: Vec<SharedDesign> = list("bob@example.com").await.unwrap().json().await.unwrap();
    let mut seen: Vec<(&str, bool, bool)> = listed
        .iter()
        .map(|d| (d.title.as_str(), d.named, d.team))
        .collect();
    seen.sort();
    assert_eq!(seen, [("CTA", true, false), ("Onboarding", false, true)]);
    let onboarding = listed.iter().find(|d| d.team).unwrap();
    assert_eq!(onboarding.owner.name, "Alice");
    assert_eq!(
        onboarding.url,
        format!("https://design.example/s/{}", team.id)
    );
    // The page lists the same with its cookie; nobody signed in, nothing.
    let cookie = session_cookie(&s.base, "bob@example.com").await;
    let from_page: Vec<SharedDesign> = http
        .get(format!("{}/v1/shares", s.base))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(from_page.len(), 2);
    let anonymous = http
        .get(format!("{}/v1/shares", s.base))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 401);

    // A member never named reads the team's share, not another.
    let snapshot = http
        .get(format!("{}/v1/shares/{}/snapshot", s.base, team.id))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(snapshot.status(), 200);
    let snapshot: ShareSnapshot = snapshot.json().await.unwrap();
    assert!(
        snapshot.reviewers.is_empty(),
        "a member never sees who is named"
    );

    // Closed to the team again: Bob's page is refused at once and the share leaves his list.
    let mut page = connect(
        &format!(
            "{}/v1/shares/{}/reviewer",
            s.base.replace("http", "ws"),
            team.id
        ),
        Some(&cookie),
    )
    .await;
    assert!(matches!(
        next::<ToReviewer>(&mut page).await,
        ToReviewer::Snapshot { .. }
    ));
    let closed: ShareStatus = http
        .patch(format!("{}/v1/shares/{}", s.base, team.id))
        .header("authorization", "Bearer dev:alice@example.com")
        .json(&serde_json::json!({"team": false}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!closed.team);
    loop {
        match next::<ToReviewer>(&mut page).await {
            ToReviewer::Refused { .. } => break,
            ToReviewer::Viewers { .. } => continue,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }
    let listed: Vec<SharedDesign> = list("bob@example.com").await.unwrap().json().await.unwrap();
    assert_eq!(
        listed.iter().map(|d| d.title.as_str()).collect::<Vec<_>>(),
        ["CTA"]
    );
    let refused = http
        .get(format!("{}/v1/shares/{}/snapshot", s.base, team.id))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
}

#[tokio::test]
async fn a_share_of_a_file_that_is_not_markdown_is_refused_and_leaves_nothing_behind() {
    let s = server().await;
    let http = reqwest::Client::new();
    let create = |document: &str| {
        http.post(format!("{}/v1/shares", s.base))
            .header("authorization", "Bearer dev:carol@example.com")
            .json(&serde_json::json!({"title": "Plan", "reviewers": ["alice@example.com"], "document": document}))
            .send()
    };
    for bad in ["notes.txt", "plans/plan.md", ".plan.md"] {
        assert_eq!(create(bad).await.unwrap().status(), 400, "{bad}");
    }
    let listed = |http: reqwest::Client, base: String| async move {
        http.get(format!("{base}/v1/shares"))
            .header("authorization", "Bearer dev:alice@example.com")
            .send()
            .await
            .unwrap()
            .json::<Vec<chauffeur_protocol::design_share::SharedDesign>>()
            .await
            .unwrap()
    };
    assert!(listed(http.clone(), s.base.clone()).await.is_empty());
    assert_eq!(create("plan.md").await.unwrap().status(), 200);
    let shares = listed(http.clone(), s.base.clone()).await;
    assert_eq!(shares.len(), 1);
    assert_eq!(shares[0].document.as_deref(), Some("plan.md"));
}
