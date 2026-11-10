//! Invitation e-mails, end to end on loopback: a share writes to the people it names, whatever
//! their address, an update to those it adds only, never to the owner or to the team.

use std::sync::{Arc, Mutex};

use chauffeur_design_share::mail::{Email, Mailer};
use chauffeur_design_share::{Auth, Config, Service, data_paths, store::Store};
use chauffeur_protocol::design_share::ShareStatus;
use chauffeur_protocol::workspace_share::WorkspaceShareStatus;
use serde_json::json;

const CAROL: &str = "Bearer dev:carol@example.com";

struct Server {
    _dir: tempfile::TempDir,
    base: String,
    sent: Arc<Mutex<Vec<Email>>>,
}

impl Server {
    /// The e-mails sent since the last call: (to, subject).
    fn sent(&self) -> Vec<(String, String)> {
        self.sent
            .lock()
            .unwrap()
            .drain(..)
            .map(|i| (i.to, i.subject))
            .collect()
    }
}

async fn server() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let (db, files) = data_paths(dir.path());
    let sent = Arc::new(Mutex::new(Vec::new()));
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
            mailer: Mailer::Memory(sent.clone()),
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = service.router();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Server {
        _dir: dir,
        base,
        sent,
    }
}

async fn call<T: serde::de::DeserializeOwned>(
    method: reqwest::Method,
    url: String,
    body: serde_json::Value,
) -> T {
    reqwest::Client::new()
        .request(method, url)
        .header("authorization", CAROL)
        .json(&body)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}

fn invited(to: &str, subject: &str) -> (String, String) {
    (to.to_string(), subject.to_string())
}

#[tokio::test]
async fn a_design_share_writes_to_the_people_it_names_then_to_those_added() {
    let s = server().await;
    let share: ShareStatus = call(
        reqwest::Method::POST,
        format!("{}/v1/shares", s.base),
        json!({"title": "CTA", "reviewers": ["Alice@example.com", "carol@example.com", "someone@gmail.com"]}),
    )
    .await;
    let sent = s.sent.lock().unwrap().clone();
    assert_eq!(
        sent.iter().map(|e| e.to.as_str()).collect::<Vec<_>>(),
        ["alice@example.com", "someone@gmail.com"]
    );
    assert_eq!(sent[0].reply_to.as_deref(), Some("carol@example.com"));
    assert!(
        sent[0]
            .text
            .contains(&format!("https://design.example/s/{}", share.id))
    );
    s.sent();

    let _: ShareStatus = call(
        reqwest::Method::PATCH,
        format!("{}/v1/shares/{}", s.base, share.id),
        json!({"reviewers": ["alice@example.com", "bob@example.com"]}),
    )
    .await;
    assert_eq!(
        s.sent(),
        vec![invited("bob@example.com", "Carol shared “CTA” with you")]
    );

    // Opening it to the team, or renaming it, writes to no one.
    let _: ShareStatus = call(
        reqwest::Method::PATCH,
        format!("{}/v1/shares/{}", s.base, share.id),
        json!({"title": "CTA v2", "team": true}),
    )
    .await;
    assert_eq!(s.sent(), vec![]);
}

#[tokio::test]
async fn a_workspace_share_writes_to_new_guests_only_naming_their_role() {
    let s = server().await;
    let share: WorkspaceShareStatus = call(
        reqwest::Method::POST,
        format!("{}/v1/workspace-shares", s.base),
        json!({"title": "Cut the project", "guests": [
            {"email": "alice@example.com", "role": "suggest"},
            {"email": "bob@example.com", "role": "viewer"},
        ]}),
    )
    .await;
    let sent = s.sent.lock().unwrap().drain(..).collect::<Vec<_>>();
    assert_eq!(
        sent.iter().map(|i| i.to.as_str()).collect::<Vec<_>>(),
        ["alice@example.com", "bob@example.com"]
    );
    assert!(sent[0].text.contains("suggest messages to its agents"));
    assert!(!sent[1].text.contains("suggest"));
    assert!(
        sent[1]
            .text
            .contains(&format!("https://share.example/w/{}", share.id))
    );

    // Bob's role changes and Erin comes: Erin alone is written to.
    let _: WorkspaceShareStatus = call(
        reqwest::Method::PATCH,
        format!("{}/v1/workspace-shares/{}", s.base, share.id),
        json!({"guests": [
            {"email": "alice@example.com", "role": "suggest"},
            {"email": "bob@example.com", "role": "suggest"},
            {"email": "erin@example.com", "role": "viewer"},
        ]}),
    )
    .await;
    assert_eq!(
        s.sent(),
        vec![invited(
            "erin@example.com",
            "Carol shared “Cut the project” with you"
        )]
    );
}
