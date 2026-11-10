//! Native handoff exercises real HTTP cookies, Origin checks and redirects without an owner Mac.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chauffeur_design_share::{Auth, Config, Service, data_paths, mail::Mailer, store::Store};
use reqwest::{Client, Response};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const VERIFIER: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const STATE: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";

struct Server {
    _dir: tempfile::TempDir,
    base: String,
    http: Client,
}
impl Server {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (db, files) = data_paths(dir.path());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let service = Service::new(
            Store::open(&db).unwrap(),
            files,
            Auth::InsecureDev,
            Config {
                public_url: base.clone(),
                workspace_url: None,
                cookie_domain: None,
                member_domain: None,
                site_url: None,
                session_key: None,
                web_client_id: None,
                web_dir: None,
                secure_cookies: false,
                mailer: Mailer::Off,
            },
        );
        tokio::spawn(async move { axum::serve(listener, service.router()).await.unwrap() });
        Self {
            _dir: dir,
            base,
            http: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
        }
    }
    async fn share(&self) -> String {
        self.http
            .post(format!("{}/v1/shares", self.base))
            .bearer_auth("dev:alice@example.com")
            .json(&json!({"reviewers":["bob@example.com"]}))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    async fn cookie(&self, email: &str) -> String {
        let response = self
            .http
            .post(format!("{}/v1/session", self.base))
            .json(&json!({"id_token":format!("dev:{email}")}))
            .send()
            .await
            .unwrap();
        response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .into()
    }
    fn authorization(&self) -> Value {
        json!({"challenge":URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes())),"state":STATE,"screen":"screen one.html","thread":"thread-123"})
    }
    async fn authorize(
        &self,
        id: &str,
        cookie: &str,
        origin: Option<&str>,
        marker: bool,
        body: Value,
    ) -> Response {
        let mut request = self
            .http
            .post(format!("{}/v1/shares/{id}/mobile-authorize", self.base))
            .header("cookie", cookie)
            .json(&body);
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        if marker {
            request = request.header("x-chauffeur-mobile", "1");
        }
        request.send().await.unwrap()
    }
    async fn code(&self, id: &str, cookie: &str) -> String {
        let response = self
            .authorize(id, cookie, Some(&self.base), true, self.authorization())
            .await;
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let body: Value = response.json().await.unwrap();
        let callback = url::Url::parse(body["callback"].as_str().unwrap()).unwrap();
        assert_eq!(callback.scheme(), "chauffeur");
        assert_eq!(callback.host_str(), Some("review"));
        assert_eq!(callback.path(), "/auth");
        let params: std::collections::HashMap<_, _> = callback.query_pairs().collect();
        assert_eq!(params["state"], STATE);
        assert_eq!(
            params.len(),
            2,
            "callback contains only code and state, never a bearer"
        );
        params["code"].to_string()
    }
    async fn exchange(&self, id: &str, code: &str, verifier: &str, state: &str) -> Response {
        self.http
            .post(format!("{}/v1/shares/{id}/mobile-session", self.base))
            .header("origin", &self.base)
            .header("x-chauffeur-mobile", "1")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(form(code, verifier, state))
            .send()
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn a_native_handoff_sets_a_cookie_once_and_restores_the_exact_screen_and_thread() {
    let s = Server::new().await;
    let id = s.share().await;
    let cookie = s.cookie("bob@example.com").await;
    let code = s.code(&id, &cookie).await;
    for (share, verifier, state) in [
        (
            &id[..],
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            STATE,
        ),
        (&id[..], VERIFIER, "wrong"),
        ("another-share", VERIFIER, STATE),
    ] {
        assert_eq!(
            s.exchange(share, &code, verifier, state).await.status(),
            400
        );
    }
    let response = s.exchange(&id, &code, VERIFIER, STATE).await;
    assert_eq!(response.status(), 303);
    assert_eq!(
        response.headers()["location"],
        format!("/s/{id}?mobile=1&native=1&screen=screen+one.html&thread=thread-123")
    );
    let set = response.headers()["set-cookie"].to_str().unwrap();
    assert!(set.contains("HttpOnly") && set.contains("SameSite=Lax"));
    let snapshot = s
        .http
        .get(format!("{}/v1/shares/{id}/snapshot", s.base))
        .header("cookie", set.split(';').next().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(snapshot.status(), 200);
    assert_eq!(
        s.exchange(&id, &code, VERIFIER, STATE).await.status(),
        400,
        "single use"
    );
}

#[tokio::test]
async fn handoff_requires_a_same_origin_cookie_session_and_refuses_redirect_injection() {
    let s = Server::new().await;
    let id = s.share().await;
    let cookie = s.cookie("bob@example.com").await;
    for origin in [None, Some("null"), Some("https://evil.example")] {
        assert_eq!(
            s.authorize(&id, &cookie, origin, true, s.authorization())
                .await
                .status(),
            403
        );
    }
    assert_eq!(
        s.authorize(&id, &cookie, Some(&s.base), false, s.authorization())
            .await
            .status(),
        403
    );
    assert_eq!(
        s.authorize(&id, "", Some(&s.base), true, s.authorization())
            .await
            .status(),
        401
    );
    let outsider = s.cookie("eve@example.com").await;
    assert_eq!(
        s.authorize(&id, &outsider, Some(&s.base), true, s.authorization())
            .await
            .status(),
        403
    );
    for field in ["screen", "thread", "challenge", "state", "redirect_uri"] {
        let mut body = s.authorization();
        body[field] = json!("https://evil.example/steal");
        assert_eq!(
            s.authorize(&id, &cookie, Some(&s.base), true, body)
                .await
                .status(),
            if field == "redirect_uri" { 422 } else { 400 },
            "{field}"
        );
    }
    let code = s.code(&id, &cookie).await;
    let refused = s
        .http
        .post(format!("{}/v1/shares/{id}/mobile-session", s.base))
        .header("origin", "https://evil.example")
        .header("x-chauffeur-mobile", "1")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form(&code, VERIFIER, STATE))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
    assert!(!refused.headers().contains_key("set-cookie"));
    assert_eq!(s.exchange(&id, &code, VERIFIER, STATE).await.status(), 303);
}

#[tokio::test]
async fn revoked_or_stopped_shares_cannot_exchange_or_issue_mobile_sessions() {
    let s = Server::new().await;
    let cookie = s.cookie("bob@example.com").await;
    for stopped in [false, true] {
        let id = s.share().await;
        let code = s.code(&id, &cookie).await;
        let request = s
            .http
            .request(
                if stopped {
                    reqwest::Method::DELETE
                } else {
                    reqwest::Method::PATCH
                },
                format!("{}/v1/shares/{id}", s.base),
            )
            .bearer_auth("dev:alice@example.com");
        let response = if stopped {
            request.send().await.unwrap()
        } else {
            request.json(&json!({"reviewers":[]})).send().await.unwrap()
        };
        assert_eq!(response.status(), 200);
        let expected = if stopped { 410 } else { 403 };
        let response = s.exchange(&id, &code, VERIFIER, STATE).await;
        assert_eq!(response.status(), expected);
        assert!(!response.headers().contains_key("set-cookie"));
        assert_eq!(s.exchange(&id, &code, VERIFIER, STATE).await.status(), 400);
        assert_eq!(
            s.authorize(&id, &cookie, Some(&s.base), true, s.authorization())
                .await
                .status(),
            expected
        );
    }
}

fn form(code: &str, verifier: &str, state: &str) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .append_pair("code", code)
        .append_pair("verifier", verifier)
        .append_pair("state", state)
        .finish()
}
