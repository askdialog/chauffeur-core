//! What the share's page offers someone not signed in (docs/adr/share-with-any-email.md): who
//! shared it ("Alice shared a design with you"), and a sign-in link e-mailed to an invitee who
//! has no Google account of their address. Also the page that stops every e-mail to an address.
//!
//! A sign-in link goes only to an address the share names, at most `SIGN_IN_LINKS_AN_HOUR` an
//! hour, never to a blocked address; the page hears the same answer either way, so that it tells
//! no one who was invited. The link carries its token in the page's fragment (never sent to a
//! server, so a mail scanner fetching it signs no one in); the page trades it once, within 15
//! minutes, for a session of that address. Such a session has no Google Workspace domain: it
//! opens what names the address, never a team's share.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use chauffeur_protocol::client::CommentAuthor;
use chauffeur_protocol::design_share::{ShareAbout, SignInLinkParams, SignInLinkSessionParams};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::mail::{self, escape};
use crate::store::EmailKind;
use crate::{Identity, Reject, Service, host, internal, now_ms, random_hex};

/// How long a sign-in link signs in.
pub const SIGN_IN_LINK_FOR_MS: u64 = 15 * 60 * 1000;
/// Sign-in links an address gets in an hour, at most.
pub const SIGN_IN_LINKS_AN_HOUR: u32 = 5;
const HOUR_MS: u64 = 3600 * 1000;
/// The answer to every request of a sign-in link.
pub const ON_ITS_WAY: &str = "If this address was invited, a link is on its way.";

fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// An address as the service keeps it, or a refusal naming it.
fn address(email: &str) -> Result<String, Reject> {
    let email = email.trim().to_lowercase();
    if !email.contains('@') || email.contains(char::is_whitespace) {
        return Err(Reject(
            StatusCode::BAD_REQUEST,
            format!("{email} is not an e-mail address"),
        ));
    }
    Ok(email)
}

impl Service {
    /// E-mails a sign-in link to `email` for the share at `page`, unless the address is blocked
    /// or had its share of links this hour.
    fn send_sign_in_link(
        &self,
        email: &str,
        owner: &CommentAuthor,
        title: &str,
        page: &str,
    ) -> Result<(), Reject> {
        let _counting = self.mail_count.lock().unwrap();
        let now = now_ms();
        if self.store.blocked(email).map_err(internal)? {
            tracing::info!(to = email, "no sign-in link to a blocked address");
            return Ok(());
        }
        let sent = self
            .store
            .sign_in_links_since(email, now.saturating_sub(HOUR_MS))
            .map_err(internal)?;
        if sent >= SIGN_IN_LINKS_AN_HOUR {
            tracing::info!(to = email, "no sign-in link: the hour's are sent");
            return Ok(());
        }
        let token = random_hex(32);
        self.store
            .add_sign_in_link(&token_hash(&token), email, now)
            .map_err(internal)?;
        self.store
            .log_email(EmailKind::SignInLink, None, email, now)
            .map_err(internal)?;
        self.config.mailer.send(vec![mail::sign_in_link(
            owner,
            email,
            title,
            &format!("{page}#sign-in={token}"),
            &self.footer(email),
        )]);
        Ok(())
    }
}

fn on_its_way() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "message": ON_ITS_WAY })),
    )
}

pub(crate) async fn design_about(
    State(service): State<Arc<Service>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<ShareAbout>, Reject> {
    let share = service.share(&id)?;
    if share.stopped() {
        return Err(Reject(StatusCode::GONE, "This share was stopped".into()));
    }
    Ok(Json(ShareAbout {
        owner: share.owner.name,
    }))
}

pub(crate) async fn workspace_about(
    State(service): State<Arc<Service>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<ShareAbout>, Reject> {
    let share = service.workspace_share(&id)?;
    if share.stopped() {
        return Err(Reject(StatusCode::GONE, "This share was stopped".into()));
    }
    Ok(Json(ShareAbout {
        owner: share.owner.name,
    }))
}

#[derive(Deserialize)]
pub(crate) struct DesignLinkParams {
    email: String,
    screen: Option<String>,
    thread: Option<String>,
    mobile_auth: Option<crate::mobile::Authorization>,
}

pub(crate) async fn design_link(
    State(service): State<Arc<Service>>,
    UrlPath(id): UrlPath<String>,
    Json(params): Json<DesignLinkParams>,
) -> Result<(StatusCode, Json<serde_json::Value>), Reject> {
    let email = address(&params.email)?;
    let target_query = crate::mobile::email_query(
        params.screen.as_deref(),
        params.thread.as_deref(),
        params.mobile_auth.as_ref(),
    )?;
    if let Ok(share) = service.share(&id)
        && !share.stopped()
        && share.named(&email)
    {
        let page = format!("{}{}", service.share_url(&share), target_query);
        service.send_sign_in_link(&email, &share.owner, &share.title, &page)?;
    }
    Ok(on_its_way())
}

pub(crate) async fn workspace_link(
    State(service): State<Arc<Service>>,
    UrlPath(id): UrlPath<String>,
    Json(params): Json<SignInLinkParams>,
) -> Result<(StatusCode, Json<serde_json::Value>), Reject> {
    let email = address(&params.email)?;
    if let Ok(share) = service.workspace_share(&id)
        && !share.stopped()
        && share.guests.iter().any(|g| g.email == email)
    {
        let page = format!("{}/w/{}", service.workspace_url(), share.id);
        service.send_sign_in_link(&email, &share.owner, &share.title, &page)?;
    }
    Ok(on_its_way())
}

/// `POST /v1/session/link`: a sign-in link's token, once and while it is fresh, for a session of
/// its address.
pub(crate) async fn session_from_link(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    Json(params): Json<SignInLinkSessionParams>,
) -> Result<Response, Reject> {
    let now = now_ms();
    let email = service
        .store
        .use_sign_in_link(
            &token_hash(params.token.trim()),
            now.saturating_sub(SIGN_IN_LINK_FOR_MS),
            now,
        )
        .map_err(internal)?
        .ok_or_else(|| {
            Reject(
                StatusCode::UNAUTHORIZED,
                "This sign-in link expired or was used already: ask for a new one".into(),
            )
        })?;
    let identity = Identity::new(&email, None);
    tracing::info!(email, "signed in with a sign-in link");
    Ok((
        [(
            header::SET_COOKIE,
            service.sessions.issue(&identity, host(&headers)),
        )],
        Json(identity),
    )
        .into_response())
}

/// The address a stop link names and the service's signature of it.
#[derive(Deserialize)]
pub(crate) struct StopLink {
    to: String,
    sig: String,
}

impl Service {
    /// The address of a stop link the service wrote; none for another.
    fn stop_address(&self, link: &StopLink) -> Option<String> {
        let to = link.to.trim().to_lowercase();
        (link.sig == self.stop_signature(&to)).then_some(to)
    }
}

/// A small page of the service, outside the share's page: a title and a few words or a form.
fn small_page(status: StatusCode, title: &str, body: &str) -> Response {
    (
        status,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::X_FRAME_OPTIONS, "DENY"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        Html(format!(
            "<!doctype html><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\">\
             <meta name=\"robots\" content=\"noindex\"><title>{title}</title>\
             <body style=\"font:15px/1.5 -apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;color:#1d1d1f;max-width:420px;margin:15vh auto;padding:0 24px\">\
             <h1 style=\"font-size:20px\">{title}</h1>{body}</body>",
            title = escape(title),
        )),
    )
        .into_response()
}

fn not_ours() -> Response {
    small_page(
        StatusCode::BAD_REQUEST,
        "This link is not one of Chauffeur's",
        "<p>Open the link of the e-mail again, whole.</p>",
    )
}

/// `GET /v1/stop-emails`: asks before blocking, so that a mail scanner opening the link blocks
/// nothing.
pub(crate) async fn stop_page(
    State(service): State<Arc<Service>>,
    Query(link): Query<StopLink>,
) -> Response {
    let Some(to) = service.stop_address(&link) else {
        return not_ours();
    };
    small_page(
        StatusCode::OK,
        "Stop these emails",
        &format!(
            "<p>Chauffeur will send no more e-mails to {}: no invitations, no sign-in links.</p>\
             <form method=\"post\"><button style=\"font:inherit;padding:8px 16px;border-radius:8px;border:none;background:#1d1d1f;color:#fff;cursor:pointer\">Stop these emails</button></form>",
            escape(&to)
        ),
    )
}

/// `POST /v1/stop-emails`: blocks every later e-mail to the address.
pub(crate) async fn stop_emails(
    State(service): State<Arc<Service>>,
    Query(link): Query<StopLink>,
) -> Response {
    let Some(to) = service.stop_address(&link) else {
        return not_ours();
    };
    if let Err(error) = service.store.block_address(&to, now_ms()) {
        tracing::error!(%error, "could not block an address");
        return small_page(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Something failed",
            "<p>Chauffeur could not record it: try again in a moment.</p>",
        );
    }
    tracing::info!(to, "e-mails stopped");
    small_page(
        StatusCode::OK,
        "Done",
        &format!(
            "<p>Chauffeur will send no more e-mails to {}.</p>",
            escape(&to)
        ),
    )
}
