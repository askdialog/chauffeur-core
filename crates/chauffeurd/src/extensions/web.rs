//! What Core serves of extensions over HTTP: their installed UI files (`assets`), on Core's port
//! and on the Office's browser gateway, and that gateway itself.
//!
//! The gateway is the Office in the user's browser, for an app whose web view cannot make calls
//! (a WebKitGTK built without WebRTC, docs/office.md): Core serves the app's built UI
//! (`chauffeur-web` beside its binary) on a listener of its own, 127.0.0.1 on a random port, from
//! the first time it is asked for one, and the page shows the Office alone. It lives in Core, not
//! in the Office's engine, so a page keeps its session while the engine is updated or restarted.
//!
//! The page never gets the daemon's token. Its link carries a one-time code in its fragment
//! (never sent in a request nor a Referer), which the page exchanges once, within a minute, for a
//! session that lives in memory until the daemon stops. A session's WebSocket takes only what the
//! Office shows (`Access::Office`: the methods `ClientCall` marks `office`, and subscriptions to
//! the registry and the Office only); its bootstrap (`/extension-ui/office`) names the Office's
//! installed UI, whose files the gateway serves under its own origin, the Office's only.
//!
//! Every request must name this listener in its Host (a DNS rebinding names another); the
//! exchange and the WebSocket must come from the page's own origin (any web page may try
//! `ws://127.0.0.1`) and, on Linux, from a process of the daemon's user (`same_user`): another
//! user of the machine who read the link in a process list gets nothing from it.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use chauffeur_protocol::events::Event;
use chauffeur_protocol::extensions::{ExtensionId, ExtensionUi};
use chauffeur_protocol::office::OfficeEvent;
use serde::Deserialize;
use serde_json::json;

use crate::extensions::assets::{self, VerifiedRevision};
use crate::server::Access;
use crate::{Daemon, Error, Result};

/// How long a link opens the Office: the browser takes it at once.
const CODE_LIFETIME: Duration = Duration::from_secs(60);

/// Core's extension web state: `Daemon::extension_web`.
#[derive(Default)]
pub struct ExtensionWeb {
    /// Core's TCP port, once `server::serve` bound it.
    port: OnceLock<u16>,
    office: BrowserGateway,
}

impl ExtensionWeb {
    /// A link that opens the Office in a browser, once; starts the gateway the first time.
    /// `office.browser_link` answers it as is.
    pub async fn office_link(&self, daemon: &Arc<Daemon>) -> Result<String> {
        self.office.link(daemon).await
    }

    /// `office.open_in_browser`: the link, opened in the user's browser too. A browser that does
    /// not open is logged, not an error: the app still shows the link.
    pub async fn open_office_in_browser(&self, daemon: &Arc<Daemon>) -> Result<String> {
        let url = self.office_link(daemon).await?;
        if let Err(error) = crate::identity::open_browser(&url) {
            tracing::warn!(%error, "could not open the browser on the office");
        }
        Ok(url)
    }

    pub(crate) fn listening(&self, port: u16) {
        let _ = self.port.set(port);
    }

    pub(crate) fn port(&self) -> Option<u16> {
        self.port.get().copied()
    }
}

/// The revision a URL's nonce reads (`Extensions::asset`): extension, revision, nonce.
type ResolveAsset =
    Arc<dyn Fn(&Daemon, ExtensionId, &str, &str) -> Option<VerifiedRevision> + Send + Sync>;
/// The UI an extension loads now, its files on Core's port (`ExtensionPackageInfo::ui`).
type CurrentUi = Arc<dyn Fn(&Daemon, ExtensionId) -> Option<ExtensionUi> + Send + Sync>;

/// Where the installed packages come from: the managed runtimes, or a test's.
#[derive(Clone)]
struct Packages {
    asset: ResolveAsset,
    ui: CurrentUi,
}

impl Packages {
    fn installed() -> Self {
        Self {
            asset: Arc::new(|daemon, id, revision, nonce| {
                daemon.extensions.asset(id, revision, nonce)
            }),
            ui: Arc::new(|daemon, id| {
                crate::extensions::list(daemon)
                    .ok()?
                    .into_iter()
                    .find(|extension| extension.id == id)?
                    .package?
                    .ui
            }),
        }
    }
}

/// `GET /extension-assets/{id}/{revision}/{nonce}/{*path}` on Core's port: for the app (Tauri,
/// its Vite dev server) and Core's own origin, never another site nor a workspace's dev server.
pub(crate) async fn core_asset(
    daemon: &Daemon,
    port: u16,
    headers: &HeaderMap,
    path: (String, String, String, String),
) -> Response {
    asset_on_core(&Packages::installed(), daemon, port, headers, path).await
}

async fn asset_on_core(
    packages: &Packages,
    daemon: &Daemon,
    port: u16,
    headers: &HeaderMap,
    (id, revision, nonce, path): (String, String, String, String),
) -> Response {
    if !own_host(port, headers) {
        return (StatusCode::FORBIDDEN, "host not allowed").into_response();
    }
    let origin = header_str(headers, header::ORIGIN);
    if let Some(origin) = origin
        && !(crate::server::origin_allowed(origin) || own_origin(port, origin))
    {
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }
    let resolved =
        assets::extension(&id).and_then(|id| (packages.asset)(daemon, id, &revision, &nonce));
    assets::respond(resolved, &path, origin).await
}

#[derive(Default)]
struct BrowserGateway {
    port: tokio::sync::OnceCell<u16>,
    /// The links' codes not exchanged yet, with when they were given.
    codes: Mutex<HashMap<String, Instant>>,
    sessions: Mutex<HashSet<String>>,
}

impl BrowserGateway {
    async fn link(&self, daemon: &Arc<Daemon>) -> Result<String> {
        let dir = web_dir().ok_or_else(|| {
            Error::Unavailable(
                "The Office's web page is not installed beside chauffeurd (chauffeur-web): install Chauffeur again with scripts/install.sh --app."
                    .into(),
            )
        })?;
        let port = *self
            .port
            .get_or_try_init(|| listen(daemon.clone(), dir, Packages::installed()))
            .await?;
        Ok(format!("http://127.0.0.1:{port}/#office={}", self.code()))
    }

    fn code(&self) -> String {
        let code = random_hex();
        let mut codes = self.codes.lock().unwrap();
        codes.retain(|_, given| given.elapsed() < CODE_LIFETIME);
        codes.insert(code.clone(), Instant::now());
        code
    }

    /// A session for a code given less than a minute ago, which no other exchange took.
    fn exchange(&self, code: &str) -> Option<String> {
        let given = self.codes.lock().unwrap().remove(code)?;
        if given.elapsed() >= CODE_LIFETIME {
            return None;
        }
        let session = random_hex();
        self.sessions.lock().unwrap().insert(session.clone());
        Some(session)
    }

    fn session(&self, token: &str) -> bool {
        !token.is_empty() && self.sessions.lock().unwrap().contains(token)
    }
}

pub(crate) fn random_hex() -> String {
    hex::encode(rand::random::<[u8; 32]>())
}

/// The app's built UI: `chauffeur-web` beside this binary (scripts/install.sh), else, in a debug
/// build, the repository's `app/dist` (`bun run build`).
fn web_dir() -> Option<PathBuf> {
    let installed = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("chauffeur-web")));
    let built = cfg!(debug_assertions)
        .then(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/dist"));
    [installed, built]
        .into_iter()
        .flatten()
        .find(|dir| dir.join("index.html").is_file())
}

async fn listen(daemon: Arc<Daemon>, dir: PathBuf, packages: Packages) -> Result<u16> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|error| Error::Internal(anyhow::anyhow!("binding 127.0.0.1: {error}")))?;
    let port = listener
        .local_addr()
        .map_err(|error| Error::Internal(error.into()))?
        .port();
    tracing::info!(port, dir = %dir.display(), "the office served to the browser");
    let shutdown = daemon.shutdown.clone();
    let app = router(daemon, dir, port, packages);
    tokio::spawn(async move {
        let served = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await;
        if let Err(error) = served {
            tracing::warn!(%error, "the office's web listener stopped");
        }
    });
    Ok(port)
}

#[derive(Clone)]
struct WebState {
    daemon: Arc<Daemon>,
    dir: Arc<PathBuf>,
    port: u16,
    packages: Packages,
}

fn router(daemon: Arc<Daemon>, dir: PathBuf, port: u16, packages: Packages) -> Router {
    Router::new()
        .route("/session", post(session_handler))
        .route("/ws", get(ws_handler))
        .route("/extension-ui/{id}", get(bootstrap_handler))
        .route(
            &format!("{}/{{id}}/{{revision}}/{{nonce}}/{{*path}}", assets::ROUTE),
            get(asset_handler),
        )
        .fallback(get(file_handler))
        .with_state(WebState {
            daemon,
            dir: Arc::new(dir),
            port,
            packages,
        })
}

fn header_str(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// Whether the request names the listener on `port`: a page of another site that a DNS
/// rebinding points at 127.0.0.1 still names its own host.
fn own_host(port: u16, headers: &HeaderMap) -> bool {
    header_str(headers, header::HOST).is_some_and(|host| {
        host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
    })
}

fn own_origin(port: u16, origin: &str) -> bool {
    origin == format!("http://127.0.0.1:{port}") || origin == format!("http://localhost:{port}")
}

/// The checks of the exchange and of the WebSocket: the page's own host and origin, as its
/// browser says (every browser sends an Origin with these), and this user's; `None` when they pass.
fn refused(state: &WebState, headers: &HeaderMap, peer: SocketAddr) -> Option<Response> {
    let origin_ok = header_str(headers, header::ORIGIN).is_some_and(|o| own_origin(state.port, o));
    if !own_host(state.port, headers) || !origin_ok {
        return Some((StatusCode::FORBIDDEN, "origin not allowed").into_response());
    }
    if !same_user(peer, state.port) {
        return Some((StatusCode::FORBIDDEN, "not this user's").into_response());
    }
    None
}

#[derive(Deserialize)]
struct ExchangeParams {
    code: String,
}

async fn session_handler(
    State(state): State<WebState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    axum::Json(params): axum::Json<ExchangeParams>,
) -> Response {
    if let Some(refusal) = refused(&state, &headers, peer) {
        return refusal;
    }
    match state.daemon.extension_web.office.exchange(&params.code) {
        Some(token) => {
            // The app waits for it to leave its own Office: an opener may show no browser.
            state.daemon.events.publish(Event::Office {
                event: OfficeEvent::BrowserJoined,
            });
            (
                [(header::CACHE_CONTROL, "no-store")],
                axum::Json(json!({ "token": token })),
            )
                .into_response()
        }
        None => (
            StatusCode::UNAUTHORIZED,
            "This link was used already or is over a minute old: open the Office in your browser again from Chauffeur.",
        )
            .into_response(),
    }
}

async fn ws_handler(
    State(state): State<WebState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if let Some(refusal) = refused(&state, &headers, peer) {
        return refusal;
    }
    let token = query.get("token").map(String::as_str).unwrap_or_default();
    if !state.daemon.extension_web.office.session(token) {
        return (StatusCode::UNAUTHORIZED, "bad session").into_response();
    }
    let daemon = state.daemon.clone();
    crate::server::with_limits(upgrade)
        .on_upgrade(move |socket| crate::server::connection(daemon, socket, Access::Office))
}

/// `GET /extension-ui/office`, with the session as a Bearer: the Office's installed UI (an
/// `ExtensionUi`), its files under this listener, before the page renders it; 503 while it is
/// not installed or starting. Never another extension's.
async fn bootstrap_handler(
    State(state): State<WebState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    // A same-origin GET carries no Origin; one that does must be the page's.
    let origin = header_str(&headers, header::ORIGIN);
    if !own_host(state.port, &headers)
        || origin.is_some_and(|origin| !own_origin(state.port, origin))
    {
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }
    if !same_user(peer, state.port) {
        return (StatusCode::FORBIDDEN, "not this user's").into_response();
    }
    let session = header_str(&headers, header::AUTHORIZATION)
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    if !state.daemon.extension_web.office.session(session) {
        return (StatusCode::UNAUTHORIZED, "bad session").into_response();
    }
    if assets::extension(&id) != Some(ExtensionId::Office) {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    let host = header_str(&headers, header::HOST).expect("checked by own_host");
    let Some(ui) = (state.packages.ui)(&state.daemon, ExtensionId::Office)
        .and_then(|ui| under(ui, &format!("http://{host}")))
    else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "The Office is not installed or is starting: install it in Chauffeur's Settings → Extensions, or wait a moment.",
        )
            .into_response();
    };
    ([(header::CACHE_CONTROL, "no-store")], axum::Json(ui)).into_response()
}

/// `ui` with its files under `origin` rather than Core's port: the same route and nonce.
fn under(mut ui: ExtensionUi, origin: &str) -> Option<ExtensionUi> {
    let route = ui.base_url.find(&format!("{}/", assets::ROUTE))?;
    ui.base_url = format!("{origin}{}", &ui.base_url[route..]);
    Some(ui)
}

/// The Office's UI files under the page's origin, so its CSP's `'self'` covers them.
async fn asset_handler(
    State(state): State<WebState>,
    axum::extract::Path((id, revision, nonce, path)): axum::extract::Path<(
        String,
        String,
        String,
        String,
    )>,
    headers: HeaderMap,
) -> Response {
    let origin = header_str(&headers, header::ORIGIN);
    if !own_host(state.port, &headers)
        || origin.is_some_and(|origin| !own_origin(state.port, origin))
    {
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }
    if assets::extension(&id) != Some(ExtensionId::Office) {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    let resolved = (state.packages.asset)(&state.daemon, ExtensionId::Office, &revision, &nonce);
    assets::respond(resolved, &path, origin).await
}

/// No script but this listener's (the app's and the Office's installed UI), no frame around it,
/// nothing reached but this listener (and the `blob:` URLs three.js loads a model's textures
/// from), and the jukebox's https audio, as in the app.
fn page_csp(port: u16) -> String {
    format!(
        "default-src 'self'; connect-src 'self' blob: data: ws://127.0.0.1:{port} ws://localhost:{port}; \
         img-src 'self' data: blob:; media-src 'self' blob: https:; style-src 'self' 'unsafe-inline'; \
         font-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"
    )
}

/// A file of the built UI, `/` its page; what is not one of its files is not found.
async fn file_handler(State(state): State<WebState>, headers: HeaderMap, uri: Uri) -> Response {
    let not_found = || (StatusCode::NOT_FOUND, "not found").into_response();
    if !own_host(state.port, &headers) {
        return (StatusCode::FORBIDDEN, "host not allowed").into_response();
    }
    let asked = match uri.path().trim_start_matches('/') {
        "" => "index.html",
        path => path,
    };
    let Some((path, _)) = chauffeur_design::screen_file(&state.dir, asked) else {
        return not_found();
    };
    let media_type = match path.extension().and_then(|e| e.to_str()) {
        Some("glb") => "model/gltf-binary",
        _ => match chauffeur_design::content_type(&path) {
            Some(media_type) => media_type,
            None => return not_found(),
        },
    };
    let Ok(bytes) = tokio::fs::read(&path).await else {
        return not_found();
    };
    (
        [
            (header::CONTENT_TYPE, media_type.to_string()),
            (header::CONTENT_SECURITY_POLICY, page_csp(state.port)),
            (header::CACHE_CONTROL, "no-cache".into()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".into()),
            (header::REFERRER_POLICY, "no-referrer".into()),
        ],
        bytes,
    )
        .into_response()
}

/// Whether the client end of this loopback connection belongs to the daemon's user. Elsewhere
/// than on Linux it cannot be read without more: the code's single use is what holds there.
#[cfg(target_os = "linux")]
fn same_user(peer: SocketAddr, port: u16) -> bool {
    let Ok(table) = std::fs::read_to_string("/proc/net/tcp") else {
        return false;
    };
    // SAFETY: geteuid cannot fail and touches no memory.
    peer_uid(&table, peer, port) == Some(unsafe { libc::geteuid() })
}

#[cfg(not(target_os = "linux"))]
fn same_user(_peer: SocketAddr, _port: u16) -> bool {
    true
}

/// The owner of the socket at `peer` connected to 127.0.0.1:`port`, as `/proc/net/tcp` lists it
/// (addresses in the kernel's byte order, in hexadecimal; the uid the eighth column).
#[cfg(target_os = "linux")]
fn peer_uid(table: &str, peer: SocketAddr, port: u16) -> Option<u32> {
    let SocketAddr::V4(peer) = peer else {
        return None;
    };
    let hex = |ip: std::net::Ipv4Addr, port: u16| {
        format!("{:08X}:{port:04X}", u32::from_ne_bytes(ip.octets()))
    };
    let local = hex(*peer.ip(), peer.port());
    let remote = hex(std::net::Ipv4Addr::LOCALHOST, port);
    table.lines().skip(1).find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.get(1) != Some(&local.as_str()) || fields.get(2) != Some(&remote.as_str()) {
            return None;
        }
        fields.get(7)?.parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::assets::VerifiedRevision;
    use chauffeur_protocol::client::ClientCall;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    type Socket = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    fn call(method: &str, params: serde_json::Value) -> ClientCall {
        serde_json::from_value(json!({ "method": method, "params": params })).unwrap()
    }

    fn office_may(call: &ClientCall) -> bool {
        chauffeur_protocol::access::check(Access::Office, call).is_ok()
    }

    fn daemon(temp: &Path) -> Arc<Daemon> {
        let paths = crate::paths::Paths::new(
            temp.join("home"),
            temp.join("workspaces"),
            temp.join("repos"),
        );
        paths.ensure().unwrap();
        std::fs::write(
            &paths.settings,
            "linear_enabled = false\ngithub_org = \"\"\ngithub_sources = false\n",
        )
        .unwrap();
        Daemon::open(paths).unwrap()
    }

    /// Installed packages as a managed runtime resolves them: one nonce per extension, its
    /// current revision and the ones still published.
    #[derive(Default)]
    struct Fake {
        port: OnceLock<u16>,
        nonces: Mutex<HashMap<&'static str, String>>,
        revisions: Mutex<HashMap<(&'static str, String), VerifiedRevision>>,
        current: Mutex<HashMap<&'static str, String>>,
    }

    impl Fake {
        fn publish(&self, id: ExtensionId, revision: &str, files: VerifiedRevision) {
            let slug = crate::extensions::packages::slug(id);
            self.nonces
                .lock()
                .unwrap()
                .entry(slug)
                .or_insert_with(random_hex);
            self.revisions
                .lock()
                .unwrap()
                .insert((slug, revision.into()), files);
            self.current.lock().unwrap().insert(slug, revision.into());
        }

        fn withdraw(&self, id: ExtensionId, revision: &str) {
            let slug = crate::extensions::packages::slug(id);
            self.revisions
                .lock()
                .unwrap()
                .remove(&(slug, revision.to_string()));
        }

        fn ui(&self, id: ExtensionId) -> Option<ExtensionUi> {
            let slug = crate::extensions::packages::slug(id);
            let revision = self.current.lock().unwrap().get(slug)?.clone();
            let files = self
                .revisions
                .lock()
                .unwrap()
                .get(&(slug, revision.clone()))?
                .clone();
            let nonce = self.nonces.lock().unwrap().get(slug)?.clone();
            let port = self.port.get()?;
            Some(ExtensionUi {
                base_url: format!(
                    "http://127.0.0.1:{port}{}/{slug}/{revision}/{nonce}/",
                    assets::ROUTE
                ),
                revision,
                script: files.script,
                stylesheet: files.stylesheet,
                assets: files.assets,
            })
        }

        fn packages(self: &Arc<Self>) -> Packages {
            let (asset, ui) = (self.clone(), self.clone());
            Packages {
                asset: Arc::new(move |_, id, revision, nonce| {
                    let slug = crate::extensions::packages::slug(id);
                    if asset.nonces.lock().unwrap().get(slug)? != nonce {
                        return None;
                    }
                    asset
                        .revisions
                        .lock()
                        .unwrap()
                        .get(&(slug, revision.to_string()))
                        .cloned()
                }),
                ui: Arc::new(move |_, id| ui.ui(id)),
            }
        }
    }

    /// Core's port as `server::serve` routes it, and the Office's verified package installed.
    async fn core(temp: &Path) -> (Arc<Daemon>, Arc<Fake>, u16, VerifiedRevision) {
        let daemon = daemon(temp);
        let fake = Arc::new(Fake::default());
        let packages = fake.packages();
        let state = daemon.clone();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        daemon.extension_web.listening(port);
        let _ = fake.port.set(port);
        let app = Router::new().route(
            &format!("{}/{{id}}/{{revision}}/{{nonce}}/{{*path}}", assets::ROUTE),
            get(
                move |axum::extract::Path(path): axum::extract::Path<(
                    String,
                    String,
                    String,
                    String,
                )>,
                      headers: HeaderMap| {
                    let (daemon, packages) = (state.clone(), packages.clone());
                    async move { asset_on_core(&packages, &daemon, port, &headers, path).await }
                },
            ),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let package = assets::tests::package(temp);
        fake.publish(ExtensionId::Office, "rev-a", package.clone());
        (daemon, fake, port, package)
    }

    fn get_with(
        http: &reqwest::Client,
        url: &str,
        headers: &[(header::HeaderName, &str)],
    ) -> reqwest::RequestBuilder {
        headers
            .iter()
            .fold(http.get(url), |request, (name, value)| {
                request.header(name.clone(), *value)
            })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_package_file_needs_its_revisions_nonce_and_an_origin_of_the_app() {
        let temp = tempfile::tempdir().unwrap();
        let (daemon, fake, port, _) = core(temp.path()).await;
        let ui = fake.ui(ExtensionId::Office).unwrap();
        assert_eq!(ui.revision, "rev-a");
        assert_eq!(ui.script, "ui/office.js");
        let base = ui.base_url.clone();
        assert!(
            base.starts_with(&format!(
                "http://127.0.0.1:{port}/extension-assets/office/rev-a/"
            )),
            "{base}"
        );
        let token = crate::server::load_or_create_token(&daemon.paths.token).unwrap();
        assert!(
            !base.contains(&token),
            "the daemon's token is never in a UI's URL"
        );
        let http = reqwest::Client::new();

        let script = http
            .get(format!("{base}ui/office.js"))
            .send()
            .await
            .unwrap();
        assert_eq!(script.status(), 200);
        let headers = script.headers().clone();
        assert_eq!(
            headers[header::CONTENT_TYPE],
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            headers[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert!(headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
        assert_eq!(script.text().await.unwrap(), "console.log('office')");
        let model = http
            .get(format!("{base}ui/assets/models/villager.glb"))
            .send()
            .await
            .unwrap();
        assert_eq!(model.headers()[header::CONTENT_TYPE], "model/gltf-binary");

        for origin in [
            "tauri://localhost",
            "http://tauri.localhost",
            "http://localhost:1420",
        ] {
            let allowed = get_with(
                &http,
                &format!("{base}ui/office.css"),
                &[(header::ORIGIN, origin)],
            )
            .send()
            .await
            .unwrap();
            assert_eq!(allowed.status(), 200, "{origin}");
            assert_eq!(
                allowed.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
                origin
            );
        }
        // Any site, and a workspace's dev server running the code agents write, are refused.
        for origin in ["http://evil.example", "http://localhost:3000", "null"] {
            let refused = get_with(
                &http,
                &format!("{base}ui/office.js"),
                &[(header::ORIGIN, origin)],
            )
            .send()
            .await
            .unwrap();
            assert_eq!(refused.status(), 403, "{origin}");
        }
        // A DNS rebinding names its own host.
        let rebound = get_with(
            &http,
            &format!("{base}ui/office.js"),
            &[(header::HOST, &format!("evil.example:{port}"))],
        )
        .send()
        .await
        .unwrap();
        assert_eq!(rebound.status(), 403);

        let other = |revision: &str, nonce: &str, path: &str| {
            format!("http://127.0.0.1:{port}/extension-assets/{revision}/{nonce}/{path}")
        };
        let nonce = base
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap()
            .to_string();
        for (url, why) in [
            (
                other("office/rev-a", "0".repeat(64).as_str(), "ui/office.js"),
                "a guessed nonce",
            ),
            (
                other("office/rev-a", &token, "ui/office.js"),
                "the daemon's token",
            ),
            (
                other("office/rev-b", &nonce, "ui/office.js"),
                "another revision",
            ),
            (
                other("database/rev-a", &nonce, "ui/office.js"),
                "another extension",
            ),
            (
                other("nothing/rev-a", &nonce, "ui/office.js"),
                "no extension",
            ),
            (format!("{base}bin/office-engine"), "the engine"),
            (format!("{base}extension.json"), "the manifest"),
            (format!("{base}ui/assets/page.html"), "an HTML page"),
            (
                format!("{base}ui/assets/missing.png"),
                "a file the install did not record",
            ),
            (
                format!("{base}..%2F..%2F..%2Fpackage%2Fbin%2Foffice-engine"),
                "an encoded parent",
            ),
            (
                format!("{base}ui%2F..%2Fbin%2Foffice-engine"),
                "an encoded parent inside",
            ),
            (
                format!("{base}%2e%2e/%2e%2e/bin/office-engine"),
                "encoded dots",
            ),
            (format!("{base}ui/.%2Foffice.js"), "a dot part"),
            (format!("{base}%2Fetc%2Fpasswd"), "an absolute path"),
            (format!("{base}ui%5Coffice.js"), "a backslash"),
        ] {
            let refused = http.get(&url).send().await.unwrap();
            assert!(
                refused.status() == 404 || refused.status() == 400,
                "{why}: {url} answered {}",
                refused.status()
            );
        }

        fake.withdraw(ExtensionId::Office, "rev-a");
        let withdrawn = http
            .get(format!("{base}ui/office.js"))
            .send()
            .await
            .unwrap();
        assert_eq!(
            withdrawn.status(),
            404,
            "a withdrawn revision is gone at once"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_package_file_changed_or_replaced_by_a_link_after_install_is_not_served() {
        let temp = tempfile::tempdir().unwrap();
        let (_daemon, fake, _port, package) = core(temp.path()).await;
        let base = fake.ui(ExtensionId::Office).unwrap().base_url;
        let http = reqwest::Client::new();
        std::fs::write(package.root.join("ui/office.css"), ".tampered{}").unwrap();
        let changed = http
            .get(format!("{base}ui/office.css"))
            .send()
            .await
            .unwrap();
        assert_eq!(changed.status(), 503);
        // Even a link to the very same bytes: a link may point anywhere tomorrow.
        let outside = temp.path().join("outside.js");
        std::fs::write(&outside, "console.log('office')").unwrap();
        std::fs::remove_file(package.root.join("ui/office.js")).unwrap();
        std::os::unix::fs::symlink(&outside, package.root.join("ui/office.js")).unwrap();
        let linked = http
            .get(format!("{base}ui/office.js"))
            .send()
            .await
            .unwrap();
        assert_eq!(linked.status(), 503);
        // A linked folder on the way as well.
        let elsewhere = temp.path().join("elsewhere");
        std::fs::create_dir_all(elsewhere.join("models")).unwrap();
        std::fs::write(elsewhere.join("models/villager.glb"), "glTF-model").unwrap();
        std::fs::remove_dir_all(package.root.join("ui/assets")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, package.root.join("ui/assets")).unwrap();
        let through = http
            .get(format!("{base}ui/assets/models/villager.glb"))
            .send()
            .await
            .unwrap();
        assert_eq!(through.status(), 503);
    }

    #[test]
    fn a_links_code_opens_one_session_once_and_within_a_minute() {
        let web = BrowserGateway::default();
        let code = web.code();
        let session = web.exchange(&code).expect("a fresh code");
        assert!(web.session(&session));
        assert!(web.exchange(&code).is_none(), "a code is used once");
        assert!(web.exchange("guessed").is_none());
        assert!(!web.session(&code), "a code is no session");
        assert!(!web.session(""));
        let old = web.code();
        let past = Instant::now().checked_sub(CODE_LIFETIME).unwrap();
        web.codes.lock().unwrap().insert(old.clone(), past);
        assert!(web.exchange(&old).is_none(), "a code over a minute old");
    }

    #[test]
    fn a_browser_session_calls_only_what_the_office_shows() {
        for allowed in [
            call("office.state", json!({})),
            call(
                "office.signal",
                json!({ "to": "a@example.com", "signal": "{}" }),
            ),
            call("workspace.list", json!({ "include_archived": false })),
            call("design.shared", json!({})),
            call(
                "office.jukebox",
                json!({ "action": "play", "url": "https://example.com/a.mp3" }),
            ),
            call(
                "pr.shipped",
                json!({ "since": "2026-10-20T00:00:00+02:00" }),
            ),
            call(
                "events.subscribe",
                json!({ "all": false, "registry": true, "workspaces": [], "chats": [], "ephemeral": false, "office": true }),
            ),
        ] {
            assert!(office_may(&allowed), "{allowed:?}");
        }
        for refused in [
            call("office.open_in_browser", json!({})),
            call("office.browser_link", json!({})),
            call("chat.send", json!({ "chat": "c", "text": "rm -rf ~" })),
            call("workspace.archive", json!({ "workspace": "w" })),
            call("terminal.list", json!({ "workspace": "w" })),
            call(
                "events.subscribe",
                json!({ "all": true, "registry": false, "workspaces": [], "chats": [], "ephemeral": false, "office": false }),
            ),
            call(
                "events.subscribe",
                json!({ "all": false, "registry": false, "workspaces": [], "chats": ["c"], "ephemeral": false, "office": false }),
            ),
        ] {
            assert!(!office_may(&refused), "{refused:?}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_owner_of_a_loopback_connection_is_read_from_the_kernels_table() {
        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
            0: 0100007F:A1B2 0100007F:1F90 01 00000000:00000000 00:00000000 00000000  1001        0 1 1\n\
            1: 0100007F:1F90 0100007F:A1B2 01 00000000:00000000 00:00000000 00000000  1000        0 2 1\n";
        let peer: SocketAddr = "127.0.0.1:41394".parse().unwrap();
        assert_eq!(peer_uid(table, peer, 0x1F90), Some(1001));
        assert_eq!(peer_uid(table, peer, 0x1F91), None);
    }

    async fn ask(ws: &mut Socket, method: &str, params: serde_json::Value) -> serde_json::Value {
        let request = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        ws.send(Message::Text(request.to_string().into()))
            .await
            .unwrap();
        let answer = ws.next().await.unwrap().unwrap();
        serde_json::from_str(answer.to_text().unwrap()).unwrap()
    }

    /// The gateway of `listen`, on a built UI of one page, with the daemon of a temp dir and the
    /// Office's package published.
    async fn gateway(temp: &Path) -> (Arc<Daemon>, Arc<Fake>, u16) {
        let daemon = daemon(temp);
        let dir = temp.join("web");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("index.html"),
            "<!doctype html><title>Office</title>",
        )
        .unwrap();
        std::fs::write(temp.join("secret"), "the token").unwrap();
        let fake = Arc::new(Fake::default());
        // Core's port, which the gateway's bootstrap replaces with its own.
        let _ = fake.port.set(9);
        fake.publish(ExtensionId::Office, "rev-a", assets::tests::package(temp));
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = router(daemon.clone(), dir, port, fake.packages());
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        (daemon, fake, port)
    }

    /// A session of the page at `port`, as its browser gets it.
    async fn session(daemon: &Daemon, port: u16) -> String {
        let code = daemon.extension_web.office.code();
        let answer = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{port}/session"))
            .header(header::ORIGIN, format!("http://127.0.0.1:{port}"))
            .json(&json!({ "code": code }))
            .send()
            .await
            .unwrap();
        assert_eq!(answer.status(), 200);
        answer.json::<serde_json::Value>().await.unwrap()["token"]
            .as_str()
            .unwrap()
            .to_string()
    }

    async fn socket(
        port: u16,
        token: &str,
        origin: &str,
    ) -> std::result::Result<Socket, tokio_tungstenite::tungstenite::Error> {
        let mut request = format!("ws://127.0.0.1:{port}/ws?token={token}")
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert(header::ORIGIN, origin.parse().unwrap());
        tokio_tungstenite::connect_async(request)
            .await
            .map(|(ws, _)| ws)
    }

    async fn bootstrap(port: u16, session: &str, id: &str) -> reqwest::Response {
        reqwest::Client::new()
            .get(format!("http://127.0.0.1:{port}/extension-ui/{id}"))
            .bearer_auth(session)
            .send()
            .await
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_page_is_served_to_its_own_host_only() {
        let temp = tempfile::tempdir().unwrap();
        let (daemon, _fake, port) = gateway(temp.path()).await;
        let http = reqwest::Client::new();
        let page = http
            .get(format!("http://127.0.0.1:{port}/"))
            .send()
            .await
            .unwrap();
        assert_eq!(page.status(), 200);
        let csp = page.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap();
        assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
        assert!(csp.contains("connect-src 'self' blob:"), "{csp}");
        let rebound = http
            .get(format!("http://127.0.0.1:{port}/"))
            .header(header::HOST, format!("evil.example:{port}"))
            .send()
            .await
            .unwrap();
        assert_eq!(rebound.status(), 403);
        let outside = http
            .get(format!("http://127.0.0.1:{port}/..%2Fsecret"))
            .send()
            .await
            .unwrap();
        assert_eq!(outside.status(), 404);
        daemon.shutdown.cancel();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_session_from_the_pages_origin_reaches_the_office_and_nothing_else() {
        let temp = tempfile::tempdir().unwrap();
        let (daemon, _fake, port) = gateway(temp.path()).await;
        let origin = format!("http://127.0.0.1:{port}");
        let http = reqwest::Client::new();
        let exchange = |code: String, origin: String| {
            http.post(format!("http://127.0.0.1:{port}/session"))
                .header(header::ORIGIN, origin)
                .json(&json!({ "code": code }))
                .send()
        };
        let mut events = daemon.events.subscribe();
        let code = daemon.extension_web.office.code();
        for (origin, host) in [
            (
                "http://evil.example".to_string(),
                format!("127.0.0.1:{port}"),
            ),
            (origin.clone(), format!("evil.example:{port}")),
        ] {
            let elsewhere = http
                .post(format!("http://127.0.0.1:{port}/session"))
                .header(header::ORIGIN, origin)
                .header(header::HOST, host)
                .json(&json!({ "code": code }))
                .send()
                .await
                .unwrap();
            assert_eq!(elsewhere.status(), 403);
        }
        let answer = exchange(code.clone(), origin.clone()).await.unwrap();
        assert_eq!(answer.status(), 200);
        tokio::time::timeout(Duration::from_secs(5), async {
            while !matches!(
                events.recv().await.unwrap().event,
                Event::Office {
                    event: OfficeEvent::BrowserJoined
                }
            ) {}
        })
        .await
        .expect("browser_joined published");
        let token = answer.json::<serde_json::Value>().await.unwrap()["token"]
            .as_str()
            .unwrap()
            .to_string();
        let again = exchange(code, origin.clone()).await.unwrap();
        assert_eq!(again.status(), 401);

        assert!(socket(port, &token, "http://evil.example").await.is_err());
        let daemon_token = crate::server::load_or_create_token(&daemon.paths.token).unwrap();
        assert!(socket(port, &daemon_token, &origin).await.is_err());
        let mut ws = socket(port, &token, &origin).await.unwrap();
        let state = ask(&mut ws, "office.state", json!({})).await;
        assert!(state.get("result").is_some(), "{state}");
        let listed = ask(
            &mut ws,
            "workspace.list",
            json!({ "include_archived": false }),
        )
        .await;
        assert!(listed.get("result").is_some(), "{listed}");
        for (method, params) in [
            ("chat.send", json!({ "chat": "c", "text": "hi" })),
            ("office.browser_link", json!({})),
            (
                "extensions.set",
                json!({ "id": "office", "enabled": false }),
            ),
            (
                "events.subscribe",
                json!({ "all": true, "registry": false, "workspaces": [], "chats": [], "ephemeral": false, "office": false }),
            ),
        ] {
            let refused = ask(&mut ws, method, params).await;
            assert!(
                refused["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains(&format!("may not call {method}")),
                "{refused}"
            );
        }
        daemon.shutdown.cancel();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_office_ui_bootstraps_under_the_gateway_for_a_session_only() {
        let temp = tempfile::tempdir().unwrap();
        let (daemon, fake, port) = gateway(temp.path()).await;
        let session = session(&daemon, port).await;
        assert_eq!(bootstrap(port, "", "office").await.status(), 401);
        let daemon_token = crate::server::load_or_create_token(&daemon.paths.token).unwrap();
        assert_eq!(bootstrap(port, &daemon_token, "office").await.status(), 401);
        assert_eq!(bootstrap(port, &session, "database").await.status(), 404);
        let foreign = reqwest::Client::new()
            .get(format!("http://127.0.0.1:{port}/extension-ui/office"))
            .bearer_auth(&session)
            .header(header::ORIGIN, "http://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(foreign.status(), 403);

        let answer = bootstrap(port, &session, "office").await;
        assert_eq!(answer.status(), 200);
        assert_eq!(answer.headers()[header::CACHE_CONTROL], "no-store");
        let ui = answer.json::<ExtensionUi>().await.unwrap();
        let base = ui.base_url.clone();
        assert!(
            base.starts_with(&format!(
                "http://127.0.0.1:{port}/extension-assets/office/rev-a/"
            )),
            "{base}"
        );
        assert_eq!(ui.script, "ui/office.js");
        let http = reqwest::Client::new();
        let script = http
            .get(format!("{base}ui/office.js"))
            .send()
            .await
            .unwrap();
        assert_eq!(script.status(), 200);
        let engine = http
            .get(format!("{base}bin/office-engine"))
            .send()
            .await
            .unwrap();
        assert_eq!(engine.status(), 404);
        let other_origin = http
            .get(format!("{base}ui/office.js"))
            .header(header::ORIGIN, "tauri://localhost")
            .send()
            .await
            .unwrap();
        assert_eq!(
            other_origin.status(),
            403,
            "the gateway serves its own page only"
        );

        fake.withdraw(ExtensionId::Office, "rev-a");
        assert_eq!(bootstrap(port, &session, "office").await.status(), 503);

        // Another extension's UI is never served through the Office's gateway.
        let database = assets::tests::package(&temp.path().join("database"));
        fake.publish(ExtensionId::Database, "rev-d", database);
        let database_base = under(
            fake.ui(ExtensionId::Database).unwrap(),
            &format!("http://127.0.0.1:{port}"),
        )
        .unwrap()
        .base_url;
        let refused = http
            .get(format!("{database_base}ui/office.js"))
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), 404);
        daemon.shutdown.cancel();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_browser_session_outlives_an_update_of_the_office() {
        let temp = tempfile::tempdir().unwrap();
        let (daemon, fake, port) = gateway(temp.path()).await;
        let origin = format!("http://127.0.0.1:{port}");
        let session = session(&daemon, port).await;
        let before = bootstrap(port, &session, "office")
            .await
            .json::<serde_json::Value>()
            .await
            .unwrap();
        let old = before["base_url"].as_str().unwrap().to_string();

        // The managed runtime installs revision b, then drops a once no view shows it.
        let next = assets::tests::package(&temp.path().join("next"));
        fake.publish(ExtensionId::Office, "rev-b", next);
        let http = reqwest::Client::new();
        let still = http.get(format!("{old}ui/office.js")).send().await.unwrap();
        assert_eq!(
            still.status(),
            200,
            "a view still showing a keeps its files"
        );
        fake.withdraw(ExtensionId::Office, "rev-a");

        let after = bootstrap(port, &session, "office")
            .await
            .json::<serde_json::Value>()
            .await
            .unwrap();
        assert_eq!(after["revision"], "rev-b");
        let new = after["base_url"].as_str().unwrap().to_string();
        assert_ne!(new, old);
        let gone = http.get(format!("{old}ui/office.js")).send().await.unwrap();
        assert_eq!(gone.status(), 404);
        let served = http.get(format!("{new}ui/office.js")).send().await.unwrap();
        assert_eq!(served.status(), 200);
        let mut ws = socket(port, &session, &origin).await.unwrap();
        let state = ask(&mut ws, "office.state", json!({})).await;
        assert!(state.get("result").is_some(), "{state}");
        daemon.shutdown.cancel();
    }
}
