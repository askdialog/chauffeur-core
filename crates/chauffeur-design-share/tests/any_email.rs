//! Sharing with any e-mail address (docs/adr/share-with-any-email.md), end to end on loopback with
//! `dev:` identities (`dev:<email>` is a Google account of its domain's Workspace, but at
//! gmail.com): who gets in with Google, sign-in links, team domains, the limits on owners who are
//! not members, and the addresses that stop Chauffeur's e-mails.

use std::sync::{Arc, Mutex};

use chauffeur_design_share::mail::{Email, Mailer};
use chauffeur_design_share::{Auth, Config, Service, data_paths, store::Store};
use chauffeur_protocol::design_share::{ShareAbout, ShareStatus};
use chauffeur_protocol::workspace_share::WorkspaceShareStatus;
use reqwest::StatusCode;
use serde_json::json;

struct Server {
    _dir: tempfile::TempDir,
    base: String,
    sent: Arc<Mutex<Vec<Email>>>,
    http: reqwest::Client,
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
            public_url: "https://share.example".into(),
            workspace_url: None,
            cookie_domain: Some("chauffeur.example".into()),
            member_domain: Some("example.com".into()),
            site_url: Some("https://chauffeur.example".into()),
            session_key: Some([3; 32]),
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
        http: reqwest::Client::new(),
    }
}

impl Server {
    /// The e-mails sent since the last call.
    fn sent(&self) -> Vec<Email> {
        self.sent.lock().unwrap().drain(..).collect()
    }

    async fn share_as(&self, owner: &str, body: serde_json::Value) -> reqwest::Response {
        self.http
            .post(format!("{}/v1/shares", self.base))
            .header("authorization", format!("Bearer dev:{owner}"))
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    async fn cookie(&self, email: &str) -> String {
        let response = self
            .http
            .post(format!("{}/v1/session", self.base))
            .json(&json!({"id_token": format!("dev:{email}")}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{email} signs in");
        cookie_of(&response)
    }

    async fn snapshot(&self, share: &str, cookie: &str) -> StatusCode {
        self.http
            .get(format!("{}/v1/shares/{share}/snapshot", self.base))
            .header("cookie", cookie)
            .send()
            .await
            .unwrap()
            .status()
    }

    async fn ask_link(&self, share: &str, email: &str) -> reqwest::Response {
        self.http
            .post(format!("{}/v1/shares/{share}/sign-in-link", self.base))
            .json(&json!({ "email": email }))
            .send()
            .await
            .unwrap()
    }

    async fn use_link(&self, token: &str) -> reqwest::Response {
        self.http
            .post(format!("{}/v1/session/link", self.base))
            .json(&json!({ "token": token }))
            .send()
            .await
            .unwrap()
    }
}

fn cookie_of(response: &reqwest::Response) -> String {
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

/// The token of the sign-in link an e-mail carries.
fn token_in(email: &Email) -> String {
    let at = email.text.find("#sign-in=").expect("a sign-in link") + "#sign-in=".len();
    email.text[at..]
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect()
}

/// The stop link an e-mail ends with, relative to the service.
fn stop_link_in(email: &Email) -> String {
    let at = email.text.find("Stop these emails: ").unwrap() + "Stop these emails: ".len();
    let link = email.text[at..].lines().next().unwrap();
    link.strip_prefix("https://share.example")
        .unwrap()
        .to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_personal_google_account_opens_what_names_it_and_nothing_else() {
    let s = server().await;
    let named: ShareStatus = s
        .share_as(
            "carol@gmail.com",
            json!({"title": "Pricing filters", "reviewers": ["bob@gmail.com"]}),
        )
        .await
        .json()
        .await
        .unwrap();
    let invitation = s.sent().pop().unwrap();
    assert_eq!(invitation.to, "bob@gmail.com");
    assert_eq!(
        invitation.subject,
        "Carol shared “Pricing filters” with you"
    );
    assert!(
        !invitation.text.contains("#sign-in="),
        "never a sign-in link"
    );
    let other: ShareStatus = s
        .share_as("carol@gmail.com", json!({"reviewers": ["dave@gmail.com"]}))
        .await
        .json()
        .await
        .unwrap();

    let bob = s.cookie("bob@gmail.com").await;
    assert_eq!(s.snapshot(&named.id, &bob).await, 200);
    let refused = s
        .http
        .get(format!("{}/v1/shares/{}/snapshot", s.base, other.id))
        .header("cookie", &bob)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_team_share_opens_to_the_owner_s_workspace_domain_only() {
    let s = server().await;
    let team: ShareStatus = s
        .share_as(
            "carol@acme.com",
            json!({"title": "Onboarding", "team": true}),
        )
        .await
        .json()
        .await
        .unwrap();
    assert!(team.team);
    assert_eq!(
        s.snapshot(&team.id, &s.cookie("bob@acme.com").await).await,
        200
    );
    assert_eq!(
        s.snapshot(&team.id, &s.cookie("eve@other.com").await).await,
        403
    );
    assert_eq!(
        s.snapshot(&team.id, &s.cookie("eve@gmail.com").await).await,
        403
    );

    // A personal Google account has no team: told why, nothing saved.
    let refused = s
        .share_as("carol@gmail.com", json!({"title": "x", "team": true}))
        .await;
    assert_eq!(refused.status(), 400);
    let said: serde_json::Value = refused.json().await.unwrap();
    assert!(
        said["error"]
            .as_str()
            .unwrap()
            .contains("carol@gmail.com is not a Google Workspace account"),
        "{said}"
    );
    let workspace = s
        .http
        .post(format!("{}/v1/workspace-shares", s.base))
        .header("authorization", "Bearer dev:carol@gmail.com")
        .json(&json!({"team": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(workspace.status(), 400);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_link_goes_to_an_invitee_only_signs_in_once_and_never_into_a_team() {
    let s = server().await;
    let share: ShareStatus = s
        .share_as(
            "carol@acme.com",
            json!({"title": "CTA", "reviewers": ["bob@nogoogle.org"], "team": true}),
        )
        .await
        .json()
        .await
        .unwrap();
    s.sent();
    let about: ShareAbout = s
        .http
        .get(format!("{}/v1/shares/{}/about", s.base, share.id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(about.owner, "Carol");

    // The same answer for an invitee and a stranger; only the invitee gets an e-mail.
    for email in ["Bob@nogoogle.org", "mallory@acme.com"] {
        let asked = s.ask_link(&share.id, email).await;
        assert_eq!(asked.status(), 202);
        let said: serde_json::Value = asked.json().await.unwrap();
        assert_eq!(
            said["message"],
            "If this address was invited, a link is on its way."
        );
    }
    let mut sent = s.sent();
    assert_eq!(sent.len(), 1);
    let link = sent.pop().unwrap();
    assert_eq!(link.to, "bob@nogoogle.org");
    assert_eq!(link.reply_to, None);
    assert!(
        link.text
            .contains(&format!("https://share.example/s/{}#sign-in=", share.id))
    );

    let signed_in = s.use_link(&token_in(&link)).await;
    assert_eq!(signed_in.status(), 200);
    let bob = cookie_of(&signed_in);
    assert_eq!(s.snapshot(&share.id, &bob).await, 200);
    let again = s.use_link(&token_in(&link)).await;
    assert_eq!(again.status(), 401, "a link signs in once");
    let said: serde_json::Value = again.json().await.unwrap();
    assert!(
        said["error"]
            .as_str()
            .unwrap()
            .contains("ask for a new one")
    );
    assert_eq!(s.use_link("not-a-token").await.status(), 401);

    // An acme.com address signed in by a link is not of acme's Workspace for all that.
    s.http
        .patch(format!("{}/v1/shares/{}", s.base, share.id))
        .header("authorization", "Bearer dev:carol@acme.com")
        .json(&json!({"reviewers": ["bob@nogoogle.org", "dan@acme.com"]}))
        .send()
        .await
        .unwrap();
    let team_only: ShareStatus = s
        .share_as("carol@acme.com", json!({"title": "Team", "team": true}))
        .await
        .json()
        .await
        .unwrap();
    s.sent();
    s.ask_link(&share.id, "dan@acme.com").await;
    let dan = cookie_of(&s.use_link(&token_in(&s.sent()[0])).await);
    assert_eq!(s.snapshot(&share.id, &dan).await, 200, "named");
    assert_eq!(s.snapshot(&team_only.id, &dan).await, 403, "no team");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_address_gets_five_sign_in_links_an_hour_at_most() {
    let s = server().await;
    let share: ShareStatus = s
        .share_as("carol@acme.com", json!({"reviewers": ["bob@nogoogle.org"]}))
        .await
        .json()
        .await
        .unwrap();
    s.sent();
    for _ in 0..6 {
        assert_eq!(
            s.ask_link(&share.id, "bob@nogoogle.org").await.status(),
            202
        );
    }
    assert_eq!(s.sent().len(), 5);
    assert_eq!(s.ask_link(&share.id, "not an address").await.status(), 400);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_owner_who_is_not_a_member_names_twenty_people_and_sends_fifty_invitations_a_day() {
    let s = server().await;
    let people = |from: usize, to: usize| -> Vec<String> {
        (from..to).map(|i| format!("p{i}@gmail.com")).collect()
    };
    let refused = s
        .share_as("carol@acme.com", json!({"reviewers": people(0, 21)}))
        .await;
    assert_eq!(refused.status(), 400);
    let said: serde_json::Value = refused.json().await.unwrap();
    assert_eq!(
        said["error"],
        "A share names at most 20 people: remove some"
    );

    for batch in 0..2 {
        let status: ShareStatus = s
            .share_as(
                "carol@acme.com",
                json!({"reviewers": people(batch * 20, batch * 20 + 20)}),
            )
            .await
            .json()
            .await
            .unwrap();
        assert_eq!(status.notice, None);
    }
    assert_eq!(s.sent().len(), 40);
    // The third share is saved; ten of its twenty invitations wait, and the owner is told.
    let third = s
        .share_as("carol@acme.com", json!({"reviewers": people(40, 60)}))
        .await;
    assert_eq!(third.status(), 200);
    let third: ShareStatus = third.json().await.unwrap();
    assert_eq!(third.reviewers.len(), 20);
    assert_eq!(s.sent().len(), 10);
    let notice = third.notice.unwrap();
    assert!(notice.starts_with("The share is saved, but no invitation went to p50@gmail.com"));
    assert!(notice.contains("at most 50 invitations a day"));
    // A workspace share counts against the same day.
    let workspace: WorkspaceShareStatus = s
        .http
        .post(format!("{}/v1/workspace-shares", s.base))
        .header("authorization", "Bearer dev:carol@acme.com")
        .json(&json!({"guests": [{"email": "w@gmail.com", "role": "viewer"}]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(workspace.notice.unwrap().contains("w@gmail.com"));
    assert!(s.sent().is_empty());

    // A member has neither limit.
    let member: ShareStatus = s
        .share_as("alice@example.com", json!({"reviewers": people(0, 50)}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(member.notice, None);
    let more: ShareStatus = s
        .share_as("alice@example.com", json!({"reviewers": people(50, 60)}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(more.notice, None);
    assert_eq!(s.sent().len(), 60);
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_these_emails_blocks_invitations_and_sign_in_links_to_the_address() {
    let s = server().await;
    let first: ShareStatus = s
        .share_as("carol@acme.com", json!({"reviewers": ["bob@gmail.com"]}))
        .await
        .json()
        .await
        .unwrap();
    let invitation = s.sent().pop().unwrap();
    assert!(
        invitation
            .text
            .contains("Shared with Chauffeur: https://chauffeur.example")
    );
    let stop = stop_link_in(&invitation);

    // Opening the link asks first: a mail scanner blocks nothing.
    let page = s
        .http
        .get(format!("{}{stop}", s.base))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200);
    assert!(
        page.text()
            .await
            .unwrap()
            .contains("<form method=\"post\">")
    );
    let forged = s
        .http
        .post(format!(
            "{}/v1/stop-emails?to=bob%40gmail.com&sig=00",
            s.base
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(forged.status(), 400);
    let second: ShareStatus = s
        .share_as("carol@acme.com", json!({"reviewers": ["bob@gmail.com"]}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(second.notice, None, "not blocked yet");
    s.sent();

    let stopped = s
        .http
        .post(format!("{}{stop}", s.base))
        .send()
        .await
        .unwrap();
    assert_eq!(stopped.status(), 200);
    let third: ShareStatus = s
        .share_as(
            "carol@acme.com",
            json!({"reviewers": ["bob@gmail.com", "ann@gmail.com"]}),
        )
        .await
        .json()
        .await
        .unwrap();
    assert!(
        third
            .notice
            .unwrap()
            .contains("bob@gmail.com, who stopped e-mails from Chauffeur")
    );
    assert_eq!(
        s.sent().iter().map(|e| e.to.as_str()).collect::<Vec<_>>(),
        ["ann@gmail.com"]
    );
    assert_eq!(s.ask_link(&first.id, "bob@gmail.com").await.status(), 202);
    assert!(s.sent().is_empty(), "no sign-in link either");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_made_on_one_host_of_the_cookie_domain_serves_the_others() {
    let s = server().await;
    let share: ShareStatus = s
        .share_as("carol@acme.com", json!({"reviewers": ["bob@nogoogle.org"]}))
        .await
        .json()
        .await
        .unwrap();
    s.sent();
    let set_cookies = |response: &reqwest::Response| -> Vec<String> {
        response
            .headers()
            .get_all("set-cookie")
            .iter()
            .map(|v| v.to_str().unwrap().to_string())
            .collect()
    };

    // Google, on the design host: the session is the whole domain's, as Lax and HttpOnly as before.
    let google = s
        .http
        .post(format!("{}/v1/session", s.base))
        .header("host", "design.chauffeur.example")
        .json(&json!({"id_token": "dev:bob@nogoogle.org"}))
        .send()
        .await
        .unwrap();
    assert_eq!(google.status(), 200);
    let set = set_cookies(&google);
    assert_eq!(set.len(), 1);
    assert!(
        set[0].contains("; Domain=chauffeur.example")
            && set[0].contains("HttpOnly")
            && set[0].contains("SameSite=Lax"),
        "{set:?}"
    );
    assert_eq!(s.snapshot(&share.id, &cookie_of(&google)).await, 200);

    // A sign-in link, used on the workspace host: the same.
    s.ask_link(&share.id, "bob@nogoogle.org").await;
    let linked = s
        .http
        .post(format!("{}/v1/session/link", s.base))
        .header("host", "share.chauffeur.example")
        .json(&json!({ "token": token_in(&s.sent()[0]) }))
        .send()
        .await
        .unwrap();
    assert_eq!(linked.status(), 200);
    assert!(set_cookies(&linked)[0].contains("; Domain=chauffeur.example"));

    // Elsewhere (an older host, loopback) the session stays the host's own.
    let elsewhere = s
        .http
        .post(format!("{}/v1/session", s.base))
        .header("host", "design.old.example")
        .json(&json!({"id_token": "dev:bob@nogoogle.org"}))
        .send()
        .await
        .unwrap();
    assert!(!set_cookies(&elsewhere)[0].contains("Domain="));

    // Signing out ends the domain's session and any of the host's own.
    let out = s
        .http
        .delete(format!("{}/v1/session", s.base))
        .header("host", "share.chauffeur.example")
        .send()
        .await
        .unwrap();
    let cleared = set_cookies(&out);
    assert_eq!(cleared.len(), 2, "{cleared:?}");
    assert!(cleared.iter().all(|c| c.contains("Max-Age=0")));
    assert!(cleared[0].contains("; Domain=chauffeur.example") && !cleared[1].contains("Domain="));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_email_sign_in_preserves_the_mobile_pkce_continuation_and_exact_review_target() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use sha2::{Digest, Sha256};

    let s = server().await;
    let share: ShareStatus = s
        .share_as(
            "alice@example.com",
            json!({"reviewers":["bob@example.com"]}),
        )
        .await
        .json()
        .await
        .unwrap();
    s.sent();
    let verifier = "a".repeat(64);
    let state = "b".repeat(64);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mobile = json!({"challenge":challenge,"state":state,"screen":"missing screen.html","thread":"missing-thread"});
    let asked = s
        .http
        .post(format!("{}/v1/shares/{}/sign-in-link", s.base, share.id))
        .json(&json!({"email":"bob@example.com","mobile_auth":mobile}))
        .send()
        .await
        .unwrap();
    assert_eq!(asked.status(), 202);
    let sent = s.sent();
    assert_eq!(sent.len(), 1);
    let link = sent[0]
        .text
        .split_whitespace()
        .find(|part| part.starts_with("https://share.example/s/"))
        .unwrap();
    let url = url::Url::parse(link).unwrap();
    let query: std::collections::HashMap<_, _> = url.query_pairs().collect();
    assert_eq!(query["screen"], "missing screen.html");
    assert_eq!(query["thread"], "missing-thread");
    assert_eq!(query["challenge"], challenge);
    assert_eq!(query["state"], state);
    assert_eq!(query["mobile-auth"], "1");
    let signed_in = s.use_link(&token_in(&sent[0])).await;
    let cookie = cookie_of(&signed_in);
    let authorized = s
        .http
        .post(format!(
            "{}/v1/shares/{}/mobile-authorize",
            s.base, share.id
        ))
        .header("cookie", cookie)
        .header("origin", "https://share.example")
        .header("x-chauffeur-mobile", "1")
        .json(&mobile)
        .send()
        .await
        .unwrap();
    assert_eq!(
        authorized.status(),
        200,
        "the e-mailed session authorizes the native handoff"
    );
}
