//! Individual Slack sign-in; the daemon alone holds the desktop session credential.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chauffeur_protocol::client::{
    SlackContext, SlackDelivery, SlackIdentity, SlackLogin, SlackReceiveParams, SlackStatus,
};
use reqwest::{Client, Url};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    sync::{
        Mutex as StdMutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

use crate::{
    Error, Result,
    secrets::{SecretName, Secrets},
    settings::LiveSettings,
};
const SESSION: SecretName<'static> = SecretName {
    service: "chauffeur-slack",
    file: "slack-session",
    label: "Chauffeur: Slack session",
};
struct Pending {
    id: String,
    verifier: String,
    expires: Instant,
}
pub struct Slack {
    secrets: Secrets,
    /// `[slack] url`: the service the integration goes through; empty, it is off.
    settings: std::sync::Arc<LiveSettings>,
    pending: Mutex<Option<Pending>>,
    credential: StdMutex<Option<Option<String>>>,
    receive_down: AtomicBool,
}
fn unavailable() -> Error {
    Error::Unavailable("Connexion Slack indisponible ; réessaie dans Réglages → Slack.".into())
}
/// Logs why the service could not be reached: the client only sees `unavailable()`.
fn unreachable(error: reqwest::Error) -> Error {
    // Without the URL: a context link's path is the draft's capability.
    let error = error.without_url();
    tracing::warn!(%error, "could not reach the Slack imports service");
    unavailable()
}
/// The app polls in the background: the service is down, Settings would not help.
fn offline() -> Error {
    Error::Unavailable(
        "Le service d’import Slack est injoignable ; Chauffeur réessaie tout seul.".into(),
    )
}
fn signed_out() -> Error {
    Error::BadRequest(
        "Connecte ton compte dans Réglages → Slack, puis rouvre le message depuis Slack.".into(),
    )
}
fn client() -> Result<Client> {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client.clone());
    }
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| unavailable())?;
    let _ = CLIENT.set(client.clone());
    Ok(client)
}

impl Slack {
    pub fn new(secrets: Secrets, settings: std::sync::Arc<LiveSettings>) -> Self {
        Self {
            secrets,
            settings,
            pending: Mutex::new(None),
            credential: StdMutex::new(None),
            receive_down: AtomicBool::new(false),
        }
    }
    /// The Slack service, without a trailing slash; refused while `[slack] url` is empty.
    fn service(&self) -> Result<String> {
        let url = self
            .settings
            .get()
            .slack
            .url
            .trim()
            .trim_end_matches('/')
            .to_string();
        if url.is_empty() {
            return Err(Error::Unavailable(
                "Slack n’est pas configuré : renseigne [slack] url dans settings.toml.".into(),
            ));
        }
        Ok(url)
    }
    fn credential(&self) -> Result<Option<String>> {
        let mut cached = self.credential.lock().unwrap();
        if let Some(value) = cached.as_ref() {
            return Ok(value.clone());
        }
        // Desktop presence polls frequently; consult the system keychain only once.
        let value = self.secrets.read_protected(&SESSION).map_err(|_| Error::Unavailable(
            "Impossible de lire la connexion Slack ; déverrouille le trousseau système puis réessaie.".into()
        ))?;
        *cached = Some(value.clone());
        Ok(value)
    }
    pub async fn status(&self) -> Result<SlackStatus> {
        let mut pending = self.pending.lock().await;
        if pending
            .as_ref()
            .is_some_and(|p| p.expires <= Instant::now())
        {
            *pending = None;
        }
        let identity = if let Some(token) = self.credential()? {
            let response = client()?
                .get(format!("{}/slack/auth/me", self.service()?))
                .bearer_auth(token)
                .send()
                .await
                .map_err(unreachable)?;
            if response.status() == reqwest::StatusCode::UNAUTHORIZED {
                None
            } else {
                Some(read::<SlackIdentity>(response).await?)
            }
        } else {
            None
        };
        Ok(SlackStatus {
            identity,
            pending: pending.is_some(),
        })
    }
    pub async fn login(&self) -> Result<SlackLogin> {
        self.credential()?;
        let mut pending = self.pending.lock().await;
        self.cancel_pending(&mut pending).await?;
        let verifier = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        #[derive(Deserialize)]
        struct Started {
            id: String,
            url: String,
        }
        let started: Started = read(
            client()?
                .post(format!("{}/slack/auth/start", self.service()?))
                .json(&json!({"challenge":challenge}))
                .send()
                .await
                .map_err(unreachable)?,
        )
        .await?;
        let url = Url::parse(&started.url).map_err(|_| unavailable())?;
        if url.scheme() != "https"
            || url.host_str() != Some("slack.com")
            || url.path() != "/openid/connect/authorize"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || !valid_token(&started.id)
        {
            return Err(unavailable());
        }
        *pending = Some(Pending {
            id: started.id,
            verifier,
            expires: Instant::now() + Duration::from_secs(600),
        });
        Ok(SlackLogin { url: started.url })
    }
    pub async fn poll(&self) -> Result<SlackStatus> {
        let mut pending = self.pending.lock().await;
        let flow = pending.as_ref().ok_or_else(signed_out)?;
        if flow.expires <= Instant::now() {
            *pending = None;
            return Err(signed_out());
        }
        #[derive(Deserialize)]
        struct Completed {
            pending: bool,
            credential: Option<String>,
            identity: Option<SlackIdentity>,
        }
        let response = client()?
            .post(format!("{}/slack/auth/complete", self.service()?))
            .json(&json!({"id":flow.id,"verifier":flow.verifier}))
            .send()
            .await
            .map_err(unreachable)?;
        if matches!(response.status().as_u16(), 403 | 410) {
            *pending = None;
            return Err(signed_out());
        }
        let result: Completed = read(response).await?;
        if result.pending {
            return Ok(SlackStatus {
                identity: None,
                pending: true,
            });
        }
        let credential = result
            .credential
            .filter(|s| valid_token(s))
            .ok_or_else(unavailable)?;
        let identity = result.identity.ok_or_else(unavailable)?;
        self.secrets.store(&SESSION, &credential).map_err(|_| Error::Unavailable("Impossible d’enregistrer la connexion Slack ; déverrouille le trousseau puis réessaie.".into()))?;
        if self
            .secrets
            .read_protected(&SESSION)
            .map_err(|_| unavailable())?
            .as_deref()
            != Some(credential.as_str())
        {
            return Err(Error::Unavailable("Impossible de conserver la connexion Slack ; vérifie le trousseau système puis réessaie.".into()));
        }
        *self.credential.lock().unwrap() = Some(Some(credential));
        *pending = None;
        Ok(SlackStatus {
            identity: Some(identity),
            pending: false,
        })
    }
    async fn cancel_pending(&self, pending: &mut Option<Pending>) -> Result<()> {
        if let Some(flow) = pending.as_ref() {
            let response = client()?
                .post(format!("{}/slack/auth/cancel", self.service()?))
                .json(&json!({"id":flow.id,"verifier":flow.verifier}))
                .send()
                .await
                .map_err(unreachable)?;
            if !response.status().is_success() && response.status().as_u16() != 410 {
                return Err(unavailable());
            }
        }
        *pending = None;
        Ok(())
    }
    pub async fn cancel(&self) -> Result<()> {
        self.cancel_pending(&mut *self.pending.lock().await).await
    }
    pub async fn logout(&self) -> Result<()> {
        let mut pending = self.pending.lock().await;
        self.cancel_pending(&mut pending).await?;
        if let Some(token) = self.credential()? {
            let response = client()?
                .post(format!("{}/slack/auth/logout", self.service()?))
                .bearer_auth(token)
                .send()
                .await
                .map_err(unreachable)?;
            if !response.status().is_success() {
                return Err(unavailable());
            }
            self.secrets.delete(&SESSION).map_err(|_| unavailable())?;
            *self.credential.lock().unwrap() = Some(None);
        }
        Ok(())
    }
    pub async fn receive(&self, params: SlackReceiveParams) -> Result<SlackDelivery> {
        if params.ack.as_ref().is_some_and(|ack| !valid_token(ack)) {
            return Err(Error::BadRequest(
                "Accusé de réception Slack invalide.".into(),
            ));
        }
        let Some(token) = self.credential()? else {
            return Ok(SlackDelivery { link: None });
        };
        let response = match client()?
            .post(format!("{}/slack/desktop", self.service()?))
            .bearer_auth(token)
            .json(&params)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => return Err(self.receive_failed(error.without_url())),
        };
        let status = response.status();
        if !status.is_success() && status != reqwest::StatusCode::UNAUTHORIZED {
            return Err(self.receive_failed(status));
        }
        if self.receive_down.swap(false, Ordering::Relaxed) {
            tracing::info!("imports from Slack reach this desktop again");
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            // An expired or revoked session is a desktop that is not connected, as without one:
            // Settings → Slack shows it signed out and Slack's modal says how to reconnect.
            return Ok(SlackDelivery { link: None });
        }
        let delivery: SlackDelivery = read(response).await.map_err(|_| offline())?;
        if delivery
            .link
            .as_ref()
            .is_some_and(|link| context_path(link).is_none())
        {
            tracing::warn!("the Slack imports service delivered an invalid link");
            return Err(unavailable());
        }
        Ok(delivery)
    }
    /// Logs only when imports stop: the app polls every few seconds, offline for hours at times.
    fn receive_failed(&self, error: impl std::fmt::Display) -> Error {
        if !self.receive_down.swap(true, Ordering::Relaxed) {
            tracing::warn!(%error, "imports from Slack stopped reaching this desktop");
        }
        offline()
    }
    pub async fn context(&self, link: &str) -> Result<SlackContext> {
        let path = context_path(link).ok_or_else(|| {
            Error::BadRequest("Lien Slack invalide ; rouvre le message depuis Slack.".into())
        })?;
        let token = self.credential()?.ok_or_else(signed_out)?;
        let response = client()?
            .get(format!("{}{path}", self.service()?))
            .bearer_auth(token)
            .send()
            .await
            .map_err(unreachable)?;
        match response.status().as_u16() {
            401 => return Err(signed_out()),
            403 => return Err(Error::BadRequest("Ce message vient d’un autre compte Slack ; connecte le compte qui a utilisé le raccourci.".into())),
            404 | 410 => return Err(Error::BadRequest("Ce lien Slack a expiré ; rouvre le message depuis Slack.".into())),
            _ => {}
        }
        let context: SlackContext = read(response).await?;
        if context.prompt.trim().is_empty() {
            return Err(Error::BadRequest(
                "Ce message Slack est vide ; choisis un autre message.".into(),
            ));
        }
        Ok(context)
    }
}
fn valid_token(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
async fn read<T: serde::de::DeserializeOwned>(mut response: reqwest::Response) -> Result<T> {
    let status = response.status();
    if !status.is_success() {
        tracing::warn!(%status, "the Slack imports service refused the request");
        return Err(unavailable());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(unreachable)? {
        if body.len() + chunk.len() > 512 * 1024 {
            tracing::warn!("the Slack imports service answered more than 512 KiB");
            return Err(unavailable());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|error| {
        tracing::warn!(%error, "could not read the Slack imports service's answer");
        unavailable()
    })
}
/// The path of a `chauffeur://slack/<token>?scope=…` link on the Slack service; `None` for any
/// other link, so that a link never sends the session credential elsewhere.
fn context_path(link: &str) -> Option<String> {
    let link = Url::parse(link).ok()?;
    if link.scheme() != "chauffeur"
        || link.host_str() != Some("slack")
        || !link.username().is_empty()
        || link.password().is_some()
        || link.port().is_some()
        || link.fragment().is_some()
    {
        return None;
    }
    let token = link.path().strip_prefix('/')?;
    if token.len() != 43
        || !token
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
    {
        return None;
    }
    let params: Vec<_> = link.query_pairs().collect();
    if params.len() != 1
        || params[0].0 != "scope"
        || !matches!(params[0].1.as_ref(), "message" | "thread")
    {
        return None;
    }
    Some(format!("/slack/context/{token}?scope={}", params[0].1))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn a_fresh_daemon_does_not_contact_slack_or_import_without_signing_in() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::new(
            dir.path().join("home"),
            dir.path().join("workspaces"),
            dir.path().join("repos"),
        );
        let settings = std::sync::Arc::new(LiveSettings::load(&paths).unwrap());
        let slack = Slack::new(Secrets::for_paths(&paths), settings);
        let status = slack.status().await.unwrap();
        assert!(status.identity.is_none());
        assert!(!status.pending);
        assert!(
            slack
                .context(&format!(
                    "chauffeur://slack/{}?scope=message",
                    "a".repeat(43)
                ))
                .await
                .unwrap_err()
                .to_string()
                .contains("Connecte ton compte")
        );
        assert!(
            slack
                .receive(SlackReceiveParams {
                    focused: true,
                    ack: None
                })
                .await
                .unwrap()
                .link
                .is_none()
        );
        assert!(
            slack
                .receive(SlackReceiveParams {
                    focused: false,
                    ack: Some("invalid".into())
                })
                .await
                .is_err()
        );
        slack.cancel().await.unwrap();
        slack.logout().await.unwrap();
    }
    /// A signed-in daemon whose service answers every desktop poll with `status`.
    async fn signed_in_against(status: u16) -> (tempfile::TempDir, Slack) {
        let app = axum::Router::new().route(
            "/slack/desktop",
            axum::routing::post(move || async move {
                axum::http::StatusCode::from_u16(status).unwrap()
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::new(
            dir.path().join("home"),
            dir.path().join("workspaces"),
            dir.path().join("repos"),
        );
        std::fs::create_dir_all(dir.path().join("home")).unwrap();
        std::fs::write(
            &paths.settings,
            format!("[slack]\nurl = \"http://127.0.0.1:{port}\"\n"),
        )
        .unwrap();
        let settings = std::sync::Arc::new(LiveSettings::load(&paths).unwrap());
        let slack = Slack::new(Secrets::for_paths(&paths), settings);
        *slack.credential.lock().unwrap() = Some(Some("a".repeat(43)));
        (dir, slack)
    }
    fn poll() -> SlackReceiveParams {
        SlackReceiveParams {
            focused: true,
            ack: None,
        }
    }
    #[tokio::test]
    async fn an_expired_session_waits_for_a_new_sign_in_instead_of_failing_every_poll() {
        let (_dir, slack) = signed_in_against(401).await;
        assert!(slack.receive(poll()).await.unwrap().link.is_none());
    }
    #[tokio::test]
    async fn a_service_outage_says_chauffeur_retries_rather_than_sending_to_settings() {
        let (_dir, slack) = signed_in_against(502).await;
        let message = slack.receive(poll()).await.unwrap_err().to_string();
        assert!(message.contains("injoignable"), "{message}");
        assert!(!message.contains("Réglages"), "{message}");
    }
    #[test]
    fn a_context_link_cannot_redirect_the_session_credential_elsewhere() {
        let link = format!("chauffeur://slack/{}?scope=thread", "a".repeat(43));
        assert_eq!(
            context_path(&link).unwrap(),
            format!("/slack/context/{}?scope=thread", "a".repeat(43))
        );
        for bad in [
            link.replace("slack/", "evil.example/"),
            link.replace("slack/", "user@slack/"),
            format!("{link}&url=https://example.com"),
            format!("{link}#fragment"),
            link.replace("thread", "run"),
        ] {
            assert!(context_path(&bad).is_none());
        }
    }
}
