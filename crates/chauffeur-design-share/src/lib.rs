//! The share service (docs/SPEC.md): the hosted, live copy of what a user shares with people they
//! name by e-mail, whatever their address, or with their team domain. A reader signs in with a
//! Google account of an invited address, or with a sign-in link e-mailed to it (sign_in.rs); an
//! account of the share's team domain gets in too.
//!
//! - A design share (story 51, `/s/<id>`): a design chat's canvas, for reviewers. The owner's
//!   daemon creates a share, uploads the design's files and keeps one WebSocket for the comments;
//!   a reviewer's page reads the files and the comments and keeps one WebSocket for what
//!   changes. It never sees a chat: a daemon sends only a design's files and `comments.json`.
//! - A workspace share (story 53, `/w/<id>`, workspace.rs): a workspace's chats, live, and the
//!   markdown of its notes, for guests who look or also suggest messages to its agents. The
//!   daemon pushes the chats' events (trimmed by it) and the notes; never terminals, diffs or the
//!   repos' files.
//!
//! The service keeps shares, guests, comments, chats' events and notes in SQLite and a design's
//! files on disk; it uses a session cookie for the page and read-only leases for sandboxed
//! screens. It e-mails invitees (mail.rs), within limits for owners who are not members.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{DefaultBodyLimit, Path as UrlPath, RawQuery, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{AppendHeaders, Html, IntoResponse, Response};
use axum::routing::{delete, get, post};
use chauffeur_protocol::client::CanvasResult;
use chauffeur_protocol::client::CommentAuthor;
use chauffeur_protocol::design_share::{
    MentionPlace, SHARE_PROTOCOL_VERSION, ShareParams, ShareSessionParams, ShareSnapshot,
    ShareStatus, SharedDesign, ToDaemon, ToReviewer,
};
use mail::{Email, Footer};
use serde::Serialize;
use tokio::sync::mpsc;

pub mod files;
pub mod google;
pub mod mail;
mod mentions;
mod mobile;
mod render;
pub mod ses;
pub mod session;
mod sign_in;
pub mod store;
mod workspace;
mod workspace_ws;
mod ws;

use files::Files;
use session::Sessions;
use store::{EmailKind, ShareRow, Store};

/// People a member's share names, at most.
const MAX_REVIEWERS: usize = 50;
/// People the share of an owner who is not a member names, at most.
const MAX_INVITEES: usize = 20;
const DAY_MS: u64 = 24 * 3600 * 1000;
const MAX_TITLE: usize = 200;
const DEFAULT_TITLE: &str = "Design";

/// Who a request is: a Google account's verified e-mail, or the address a sign-in link went to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Identity {
    /// Lowercase.
    pub email: String,
    pub name: String,
    /// The account's Google Workspace domain, lowercase, from its ID token: none for a personal
    /// Google account or a sign-in link. What lets it into a share's team domain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hd: Option<String>,
}

impl Identity {
    /// With the e-mail's first name when Google gave none.
    pub fn new(email: &str, name: Option<&str>) -> Identity {
        let email = email.trim().to_lowercase();
        let name = name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| first_name(&email));
        Identity {
            email,
            name,
            hd: None,
        }
    }

    /// The same, of the Google Workspace domain `hd` when there is one.
    pub fn in_workspace(self, hd: Option<&str>) -> Identity {
        Identity {
            hd: hd
                .map(|hd| hd.trim().to_lowercase())
                .filter(|hd| !hd.is_empty()),
            ..self
        }
    }

    pub fn author(&self) -> CommentAuthor {
        CommentAuthor {
            name: self.name.clone(),
            email: self.email.clone(),
        }
    }
}

/// What a person is called before the service knows their name: their address's first word.
pub(crate) fn first_name(email: &str) -> String {
    let local = email.split('@').next().unwrap_or(email);
    let first = local.split(['.', '_', '-']).next().unwrap_or(local);
    let mut chars = first.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

/// How a daemon or a page proves who its user is.
pub enum Auth {
    /// A Google ID token for one of our OAuth clients, of any Google account.
    Google(google::Verifier),
    /// `dev:<email>` in place of a token, unchecked: for a server on loopback, in development
    /// and tests. The account is of the Google Workspace of its e-mail's domain, but at
    /// `gmail.com`, a personal account.
    InsecureDev,
}

pub struct Config {
    /// Where reviewers open design shares, without a trailing slash (`https://design.example.com`);
    /// also the address of the links that stop e-mails.
    pub public_url: String,
    /// Where guests open workspace shares, without a trailing slash (`https://share.example.com`);
    /// none: `public_url`.
    pub workspace_url: Option<String>,
    /// The parent domain of the service's hosts (`example.com`): a session made on one of them
    /// serves all of them. None: each host signs in on its own.
    pub cookie_domain: Option<String>,
    /// The members' domain (`example.com`): an owner of an address of it has no limit on
    /// invitations. None: every owner has.
    pub member_domain: Option<String>,
    /// The site "Shared with Chauffeur" links to, in e-mails and on the page; none: no link.
    pub site_url: Option<String>,
    /// The key sessions and the links that stop e-mails are signed with, kept by the operator;
    /// none draws one at start, and a restart signs everyone out.
    pub session_key: Option<[u8; 32]>,
    /// The web OAuth client the page signs reviewers in with; none in development.
    pub web_client_id: Option<String>,
    /// The reviewer page's build (`index.html`, `assets/`); none serves a placeholder.
    pub web_dir: Option<PathBuf>,
    /// `Secure` on the session cookie: off for a server on plain http in development.
    pub secure_cookies: bool,
    /// How invitations and sign-in links leave.
    pub mailer: mail::Mailer,
}

pub struct Service {
    auth: Auth,
    pub store: Store,
    pub files: Files,
    sessions: Sessions,
    renders: render::Renders,
    mobile: mobile::Codes,
    config: Config,
    next_id: AtomicU64,
    live: Mutex<HashMap<String, Live>>,
    workspace_live: Mutex<HashMap<String, workspace::Live>>,
    /// The signed-in daemons listening for their user's mentions, by lowercase e-mail.
    users: Mutex<HashMap<String, Vec<mentions::UserConnection>>>,
    /// Held from counting an address's or owner's e-mails to recording those sent, so that two
    /// requests at once never pass a limit together.
    mail_count: Mutex<()>,
}

/// Who is connected to a share now.
#[derive(Default)]
struct Live {
    daemon: Option<(u64, mpsc::UnboundedSender<ToDaemon>)>,
    reviewers: Vec<ReviewerConnection>,
}

struct ReviewerConnection {
    id: u64,
    identity: Identity,
    tx: mpsc::UnboundedSender<ToReviewer>,
}

/// A refusal the client reads: the status, and one sentence.
#[derive(Debug)]
pub struct Reject(StatusCode, String);

impl IntoResponse for Reject {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

fn internal(error: impl std::fmt::Display) -> Reject {
    tracing::error!(%error, "a request failed");
    Reject(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Something failed on the share service; try again".into(),
    )
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn random_hex(bytes: usize) -> String {
    (0..bytes)
        .map(|_| format!("{:02x}", rand::random::<u8>()))
        .collect()
}

impl Service {
    pub fn new(store: Store, files: Files, auth: Auth, config: Config) -> Arc<Service> {
        Arc::new(Service {
            auth,
            store,
            files,
            sessions: Sessions::new(config.session_key, config.secure_cookies)
                .for_domain(config.cookie_domain.as_deref()),
            renders: render::Renders::default(),
            mobile: mobile::Codes::default(),
            config,
            next_id: AtomicU64::new(1),
            live: Mutex::new(HashMap::new()),
            workspace_live: Mutex::new(HashMap::new()),
            users: Mutex::new(HashMap::new()),
            mail_count: Mutex::new(()),
        })
    }

    pub fn router(self: &Arc<Service>) -> Router {
        Router::new()
            .route("/health", get(|| async { "ok" }))
            .route("/", get(|| async { "Chauffeur shares" }))
            .route("/v1/config", get(config))
            .route("/v1/shares", get(list_shares).post(create_share))
            .route(
                "/v1/shares/{id}",
                get(get_share).patch(patch_share).delete(delete_share),
            )
            .route(
                "/v1/shares/{id}/files/{*path}",
                get(get_file)
                    .put(put_file)
                    .delete(delete_file)
                    .layer(DefaultBodyLimit::max(files::MAX_FILE_BYTES + 1024)),
            )
            .route("/v1/shares/{id}/daemon", get(daemon_upgrade))
            .route("/v1/shares/{id}/snapshot", get(snapshot))
            .route("/v1/shares/{id}/canvas", get(canvas))
            .route("/v1/shares/{id}/render/{token}/{*path}", get(render_file))
            .route("/v1/shares/{id}/reviewer", get(reviewer_upgrade))
            .route("/v1/shares/{id}/about", get(sign_in::design_about))
            .route("/v1/shares/{id}/mobile-authorize", post(mobile::authorize))
            .route("/v1/shares/{id}/mobile-session", post(mobile::exchange))
            .route("/v1/shares/{id}/sign-in-link", post(sign_in::design_link))
            .route("/v1/workspace-shares", post(workspace::create))
            .route(
                "/v1/workspace-shares/{id}",
                get(workspace::get)
                    .patch(workspace::patch)
                    .delete(workspace::delete),
            )
            .route(
                "/v1/workspace-shares/{id}/daemon",
                get(workspace::daemon_upgrade),
            )
            .route(
                "/v1/workspace-shares/{id}/snapshot",
                get(workspace::snapshot),
            )
            .route(
                "/v1/workspace-shares/{id}/chats/{chat}/events",
                get(workspace::chat_events),
            )
            .route(
                "/v1/workspace-shares/{id}/comments",
                post(workspace::add_comment),
            )
            .route(
                "/v1/workspace-shares/{id}/comments/{comment}",
                delete(workspace::delete_comment_of_owner),
            )
            .route(
                "/v1/workspace-shares/{id}/guest",
                get(workspace::guest_upgrade),
            )
            .route(
                "/v1/workspace-shares/{id}/about",
                get(sign_in::workspace_about),
            )
            .route(
                "/v1/workspace-shares/{id}/sign-in-link",
                post(sign_in::workspace_link),
            )
            .route("/v1/session", post(session).delete(end_session))
            .route("/v1/session/link", post(sign_in::session_from_link))
            .route(
                "/v1/stop-emails",
                get(sign_in::stop_page).post(sign_in::stop_emails),
            )
            .route("/v1/me", get(me))
            .route("/v1/mentions/daemon", get(mentions_upgrade))
            .route("/s/{id}", get(page))
            .route("/w/{id}", get(page))
            .route("/assets/{*path}", get(asset))
            .with_state(self.clone())
    }

    /// Where a workspace share's guests open it, without a trailing slash.
    pub(crate) fn workspace_url(&self) -> &str {
        self.config
            .workspace_url
            .as_deref()
            .unwrap_or(&self.config.public_url)
    }

    pub(crate) fn next_connection(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Who a token is: Google's word, or `dev:<email>` on a development server.
    pub(crate) async fn identity_from_token(&self, token: &str) -> Result<Identity, String> {
        match &self.auth {
            Auth::Google(verifier) => {
                let identity = verifier.verify(token).await?;
                self.learn_name(&identity);
                Ok(identity)
            }
            Auth::InsecureDev => {
                let email = token
                    .strip_prefix("dev:")
                    .ok_or("This development server takes `dev:<email>` as a token")?;
                let domain = email
                    .rsplit_once('@')
                    .map(|(_, domain)| domain)
                    .ok_or_else(|| format!("{email} is not an e-mail address"))?;
                let hd = (!domain.eq_ignore_ascii_case("gmail.com")).then_some(domain);
                Ok(Identity::new(email, None).in_workspace(hd))
            }
        }
    }

    /// The daemon's identity: its Google ID token as a bearer.
    async fn daemon_identity(&self, headers: &HeaderMap) -> Result<Identity, Reject> {
        let token = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(|| {
                Reject(
                    StatusCode::UNAUTHORIZED,
                    "Sign in with Google to share a design".into(),
                )
            })?;
        self.identity_from_token(token)
            .await
            .map_err(|reason| Reject(StatusCode::UNAUTHORIZED, reason))
    }

    /// A reviewer's identity: the session cookie.
    fn reviewer_identity(&self, headers: &HeaderMap) -> Result<Identity, Reject> {
        self.sessions
            .identity(headers.get(header::COOKIE).and_then(|v| v.to_str().ok()))
            .ok_or_else(|| {
                Reject(
                    StatusCode::UNAUTHORIZED,
                    "Sign in with Google to see this design".into(),
                )
            })
    }

    fn share(&self, id: &str) -> Result<ShareRow, Reject> {
        self.store
            .share(id)
            .map_err(internal)?
            .ok_or_else(|| Reject(StatusCode::NOT_FOUND, "There is no such share".into()))
    }

    /// The share and its owner, from their daemon (a bearer) or their own page (the cookie).
    async fn owned(&self, headers: &HeaderMap, id: &str) -> Result<(Identity, ShareRow), Reject> {
        let identity = if headers.contains_key(header::AUTHORIZATION) {
            self.daemon_identity(headers).await?
        } else {
            self.reviewer_identity(headers)?
        };
        let share = self.share(id)?;
        if share.owner.email != identity.email {
            return Err(Reject(
                StatusCode::FORBIDDEN,
                "This share is not yours".into(),
            ));
        }
        Ok((identity, share))
    }

    /// Who reads a share: a page (the session cookie), or a daemon taking the design over
    /// (its Google ID token as a bearer).
    async fn viewer_identity(&self, headers: &HeaderMap) -> Result<Identity, Reject> {
        if headers.contains_key(header::AUTHORIZATION) {
            self.daemon_identity(headers).await
        } else {
            self.reviewer_identity(headers)
        }
    }

    /// The share, for someone named on it (or its owner) while it lives.
    pub(crate) fn viewable(&self, id: &str, identity: &Identity) -> Result<ShareRow, Reject> {
        let share = self.share(id)?;
        if share.stopped() {
            return Err(Reject(StatusCode::GONE, "This share was stopped".into()));
        }
        if !share.can_view(identity) {
            return Err(Reject(
                StatusCode::FORBIDDEN,
                format!(
                    "This design was not shared with you. Ask {}.",
                    share.owner.name
                ),
            ));
        }
        Ok(share)
    }

    pub(crate) fn viewers(&self, share_id: &str) -> Vec<String> {
        let live = self.live.lock().unwrap();
        let mut viewers: Vec<String> = live
            .get(share_id)
            .map(|l| {
                l.reviewers
                    .iter()
                    .map(|r| r.identity.email.clone())
                    .collect()
            })
            .unwrap_or_default();
        viewers.sort();
        viewers.dedup();
        viewers
    }

    pub(crate) fn online(&self, share_id: &str) -> bool {
        self.live
            .lock()
            .unwrap()
            .get(share_id)
            .is_some_and(|l| l.daemon.is_some())
    }

    pub(crate) fn status(&self, share: &ShareRow) -> ShareStatus {
        ShareStatus {
            id: share.id.clone(),
            url: format!("{}/s/{}", self.config.public_url, share.id),
            title: share.title.clone(),
            owner: share.owner.clone(),
            reviewers: share.reviewers.clone(),
            people: self.audience(share).people().to_vec(),
            team: share.team_domain.is_some(),
            viewers: self.viewers(&share.id),
            online: self.online(&share.id),
            last_push_at: share.last_push_at,
            stopped: share.stopped(),
            notice: None,
        }
    }

    /// A share as listed to someone it is open to: never its reviewers.
    pub(crate) fn listed(&self, share: &ShareRow, me: &Identity) -> SharedDesign {
        SharedDesign {
            id: share.id.clone(),
            url: format!("{}/s/{}", self.config.public_url, share.id),
            title: share.title.clone(),
            owner: share.owner.clone(),
            named: share.named(&me.email),
            team: share.team_domain.is_some(),
            online: self.online(&share.id),
            created_at: share.created_at,
            last_push_at: share.last_push_at,
            document: share.document.clone(),
        }
    }

    pub(crate) fn snapshot(
        &self,
        share: &ShareRow,
        me: &Identity,
    ) -> Result<ShareSnapshot, Reject> {
        Ok(ShareSnapshot {
            id: share.id.clone(),
            title: share.title.clone(),
            owner: share.owner.clone(),
            me: me.author(),
            canvas: self.render_canvas(&share.id, me),
            document: share.document.clone(),
            reviewers: if me.email == share.owner.email {
                share.reviewers.clone()
            } else {
                Vec::new()
            },
            people: self.audience(share).people().to_vec(),
            team_domain: share.team_domain.clone(),
            unread_mentions: self
                .store
                .unread_comments(MentionPlace::Design, &share.id, &me.email)
                .map_err(internal)?,
            files: self.files.list(&share.id),
            comments: self.store.comments(&share.id).map_err(internal)?,
            viewers: self.viewers(&share.id),
            online: self.online(&share.id),
            last_push_at: share.last_push_at,
        })
    }

    fn render_canvas(&self, share_id: &str, identity: &Identity) -> CanvasResult {
        let token = self.renders.issue(share_id, identity);
        self.files
            .canvas(share_id, &format!("/v1/shares/{share_id}/render/{token}/"))
    }

    pub(crate) fn with_live<T>(&self, share_id: &str, f: impl FnOnce(&mut Live) -> T) -> T {
        let mut live = self.live.lock().unwrap();
        let entry = live.entry(share_id.to_string()).or_default();
        let out = f(entry);
        if entry.daemon.is_none() && entry.reviewers.is_empty() {
            live.remove(share_id);
        }
        out
    }

    /// To the owner's own pages of the share only.
    pub(crate) fn tell_owner_pages(&self, share_id: &str, owner: &str, message: &ToReviewer) {
        let live = self.live.lock().unwrap();
        if let Some(l) = live.get(share_id) {
            for reviewer in l.reviewers.iter().filter(|r| r.identity.email == owner) {
                let _ = reviewer.tx.send(message.clone());
            }
        }
    }

    pub(crate) fn tell_reviewers(&self, share_id: &str, message: &ToReviewer) {
        let live = self.live.lock().unwrap();
        if let Some(l) = live.get(share_id) {
            for reviewer in &l.reviewers {
                let _ = reviewer.tx.send(message.clone());
            }
        }
    }

    pub(crate) fn tell_daemon(&self, share_id: &str, message: ToDaemon) {
        let live = self.live.lock().unwrap();
        if let Some((_, tx)) = live.get(share_id).and_then(|l| l.daemon.as_ref()) {
            let _ = tx.send(message);
        }
    }

    /// Whether an address is of the members' domain.
    pub(crate) fn member(&self, email: &str) -> bool {
        self.config
            .member_domain
            .as_deref()
            .is_some_and(|domain| email.ends_with(&format!("@{domain}")))
    }

    /// How many people a share of `owner` names, at most.
    pub(crate) fn max_invitees(&self, owner: &str) -> usize {
        if self.member(owner) {
            MAX_REVIEWERS
        } else {
            MAX_INVITEES
        }
    }

    /// The links an e-mail to `to` ends with: the one that stops e-mails to it, signed by the
    /// service, and the site's.
    pub(crate) fn footer(&self, to: &str) -> Footer {
        let address: String = url::form_urlencoded::byte_serialize(to.as_bytes()).collect();
        Footer {
            stop: format!(
                "{}/v1/stop-emails?to={address}&sig={}",
                self.config.public_url,
                self.stop_signature(to)
            ),
            site: self.config.site_url.clone(),
        }
    }

    pub(crate) fn stop_signature(&self, to: &str) -> String {
        self.sessions.sign(&format!("stop-emails:{to}"))
    }

    /// Writes to `invitees` by the rules of mail.rs: none to a blocked address, and at most
    /// `INVITATIONS_A_DAY` a day for an owner who is not a member. Returns what the owner is
    /// told of those held back.
    pub(crate) fn invite(
        &self,
        owner: &CommentAuthor,
        invitees: Vec<&str>,
        email_to: impl Fn(&str) -> Email,
    ) -> Result<Option<String>, Reject> {
        if invitees.is_empty() {
            return Ok(None);
        }
        let _counting = self.mail_count.lock().unwrap();
        let now = now_ms();
        let left = if self.member(&owner.email) {
            None
        } else {
            let sent = self
                .store
                .invitations_since(&owner.email, now.saturating_sub(DAY_MS))
                .map_err(internal)?;
            Some(mail::INVITATIONS_A_DAY.saturating_sub(sent))
        };
        let mut blocked = Vec::new();
        for to in &invitees {
            if self.store.blocked(to).map_err(internal)? {
                blocked.push(*to);
            }
        }
        let sorted = mail::sort(invitees, |to| blocked.contains(&to), left);
        for to in &sorted.send {
            self.store
                .log_email(EmailKind::Invitation, Some(&owner.email), to, now)
                .map_err(internal)?;
        }
        self.config
            .mailer
            .send(sorted.send.iter().map(|to| email_to(to)).collect());
        if !sorted.over_limit.is_empty() || !sorted.blocked.is_empty() {
            tracing::info!(
                owner = owner.email,
                over_limit = sorted.over_limit.len(),
                blocked = sorted.blocked.len(),
                "invitations held back"
            );
        }
        Ok(sorted.notice())
    }

    /// Writes to the share's reviewers not in `already` (mail.rs).
    fn invite_reviewers(
        &self,
        share: &ShareRow,
        already: &[&str],
    ) -> Result<Option<String>, Reject> {
        let link = format!("{}/s/{}", self.config.public_url, share.id);
        self.invite(
            &share.owner,
            mail::invitees(
                &share.owner.email,
                share.reviewers.iter().map(String::as_str),
                already,
            ),
            |to| {
                mail::invitation(
                    &share.owner,
                    to,
                    &share.title,
                    &link,
                    if share.document.is_some() {
                        mail::Shared::Document
                    } else {
                        mail::Shared::Design
                    },
                    &self.footer(to),
                )
            },
        )
    }

    /// The reviewers' page, built by the app (`app/share.html`), or a word while it is not.
    fn page_html(&self) -> String {
        self.config
            .web_dir
            .as_ref()
            .and_then(|dir| std::fs::read_to_string(dir.join("index.html")).ok())
            .unwrap_or_else(|| {
                "<!doctype html><meta charset=\"utf-8\"><title>Chauffeur design share</title>\
<p style=\"font: 14px system-ui; padding: 2em\">The reviewer page is not built on this server yet.</p>"
                    .into()
            })
    }
}

/// The team domain a share is opened to: none unless `team`, then the owner's Google Workspace
/// domain; refused for an account without one.
pub(crate) fn team_domain(owner: &Identity, team: bool) -> Result<Option<String>, Reject> {
    if !team {
        return Ok(None);
    }
    owner.hd.clone().map(Some).ok_or_else(|| {
        Reject(
            StatusCode::BAD_REQUEST,
            format!(
                "{} is not a Google Workspace account, so it has no team to open a share to: name the people instead",
                owner.email
            ),
        )
    })
}

/// The people a share names: e-mails lowercase, once, at most `max`.
fn valid_reviewers(reviewers: &[String], max: usize) -> Result<Vec<String>, Reject> {
    let mut out = Vec::new();
    for email in reviewers {
        let email = email.trim().to_lowercase();
        if email.is_empty() {
            continue;
        }
        if !email.contains('@') || email.contains(char::is_whitespace) {
            return Err(Reject(
                StatusCode::BAD_REQUEST,
                format!("{email} is not an e-mail address"),
            ));
        }
        if !out.contains(&email) {
            out.push(email);
        }
    }
    if out.len() > max {
        return Err(Reject(
            StatusCode::BAD_REQUEST,
            format!("A share names at most {max} people: remove some"),
        ));
    }
    Ok(out)
}

/// A markdown file's name as a share holds it: one name, no folder, ending in `.md`, `.mdx` or
/// `.markdown`.
fn valid_document(name: &str) -> Result<String, Reject> {
    let name = name.trim();
    let markdown = [".md", ".mdx", ".markdown"]
        .iter()
        .any(|ext| name.to_ascii_lowercase().ends_with(ext));
    if !markdown || name.contains(['/', '\\']) || name.starts_with('.') || name.len() > 200 {
        return Err(Reject(
            StatusCode::BAD_REQUEST,
            format!("{name} is not a markdown file's name"),
        ));
    }
    Ok(name.to_string())
}

fn valid_title(title: Option<&str>) -> String {
    let title = title
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or(DEFAULT_TITLE);
    title.chars().take(MAX_TITLE).collect()
}

// --- the owner's daemon ---

async fn create_share(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    Json(params): Json<ShareParams>,
) -> Result<Json<ShareStatus>, Reject> {
    let identity = service.daemon_identity(&headers).await?;
    let reviewers = valid_reviewers(
        params.reviewers.as_deref().unwrap_or_default(),
        service.max_invitees(&identity.email),
    )?;
    let team_domain = team_domain(&identity, params.team.unwrap_or(false))?;
    let document = params.document.as_deref().map(valid_document).transpose()?;
    let share = service
        .store
        .create_share(
            &random_hex(16),
            &identity.author(),
            &valid_title(params.title.as_deref()),
            &reviewers,
            team_domain.as_deref(),
            now_ms(),
        )
        .map_err(internal)?;
    tracing::info!(
        share = share.id,
        owner = identity.email,
        reviewers = reviewers.len(),
        team = share.team_domain,
        "share created"
    );
    let share = match document {
        Some(name) => {
            service
                .store
                .set_document(&share.id, &name)
                .map_err(internal)?;
            ShareRow {
                document: Some(name),
                ..share
            }
        }
        None => share,
    };
    let notice = service.invite_reviewers(&share, &[])?;
    Ok(Json(ShareStatus {
        notice,
        ..service.status(&share)
    }))
}

/// The live shares of others opened to the caller (named, or the team's), for their Office.
async fn list_shares(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
) -> Result<Json<Vec<SharedDesign>>, Reject> {
    let identity = service.viewer_identity(&headers).await?;
    let shares = service.store.shares_for(&identity).map_err(internal)?;
    Ok(Json(
        shares
            .iter()
            .map(|share| service.listed(share, &identity))
            .collect(),
    ))
}

async fn get_share(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<ShareStatus>, Reject> {
    let (_, share) = service.owned(&headers, &id).await?;
    Ok(Json(service.status(&share)))
}

async fn patch_share(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    Json(params): Json<ShareParams>,
) -> Result<Json<ShareStatus>, Reject> {
    let (identity, before) = service.owned(&headers, &id).await?;
    if before.stopped() {
        return Err(Reject(
            StatusCode::GONE,
            "This share was stopped: share again".into(),
        ));
    }
    let max = service.max_invitees(&identity.email);
    let reviewers = params
        .reviewers
        .as_deref()
        .map(|reviewers| valid_reviewers(reviewers, max))
        .transpose()?;
    let title = params.title.as_deref().map(|t| valid_title(Some(t)));
    let team = params
        .team
        .map(|team| team_domain(&identity, team))
        .transpose()?;
    service
        .store
        .update_share(
            &id,
            title.as_deref(),
            reviewers.as_deref(),
            team.as_ref().map(Option::as_deref),
        )
        .map_err(internal)?;
    let share = service.share(&id)?;
    let notice = service.invite_reviewers(
        &share,
        &before
            .reviewers
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    )?;
    // A reviewer taken off the list (or no longer let in by the team) is told at once.
    service.with_live(&id, |live| {
        for reviewer in live
            .reviewers
            .iter()
            .filter(|r| !share.can_view(&r.identity))
        {
            let _ = reviewer.tx.send(ToReviewer::Refused {
                reason: format!(
                    "This design is no longer shared with you. Ask {}.",
                    share.owner.name
                ),
            });
        }
    });
    // The owner's daemon and pages learn the list, wherever it was changed from.
    service.tell_owner_pages(
        &id,
        &share.owner.email,
        &ToReviewer::Reviewers {
            reviewers: share.reviewers.clone(),
        },
    );
    service.tell_daemon(
        &id,
        ToDaemon::Reviewers {
            reviewers: share.reviewers.clone(),
        },
    );
    // Every page hears who the share is open to; someone no longer let in loses their mentions.
    service.tell_people(&share);
    service.sync_mentions(&id);
    Ok(Json(ShareStatus {
        notice,
        ..service.status(&share)
    }))
}

async fn delete_share(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<ShareStatus>, Reject> {
    let (_, _) = service.owned(&headers, &id).await?;
    service.store.stop_share(&id, now_ms()).map_err(internal)?;
    service.files.remove_all(&id);
    service.tell_reviewers(
        &id,
        &ToReviewer::Refused {
            reason: "This share was stopped".into(),
        },
    );
    service.sync_mentions(&id);
    let share = service.share(&id)?;
    tracing::info!(share = id, "share stopped");
    Ok(Json(service.status(&share)))
}

async fn put_file(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    UrlPath((id, path)): UrlPath<(String, String)>,
    body: Bytes,
) -> Result<StatusCode, Reject> {
    let (_, share) = service.owned(&headers, &id).await?;
    if share.stopped() {
        return Err(Reject(StatusCode::GONE, "This share was stopped".into()));
    }
    service
        .files
        .write(&id, &path, &body)
        .map_err(|reason| Reject(StatusCode::BAD_REQUEST, reason))?;
    service.store.touch_push(&id, now_ms()).map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_file(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    UrlPath((id, path)): UrlPath<(String, String)>,
) -> Result<StatusCode, Reject> {
    let (_, _) = service.owned(&headers, &id).await?;
    service
        .files
        .remove(&id, &path)
        .map_err(|reason| Reject(StatusCode::BAD_REQUEST, reason))?;
    service.store.touch_push(&id, now_ms()).map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

/// A signed-in daemon listening for its user's mentions.
async fn mentions_upgrade(
    State(service): State<Arc<Service>>,
    upgrade: WebSocketUpgrade,
) -> Response {
    upgrade.on_upgrade(move |socket| mentions::user_socket(service, socket))
}

async fn daemon_upgrade(
    State(service): State<Arc<Service>>,
    UrlPath(id): UrlPath<String>,
    upgrade: WebSocketUpgrade,
) -> Response {
    upgrade.on_upgrade(move |socket| ws::daemon_socket(service, id, socket))
}

// --- the reviewers' page ---

async fn config(State(service): State<Arc<Service>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "web_client_id": service.config.web_client_id,
        "site_url": service.config.site_url,
        "version": SHARE_PROTOCOL_VERSION,
    }))
}

/// The `Host` a request came to, as the reverse proxy passed it on.
pub(crate) fn host(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::HOST).and_then(|v| v.to_str().ok())
}

async fn session(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    Json(params): Json<ShareSessionParams>,
) -> Result<Response, Reject> {
    let identity = service
        .identity_from_token(&params.id_token)
        .await
        .map_err(|reason| Reject(StatusCode::UNAUTHORIZED, reason))?;
    Ok((
        [(
            header::SET_COOKIE,
            service.sessions.issue(&identity, host(&headers)),
        )],
        Json(identity),
    )
        .into_response())
}

async fn end_session(State(service): State<Arc<Service>>, headers: HeaderMap) -> Response {
    let cleared = service.sessions.clear(host(&headers));
    (
        AppendHeaders(cleared.into_iter().map(|value| (header::SET_COOKIE, value))),
        StatusCode::NO_CONTENT,
    )
        .into_response()
}

async fn me(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
) -> Result<Json<Identity>, Reject> {
    Ok(Json(service.viewer_identity(&headers).await?))
}

async fn snapshot(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<ShareSnapshot>, Reject> {
    let identity = service.viewer_identity(&headers).await?;
    let share = service.viewable(&id, &identity)?;
    Ok(Json(service.snapshot(&share, &identity)?))
}

/// The canvas again, after the page heard `Files`.
async fn canvas(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<CanvasResult>, Reject> {
    let identity = service.viewer_identity(&headers).await?;
    let share = service.viewable(&id, &identity)?;
    Ok(Json(service.render_canvas(&share.id, &identity)))
}

async fn get_file(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    UrlPath((id, path)): UrlPath<(String, String)>,
    RawQuery(query): RawQuery,
) -> Result<Response, Reject> {
    let identity = service.viewer_identity(&headers).await?;
    service.viewable(&id, &identity)?;
    file_response(&service, &id, &path, query.as_deref())
}

async fn render_file(
    State(service): State<Arc<Service>>,
    UrlPath((id, token, path)): UrlPath<(String, String, String)>,
    RawQuery(query): RawQuery,
) -> Result<Response, Reject> {
    let identity = service.renders.identity(&id, &token).ok_or_else(|| {
        Reject(
            StatusCode::UNAUTHORIZED,
            "This screen's access expired; reload the shared design".into(),
        )
    })?;
    service.viewable(&id, &identity)?;
    let mut response = file_response(&service, &id, &path, query.as_deref())?;
    // Modules and fonts load from the iframe's opaque origin, without credentials.
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        axum::http::HeaderValue::from_static("*"),
    );
    Ok(response)
}

fn file_response(
    service: &Service,
    id: &str,
    path: &str,
    query: Option<&str>,
) -> Result<Response, Reject> {
    let (bytes, media_type) = service.files.read(id, path).ok_or_else(|| {
        Reject(
            StatusCode::NOT_FOUND,
            format!("{path} is not in this design"),
        )
    })?;
    let bytes = files::with_capture(bytes, media_type, query);
    Ok((
        [
            (header::CONTENT_TYPE, media_type),
            (header::CONTENT_SECURITY_POLICY, files::SCREEN_CSP),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        bytes,
    )
        .into_response())
}

async fn reviewer_upgrade(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, Reject> {
    let identity = service.reviewer_identity(&headers)?;
    service.viewable(&id, &identity)?;
    Ok(upgrade.on_upgrade(move |socket| ws::reviewer_socket(service, id, identity, socket)))
}

async fn page(State(service): State<Arc<Service>>, UrlPath(_id): UrlPath<String>) -> Response {
    (
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::X_FRAME_OPTIONS, "DENY"),
        ],
        Html(service.page_html()),
    )
        .into_response()
}

/// The page's own files, from its build; hashed names, cached for long.
async fn asset(State(service): State<Arc<Service>>, UrlPath(path): UrlPath<String>) -> Response {
    let not_found = || (StatusCode::NOT_FOUND, "not found").into_response();
    let Some(dir) = &service.config.web_dir else {
        return not_found();
    };
    let Some(relative) = files::safe_relative(&path) else {
        return not_found();
    };
    let file = dir.join("assets").join(relative);
    let Some(media_type) = files::content_type(&file) else {
        return not_found();
    };
    match tokio::fs::read(&file).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, media_type),
                (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => not_found(),
    }
}

/// Where the data lives: the SQLite file and the shares' files.
pub fn data_paths(data_dir: &Path) -> (PathBuf, Files) {
    (data_dir.join("shares.sqlite"), Files::new(data_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identity_has_a_name_from_google_or_its_email() {
        assert_eq!(Identity::new(" Alice.P@AskDialog.com ", None).name, "Alice");
        assert_eq!(
            Identity::new("bob@example.com", Some(" Bob L. ")).name,
            "Bob L."
        );
        assert_eq!(Identity::new("bob@example.com", Some("  ")).name, "Bob");
        assert_eq!(
            Identity::new("Bob@example.com", None).email,
            "bob@example.com"
        );
    }

    #[test]
    fn reviewers_are_emails_lowercase_once_and_bounded() {
        assert_eq!(
            valid_reviewers(
                &[
                    "Alice@example.com".into(),
                    "".into(),
                    "alice@example.com".into()
                ],
                MAX_INVITEES
            )
            .unwrap(),
            ["alice@example.com"]
        );
        assert!(valid_reviewers(&["alice".into()], MAX_INVITEES).is_err());
        assert!(valid_reviewers(&["lo uis@example.com".into()], MAX_INVITEES).is_err());
        let many: Vec<String> = (0..21).map(|i| format!("r{i}@example.com")).collect();
        assert!(valid_reviewers(&many, MAX_INVITEES).is_err());
        assert!(valid_reviewers(&many, MAX_REVIEWERS).is_ok());
        assert_eq!(valid_title(None), "Design");
        assert_eq!(valid_title(Some("  CTA  ")), "CTA");
    }

    #[test]
    fn only_a_google_workspace_account_opens_a_share_to_its_team_domain() {
        let workspace = Identity::new("carol@acme.com", None).in_workspace(Some("Acme.com"));
        assert_eq!(
            team_domain(&workspace, true).unwrap().as_deref(),
            Some("acme.com")
        );
        assert_eq!(team_domain(&workspace, false).unwrap(), None);
        let gmail = Identity::new("carol@gmail.com", None);
        assert_eq!(team_domain(&gmail, false).unwrap(), None);
        let refused = team_domain(&gmail, true).unwrap_err();
        assert_eq!(refused.0, StatusCode::BAD_REQUEST);
        assert!(
            refused
                .1
                .contains("carol@gmail.com is not a Google Workspace account")
        );
    }
}
