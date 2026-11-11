//! The daemon's endpoints: a WebSocket on 127.0.0.1 (token + Origin checked) for the app and
//! future remote clients, and the same protocol on a Unix socket (owner-only) for the CLI.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response as HttpResponse};
use axum::routing::get;
use chauffeur_protocol::client::{
    ClientCall, DaemonNotification, EventNotification, Replay, SubscribeParams, SubscribeResult,
};
use chauffeur_protocol::events::{Event, EventEnvelope, WorkspaceEvent};
use chauffeur_protocol::methods::Method;
use chauffeur_protocol::peer::{PeerCall, PeerMethod};
use chauffeur_protocol::rpc::{Incoming, Notification, Response, RpcError, codes};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::Instrument;

use crate::store::EventFilter;
use crate::{Daemon, attachments};
pub use chauffeur_protocol::access::Access;

#[derive(Clone)]
struct AppState {
    daemon: Arc<Daemon>,
    token: Arc<String>,
    require_token: bool,
}

/// Reads or creates the token clients present on the TCP endpoint.
pub fn load_or_create_token(path: &std::path::Path) -> anyhow::Result<String> {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        use std::os::unix::fs::PermissionsExt;
        // A link planted here would hand the token over, or read one from elsewhere.
        anyhow::ensure!(
            meta.file_type().is_file(),
            "{} is not a regular file: remove it",
            path.display()
        );
        // Restored from a backup, `chmod -R`… the mode is ours to keep, not the file's.
        if meta.permissions().mode() & 0o077 != 0 {
            tracing::warn!(path = %path.display(), "token file was readable by others: fixed");
            crate::paths::private_file(path)?;
        }
    }
    if let Ok(token) = std::fs::read_to_string(path) {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return Ok(token);
        }
    }
    let bytes: [u8; 32] = rand::random();
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    write_private(path, &token)?;
    Ok(token)
}

fn write_private(path: &std::path::Path, contents: &str) -> anyhow::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("writing {}", path.display()))?;
    std::io::Write::write_all(&mut file, contents.as_bytes())?;
    Ok(())
}

/// Serves until the daemon's shutdown token is cancelled.
pub async fn serve(daemon: Arc<Daemon>) -> anyhow::Result<()> {
    let token = Arc::new(load_or_create_token(&daemon.paths.token)?);
    let port = std::env::var("CHAUFFEUR_PORT_OVERRIDE")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(daemon.settings.get().daemon_port);
    let tcp = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(tcp) => tcp,
        // Another program holds the port (or squats it): clients read the port from
        // `daemon.json`, so any free one does; the user's settings are not touched.
        Err(error) if port != 0 => {
            tracing::warn!(port, %error, "port taken: listening on a free one instead");
            tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .context("binding 127.0.0.1 on a free port")?
        }
        Err(error) => return Err(error).with_context(|| format!("binding 127.0.0.1:{port}")),
    };
    let port = tcp.local_addr()?.port();
    daemon.extension_web.listening(port);
    // Written before the socket answers: a client that finds the socket alive reads a
    // `daemon.json` that is this daemon's, never a stale one naming a port someone else holds.
    write_private(
        &daemon.paths.daemon_info,
        &serde_json::to_string_pretty(&json!({
            "pid": std::process::id(),
            "port": port,
            "socket": daemon.paths.socket,
            "version": env!("CARGO_PKG_VERSION"),
            "build": crate::BUILD,
        }))?,
    )?;
    let _ = std::fs::remove_file(&daemon.paths.socket);
    // The state folder is 0700 (`Paths::ensure`), so the socket is unreachable by others even
    // before its own mode is set.
    let unix = tokio::net::UnixListener::bind(&daemon.paths.socket)
        .with_context(|| format!("binding {}", daemon.paths.socket.display()))?;
    crate::paths::private_file(&daemon.paths.socket)?;
    tracing::info!(port, socket = %daemon.paths.socket.display(), "chauffeurd listening");

    let router = |require_token: bool| {
        let state = AppState {
            daemon: daemon.clone(),
            token: token.clone(),
            require_token,
        };
        let router = match require_token {
            true => Router::new().route("/ws", get(ws_handler)),
            false => Router::new()
                .route("/ws", get(ws_unix_handler))
                .route("/peer", get(peer_handler)),
        };
        router
            .route("/attachments/{workspace}/{file}", get(attachment_handler))
            .route("/images/{workspace}", get(image_handler))
            .route("/media/{workspace}", get(media_handler))
            .route("/deliverables/{workspace}", get(deliverable_handler))
            .route("/tunnel/{workspace}/{offset}", get(tunnel_handler))
            .route("/preview/{workspace}/{offset}", get(preview_handler))
            .route("/workspace-file/{workspace}", get(workspace_file_handler))
            .route("/sound", get(sound_handler))
            .route(
                "/extension-assets/{id}/{revision}/{nonce}/{*path}",
                get(extension_asset_handler),
            )
            .route("/health", get(|| async { "ok" }))
            .with_state(state)
    };
    let shutdown = daemon.shutdown.clone();
    let tcp_server =
        axum::serve(tcp, router(true)).with_graceful_shutdown(shutdown.clone().cancelled_owned());
    let unix_server = axum::serve(
        unix,
        router(false).into_make_service_with_connect_info::<UnixPeer>(),
    )
    .with_graceful_shutdown(shutdown.clone().cancelled_owned());
    // The design chats' screens: an origin apart from this one, so that their HTML never runs
    // where the token and the WebSocket are.
    let designs = crate::canvas::DesignServer::serve(daemon.clone());
    // The design and workspace shares alive before the restart connect again (stories 51, 52).
    daemon.shares.resume();
    daemon.workspace_shares.resume();
    daemon.mentions.restart();
    tokio::select! {
        result = tcp_server => result?,
        result = unix_server => result?,
        result = designs => result?,
        _ = daemon.mobile.serve(daemon.clone()) => {},
    }
    let _ = std::fs::remove_file(&daemon.paths.socket);
    let _ = std::fs::remove_file(&daemon.paths.daemon_info);
    Ok(())
}

/// Whether the request carries the token (header or `?token=`), where one is required.
fn authorized(state: &AppState, headers: &HeaderMap, query: &HashMap<String, String>) -> bool {
    if !state.require_token {
        return true;
    }
    let presented = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
        .or_else(|| query.get("token").cloned());
    presented.as_deref() == Some(state.token.as_str())
}

/// An image attached to a message of a workspace, for `<img>` tags (hence the token in the URL).
/// Only images are served, as their bytes say: another attached file (an HTML page) must never be
/// rendered by a browser on the daemon's origin.
async fn attachment_handler(
    State(state): State<AppState>,
    Path((workspace, file)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> HttpResponse {
    if !authorized(&state, &headers, &query) {
        return (StatusCode::UNAUTHORIZED, "bad token").into_response();
    }
    if !attachments::valid_file_name(&file) {
        return (StatusCode::BAD_REQUEST, "bad file name").into_response();
    }
    let Ok(row) = state.daemon.workspaces.row(&workspace) else {
        return (StatusCode::NOT_FOUND, "no such workspace").into_response();
    };
    if let Some(machine) = crate::relay::machine_of(&state.daemon, &row.id) {
        let route = format!("/attachments/{}/{file}", row.id);
        return relayed(&state, &machine, &route, &[], &headers).await;
    }
    let Ok(bytes) = tokio::fs::read(attachments::dir(&row.path).join(&file)).await else {
        return (StatusCode::NOT_FOUND, "no such attachment").into_response();
    };
    match attachments::sniff(&bytes) {
        Some(media_type) => (
            [
                (header::CONTENT_TYPE, media_type),
                // Names are unique: an attachment never changes.
                (
                    header::CACHE_CONTROL,
                    "private, max-age=31536000, immutable",
                ),
            ],
            bytes,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "not an image").into_response(),
    }
}

/// The user's notification sound file (`sound` in `[notifications]`), for the app to play. Only
/// audio is served, as its bytes say, so the setting cannot be used to read another file.
async fn sound_handler(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> HttpResponse {
    if !authorized(&state, &headers, &query) {
        return (StatusCode::UNAUTHORIZED, "bad token").into_response();
    }
    let settings = state.daemon.settings.get();
    let crate::sound::Sound::File(path) = crate::sound::kind(&settings.notifications.sound) else {
        return (StatusCode::NOT_FOUND, "no sound file").into_response();
    };
    let path = path.to_string();
    // The app fetches it to decode it (an <img> needs no permission, a fetch does): only the app's
    // own origins may read it.
    let origin = headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .filter(|origin| origin_allowed(origin))
        .map(str::to_string);
    let mut response = match tokio::task::spawn_blocking(move || crate::sound::read(&path)).await {
        Ok(Ok((bytes, media_type))) => (
            [
                (header::CONTENT_TYPE, media_type),
                // The user may replace the file under the same name.
                (header::CACHE_CONTROL, "no-cache"),
            ],
            bytes,
        )
            .into_response(),
        _ => (StatusCode::NOT_FOUND, "not a sound file").into_response(),
    };
    if let Some(origin) = origin.and_then(|o| o.parse().ok()) {
        response
            .headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        response
            .headers_mut()
            .insert(header::VARY, header::HeaderValue::from_static("Origin"));
    }
    response
}

/// An image file an agent's answer shows (`![](shot.png)`): `?path=` absolute, or under the
/// workspace folder. Only images are served, as their bytes say, so no other file can be read.
async fn image_handler(
    State(state): State<AppState>,
    Path(workspace): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> HttpResponse {
    if !authorized(&state, &headers, &query) {
        return (StatusCode::UNAUTHORIZED, "bad token").into_response();
    }
    let Some(asked) = query.get("path") else {
        return (StatusCode::BAD_REQUEST, "no path").into_response();
    };
    let Ok(row) = state.daemon.workspaces.row(&workspace) else {
        return (StatusCode::NOT_FOUND, "no such workspace").into_response();
    };
    if let Some(machine) = crate::relay::machine_of(&state.daemon, &row.id) {
        let route = format!("/images/{}", row.id);
        return relayed(&state, &machine, &route, &[("path", asked)], &headers).await;
    }
    let asked = asked.strip_prefix("file://").unwrap_or(asked);
    let path = std::path::Path::new(&row.path).join(asked);
    let readable = tokio::fs::metadata(&path)
        .await
        .is_ok_and(|m| m.is_file() && m.len() <= attachments::MAX_BYTES as u64);
    let bytes = match readable {
        true => tokio::fs::read(&path).await.ok(),
        false => None,
    };
    match bytes.as_deref().and_then(attachments::sniff) {
        Some(media_type) => (
            [
                (header::CONTENT_TYPE, media_type),
                // The agent may write the file again under the same name.
                (header::CACHE_CONTROL, "no-cache"),
            ],
            bytes.unwrap_or_default(),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "no such image").into_response(),
    }
}

/// A video, an audio file or a PDF of the workspace (`?path=` as `files.read` names it), for the
/// Files tab's player and PDF viewer: only under the workspace's roots, only what its name and its bytes say is media,
/// in the byte ranges a player asks for (WebKit plays no video without them).
async fn media_handler(
    State(state): State<AppState>,
    Path(workspace): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> HttpResponse {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    if !authorized(&state, &headers, &query) {
        return (StatusCode::UNAUTHORIZED, "bad token").into_response();
    }
    let Some(asked) = query.get("path") else {
        return (StatusCode::BAD_REQUEST, "no path").into_response();
    };
    let not_found = || (StatusCode::NOT_FOUND, "no such media file").into_response();
    if let Ok(row) = state.daemon.workspaces.row(&workspace)
        && let Some(machine) = crate::relay::machine_of(&state.daemon, &row.id)
    {
        let route = format!("/media/{}", row.id);
        return relayed(&state, &machine, &route, &[("path", asked)], &headers).await;
    }
    let Ok((_, roots)) = crate::rpc::file_roots(&state.daemon, &workspace) else {
        return not_found();
    };
    let Ok(path) = crate::files::resolve(&roots, asked) else {
        return not_found();
    };
    let Some(media_type) = crate::files::media_type(&path, asked) else {
        return not_found();
    };
    let Ok(mut file) = tokio::fs::File::open(&path).await else {
        return not_found();
    };
    let Ok(size) = file.metadata().await.map(|m| m.len()) else {
        return not_found();
    };
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|v| byte_range(v, size));
    let (status, start, end) = match range {
        None => (StatusCode::OK, 0, size),
        Some(Some((start, end))) => (StatusCode::PARTIAL_CONTENT, start, end),
        Some(None) => {
            return (
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{size}"))],
            )
                .into_response();
        }
    };
    if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return not_found();
    }
    let body =
        axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file.take(end - start)));
    let mut response = (
        status,
        [
            (header::CONTENT_TYPE, media_type.to_string()),
            (header::CONTENT_LENGTH, (end - start).to_string()),
            (header::ACCEPT_RANGES, "bytes".into()),
            // The agent may write the file again under the same name.
            (header::CACHE_CONTROL, "no-cache".into()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".into()),
            (
                header::CONTENT_SECURITY_POLICY,
                media_csp(media_type).into(),
            ),
        ],
        body,
    )
        .into_response();
    if status == StatusCode::PARTIAL_CONTENT
        && let Ok(value) = format!("bytes {start}-{}/{size}", end - 1).parse()
    {
        response.headers_mut().insert(header::CONTENT_RANGE, value);
    }
    response
}

/// A moved workspace's file, as the machine it runs on serves it: its status, its headers saying
/// what it is and how to show it, and its bytes as they come.
async fn relayed(
    state: &AppState,
    machine: &crate::relay::Machine,
    route: &str,
    query: &[(&str, &str)],
    headers: &HeaderMap,
) -> HttpResponse {
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    let answer = match state
        .daemon
        .relay
        .http_get(machine, route, query, range)
        .await
    {
        Ok(answer) => answer,
        Err(error) => {
            let message = format!("{} does not answer ({error})", machine.name);
            return (StatusCode::BAD_GATEWAY, message).into_response();
        }
    };
    let status = StatusCode::from_u16(answer.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let kept = relay_response_headers(answer.headers());
    // An error ends the body: the next read is not tried.
    let chunks = futures_util::stream::unfold(Some(answer), |answer| async move {
        let mut answer = answer?;
        match answer.chunk().await {
            Ok(Some(chunk)) => Some((Ok::<_, reqwest::Error>(chunk), Some(answer))),
            Ok(None) => None,
            Err(error) => Some((Err(error), None)),
        }
    });
    let body = axum::body::Body::from_stream(chunks);
    (status, kept, body).into_response()
}

/// Preserve download and media semantics without forwarding cookies or upstream capabilities.
fn relay_response_headers(headers: &reqwest::header::HeaderMap) -> HeaderMap {
    let mut kept = HeaderMap::new();
    for name in [
        header::CONTENT_TYPE,
        header::CONTENT_LENGTH,
        header::CONTENT_RANGE,
        header::CONTENT_DISPOSITION,
        header::ACCEPT_RANGES,
        header::CACHE_CONTROL,
        header::X_CONTENT_TYPE_OPTIONS,
        header::CONTENT_SECURITY_POLICY,
    ] {
        if let Some(value) = headers
            .get(name.as_str())
            .and_then(|v| header::HeaderValue::from_bytes(v.as_bytes()).ok())
        {
            kept.insert(name, value);
        }
    }
    kept
}

/// No script ever runs in what `/media` serves. A PDF is shown by WebKit's PDF viewer in a frame,
/// which a `sandbox` would block: it gets no script instead.
fn media_csp(media_type: &str) -> &'static str {
    if media_type == "application/pdf" {
        "default-src 'none'"
    } else {
        "sandbox; default-src 'none'"
    }
}

/// The bytes `start..end` a `Range: bytes=…` header asks of a file of `size` bytes (its first
/// range only), or None when it cannot be satisfied.
fn byte_range(header: &str, size: u64) -> Option<(u64, u64)> {
    let spec = header.trim().strip_prefix("bytes=")?;
    let spec = spec.split(',').next()?.trim();
    let (first, last) = spec.split_once('-')?;
    let (start, end) = match (first.trim(), last.trim()) {
        ("", suffix) => {
            let length: u64 = suffix.parse().ok()?;
            (size.saturating_sub(length), size)
        }
        (first, "") => (first.parse().ok()?, size),
        (first, last) => {
            let last: u64 = last.parse().ok()?;
            (first.parse().ok()?, last.saturating_add(1).min(size))
        }
    };
    (start < end).then_some((start, end))
}

/// Downloads declared HTML/PDF documents as attachments, never executable pages on the RPC origin.
async fn deliverable_handler(
    State(state): State<AppState>,
    Path(workspace): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> HttpResponse {
    use tokio::io::AsyncReadExt;

    let path = if state.require_token {
        query
            .get("ticket")
            .and_then(|ticket| state.daemon.downloads.take(ticket, &workspace))
    } else {
        // Only a peer on the owner's private Unix socket can request a path without a ticket.
        query.get("path").cloned()
    };
    let Some(asked) = path.as_ref().filter(|path| deliverable_path(path)) else {
        return (
            StatusCode::UNAUTHORIZED,
            "download link expired: download the document again from its chat",
        )
            .into_response();
    };
    if let Ok(row) = state.daemon.workspaces.row(&workspace)
        && let Some(machine) = crate::relay::machine_of(&state.daemon, &row.id)
    {
        let route = format!("/deliverables/{}", row.id);
        return relayed(&state, &machine, &route, &[("path", asked)], &headers).await;
    }
    let not_found = || {
        (
            StatusCode::NOT_FOUND,
            "document not found: regenerate it in its chat",
        )
            .into_response()
    };
    let Ok((_, roots)) = crate::rpc::file_roots(&state.daemon, &workspace) else {
        return not_found();
    };
    let Ok(path) = crate::files::resolve(&roots, asked) else {
        return not_found();
    };
    let Ok(file) = tokio::fs::File::open(&path).await else {
        return not_found();
    };
    const MAX_BYTES: u64 = 32 * 1024 * 1024;
    let mut bytes = Vec::new();
    if file
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .is_err()
    {
        return not_found();
    }
    if bytes.len() as u64 > MAX_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            "document exceeds 32 MB: ask for a smaller export",
        )
            .into_response();
    }
    let pdf = asked.to_ascii_lowercase().ends_with(".pdf");
    if pdf && !crate::files::is_media("application/pdf", &bytes) {
        return not_found();
    }
    let filename: String = asked
        .rsplit('/')
        .next()
        .unwrap_or("document")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    (
        [
            (
                header::CONTENT_TYPE,
                if pdf {
                    "application/pdf".to_string()
                } else {
                    "text/html; charset=utf-8".to_string()
                },
            ),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
            (header::CACHE_CONTROL, "no-store".to_string()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            (
                header::CONTENT_SECURITY_POLICY,
                "sandbox; default-src 'none'".to_string(),
            ),
        ],
        bytes,
    )
        .into_response()
}

pub(crate) fn deliverable_path(path: &str) -> bool {
    path.len() <= 1024
        && path.starts_with(".chauffeur/artifacts/")
        && !path
            .chars()
            .any(|c| c.is_control() || matches!(c, '\\' | '%' | '?' | '#'))
        && path.split('/').all(|p| !matches!(p, "" | "." | ".."))
        && [".html", ".htm", ".pdf"]
            .iter()
            .any(|ext| path.to_ascii_lowercase().ends_with(ext))
}

/// A workspace's file as the Files tab's preview serves it, for the daemon of the machine it moved
/// from, which serves it to its app's preview under its own policy (`canvas::file_handler`). On
/// the private Unix socket only.
async fn workspace_file_handler(
    State(state): State<AppState>,
    Path(workspace): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> HttpResponse {
    if state.require_token {
        return (StatusCode::NOT_FOUND, "not here").into_response();
    }
    let Some(path) = query.get("path") else {
        return (StatusCode::BAD_REQUEST, "no path").into_response();
    };
    match crate::canvas::workspace_file(&state.daemon, &workspace, path).await {
        Some((media_type, bytes)) => ([(header::CONTENT_TYPE, media_type)], bytes).into_response(),
        None => (StatusCode::NOT_FOUND, "no such file").into_response(),
    }
}

/// A TCP connection to one port of a workspace's block on this machine, for the daemon of the
/// machine it moved from (`relay_ports`): its bytes as binary WebSocket messages. On the private
/// Unix socket only, where whoever connects could reach those ports already.
async fn tunnel_handler(
    State(state): State<AppState>,
    Path((workspace, offset)): Path<(String, u16)>,
    upgrade: WebSocketUpgrade,
) -> HttpResponse {
    if state.require_token {
        return (StatusCode::NOT_FOUND, "not here").into_response();
    }
    let block = state
        .daemon
        .workspaces
        .row(&workspace)
        .ok()
        .and_then(|row| state.daemon.store.port_block(&row.id).ok().flatten());
    let Some(block) = block.filter(|b| offset < b.size) else {
        return (StatusCode::NOT_FOUND, "no such port of the workspace").into_response();
    };
    let port = block.base + offset;
    let stream = match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
        Ok(stream) => stream,
        // Nothing listens there: the client's connection is refused as a local one would be.
        Err(error) => return (StatusCode::BAD_GATEWAY, error.to_string()).into_response(),
    };
    with_limits(upgrade).on_upgrade(move |socket| async move {
        crate::relay_ports::pipe_socket(socket, stream).await;
    })
}

/// One connection to a declared preview of a workspace that runs on another machine, for the
/// mobile host (docs/workspace-relay.md): its bytes as binary WebSocket messages, passed to that
/// machine's `/tunnel`. On the private Unix socket only; refused for a workspace that runs here,
/// a port not declared as a preview there, or one nothing listens on.
async fn preview_handler(
    State(state): State<AppState>,
    Path((workspace, offset)): Path<(String, u16)>,
    upgrade: WebSocketUpgrade,
) -> HttpResponse {
    if state.require_token {
        return (StatusCode::NOT_FOUND, "not here").into_response();
    }
    let daemon = state.daemon.clone();
    let Ok(row) = daemon.workspaces.row(&workspace) else {
        return (StatusCode::NOT_FOUND, "no such workspace").into_response();
    };
    let Some(machine) = crate::relay::machine_of(&daemon, &row.id) else {
        return (
            StatusCode::NOT_FOUND,
            "the workspace runs on this machine: its previews open once it runs on another one",
        )
            .into_response();
    };
    let listed = daemon
        .relay
        .call(
            &machine,
            ClientCall::PreviewsList(chauffeur_protocol::client::WorkspaceParams {
                workspace: row.id.clone(),
            }),
        )
        .await
        .and_then(|value| {
            serde_json::from_value::<chauffeur_protocol::dev_services::Previews>(value)
                .map_err(|e| crate::Error::Internal(e.into()))
        });
    match listed {
        Ok(listed) if listed.previews.iter().any(|p| p.offset == offset) => {}
        Ok(_) => {
            return (StatusCode::NOT_FOUND, "no such preview declared there").into_response();
        }
        Err(error) => return (StatusCode::BAD_GATEWAY, error.to_string()).into_response(),
    }
    let tunnel = match crate::relay_ports::open(&machine, &row.id, offset).await {
        Ok(tunnel) => tunnel,
        Err(error) => return (StatusCode::BAD_GATEWAY, error.to_string()).into_response(),
    };
    with_limits(upgrade).on_upgrade(move |socket| {
        crate::relay_ports::preview(daemon, socket, tunnel, machine, row.id)
    })
}

/// An installed extension's UI file, on the TCP port only: its URL's nonce, not the daemon's
/// token, is what reads it (`extensions::assets`).
async fn extension_asset_handler(
    State(state): State<AppState>,
    Path(path): Path<(String, String, String, String)>,
    headers: HeaderMap,
) -> HttpResponse {
    let port = state
        .require_token
        .then(|| state.daemon.extension_web.port())
        .flatten();
    let Some(port) = port else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    crate::extensions::web::core_asset(&state.daemon, port, &headers, path).await
}

async fn ws_handler(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> HttpResponse {
    if state.require_token {
        if !authorized(&state, &headers, &query) {
            return (StatusCode::UNAUTHORIZED, "bad token").into_response();
        }
        if let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok())
            && !origin_allowed(origin)
        {
            return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
        }
    }
    let daemon = state.daemon.clone();
    with_limits(upgrade).on_upgrade(move |socket| connection(daemon, socket, Access::Token))
}

/// The process behind a Unix socket connection, read from the socket's peer credentials.
#[derive(Clone, Copy, Debug)]
pub(crate) struct UnixPeer {
    pub(crate) pid: Option<i32>,
}

impl
    axum::extract::connect_info::Connected<
        axum::serve::IncomingStream<'_, tokio::net::UnixListener>,
    > for UnixPeer
{
    fn connect_info(stream: axum::serve::IncomingStream<'_, tokio::net::UnixListener>) -> Self {
        UnixPeer {
            pid: stream.io().peer_cred().ok().and_then(|cred| cred.pid()),
        }
    }
}

/// The Unix socket: the user's CLI, the mobile host, and the agents, which get `Access::Agent`.
async fn ws_unix_handler(
    State(state): State<AppState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<UnixPeer>,
    upgrade: WebSocketUpgrade,
) -> HttpResponse {
    let daemon = state.daemon.clone();
    let access = match crate::access::is_agent(&daemon, peer.pid) {
        true => Access::Agent,
        false => Access::Private,
    };
    with_limits(upgrade).on_upgrade(move |socket| connection(daemon, socket, access))
}

/// `/peer`, on the Unix socket only: another daemon driving a transfer, or a cloud process of the
/// user's (`PeerCall`). Never an agent's.
async fn peer_handler(
    State(state): State<AppState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<UnixPeer>,
    upgrade: WebSocketUpgrade,
) -> HttpResponse {
    let daemon = state.daemon.clone();
    if crate::access::is_agent(&daemon, peer.pid) {
        return (
            StatusCode::FORBIDDEN,
            "An agent may not call the daemon's peer methods: they are another daemon's",
        )
            .into_response();
    }
    with_limits(upgrade).on_upgrade(move |socket| peer_connection(daemon, socket))
}

/// The largest message a client sends: a message's attached files travel in it, base64-encoded
/// (a 20 MB file is about 27 MB). The app keeps a message's files under it (`MAX_MESSAGE_FILE_BYTES`).
pub const MAX_MESSAGE_BYTES: usize = 64 << 20;

/// A browser sends a message as one frame: a frame limit under the message's (tungstenite's
/// default is 16 MiB) closed the connection on any file over about 12 MB.
pub fn with_limits(upgrade: WebSocketUpgrade) -> WebSocketUpgrade {
    upgrade
        .max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_MESSAGE_BYTES)
}

/// Any web page can open `ws://127.0.0.1`: only the app's own origins are accepted, and the
/// Vite dev server's (port 1420, or `CHAUFFEUR_VITE_PORT`), not every dev server on localhost,
/// which includes the workspaces' own, running the code agents write.
pub(crate) fn origin_allowed(origin: &str) -> bool {
    if matches!(
        origin,
        "tauri://localhost" | "http://tauri.localhost" | "https://tauri.localhost"
    ) {
        return true;
    }
    let dev_port = std::env::var("CHAUFFEUR_VITE_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(1420);
    origin == format!("http://localhost:{dev_port}")
        || origin == format!("http://127.0.0.1:{dev_port}")
}

pub async fn connection(daemon: Arc<Daemon>, socket: WebSocket, access: Access) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(text) = rx.recv().await {
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });
    let mut subscriptions: HashMap<String, JoinHandle<()>> = HashMap::new();
    let shutdown = daemon.shutdown.clone();
    loop {
        // Open connections must not hold the daemon up when it shuts down.
        let message = tokio::select! {
            next = stream.next() => match next {
                Some(Ok(message)) => message,
                Some(Err(error)) => {
                    tracing::warn!(%error, "WebSocket connection closed on an error");
                    break;
                }
                None => break,
            },
            _ = shutdown.cancelled() => break,
        };
        let text = match message {
            Message::Text(text) => text.to_string(),
            Message::Binary(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Message::Close(_) => break,
            _ => continue,
        };
        let incoming: Incoming = match serde_json::from_str(&text) {
            Ok(incoming) => incoming,
            Err(error) => {
                send(
                    &tx,
                    &Response::err(Value::Null, rpc_error(codes::PARSE_ERROR, error)),
                );
                continue;
            }
        };
        let id = incoming.id.clone().unwrap_or(Value::Null);
        let call: ClientCall = match serde_json::from_value(incoming.call_value()) {
            Ok(call) => call,
            Err(_) if incoming.method.as_deref().is_some_and(is_peer_method) => {
                let method = incoming.method.as_deref().unwrap_or_default();
                let refusal = crate::Error::BadRequest(format!(
                    "{method} is another daemon's method: call it on the private Unix socket's /peer path"
                ));
                send(&tx, &Response::err(id, refusal.to_rpc()));
                continue;
            }
            Err(error) => {
                let code = if incoming.method.as_deref().is_some_and(is_known_method) {
                    codes::INVALID_PARAMS
                } else {
                    codes::METHOD_NOT_FOUND
                };
                send(&tx, &Response::err(id, rpc_error(code, error)));
                continue;
            }
        };
        if let Err(error) = crate::access::check(access, &call) {
            send(&tx, &Response::err(id, error.to_rpc()));
            continue;
        }
        match call {
            ClientCall::EventsSubscribe(params) => {
                let subscription = uuid::Uuid::now_v7().to_string();
                // A moved workspace's events are the machine's it runs on: it answers there.
                match daemon.relay.subscription_target(&daemon, &params).await {
                    Ok(Some(target)) => {
                        let handle = crate::relay_events::spawn_subscription(
                            daemon.clone(),
                            target,
                            subscription.clone(),
                            params,
                            id,
                            tx.clone(),
                        );
                        subscriptions.insert(subscription, handle);
                        continue;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        send(&tx, &Response::err(id, error.to_rpc()));
                        continue;
                    }
                }
                // The live receiver is opened before `last_seq` is read: an event persisted
                // between the two would otherwise be neither replayed nor delivered, while the
                // client takes the `last_seq` it gets as current.
                let live = daemon.events.subscribe();
                let last_seq = daemon.store.last_seq().unwrap_or(0);
                match resolve_filter(&daemon, &params) {
                    Ok(filter) => {
                        let handle = spawn_subscription(
                            daemon.clone(),
                            live,
                            last_seq,
                            subscription.clone(),
                            params,
                            filter,
                            tx.clone(),
                        );
                        subscriptions.insert(subscription.clone(), handle);
                        send(
                            &tx,
                            &Response::ok(
                                id,
                                serde_json::to_value(SubscribeResult {
                                    subscription,
                                    last_seq,
                                })
                                .unwrap(),
                            ),
                        );
                    }
                    Err(error) => send(&tx, &Response::err(id, error.to_rpc())),
                }
            }
            ClientCall::ExtensionsWatch(params) => {
                // A view of an extension's UI: its lease keeps that version loaded until it is
                // unsubscribed or the connection closes, and it hears that extension's events.
                match daemon.extensions.watch(params.id) {
                    Ok(lease) => {
                        let subscription = uuid::Uuid::now_v7().to_string();
                        let live = daemon.events.subscribe();
                        let last_seq = daemon.store.last_seq().unwrap_or(0);
                        let handle =
                            spawn_watch(lease, live, params.id, subscription.clone(), tx.clone());
                        subscriptions.insert(subscription.clone(), handle);
                        send(
                            &tx,
                            &Response::ok(
                                id,
                                serde_json::to_value(SubscribeResult {
                                    subscription,
                                    last_seq,
                                })
                                .expect("a subscription serializes"),
                            ),
                        );
                    }
                    Err(error) => send(&tx, &Response::err(id, error.to_rpc())),
                }
            }
            ClientCall::EventsUnsubscribe(params) => {
                if let Some(handle) = subscriptions.remove(&params.subscription) {
                    handle.abort();
                }
                send(&tx, &Response::ok(id, json!({})));
            }
            call => {
                let daemon = daemon.clone();
                let tx = tx.clone();
                let method = incoming.method.clone().unwrap_or_default();
                // Names the failure the app may report, with the lines logged meanwhile.
                let request = uuid::Uuid::now_v7().to_string();
                let span = tracing::error_span!("request", id = %request, %method);
                // Long calls (workspace.wait) must not block the connection.
                tokio::spawn(
                    async move {
                        // A moved workspace's chats are answered by the machine it runs on.
                        let result = match daemon.relay.target(&daemon, &call).await {
                            Ok(Some(target)) => {
                                if target.acts {
                                    daemon.idle_reminders.active(&daemon, &target.workspace);
                                }
                                daemon.relay.forward(&daemon, &target.machine, call).await
                            }
                            Ok(None) => crate::rpc::handle(&daemon, call).await,
                            Err(error) => Err(error),
                        };
                        let response = match result {
                            Ok(value) => Response::ok(id, value),
                            Err(error) => Response::err(
                                id,
                                daemon
                                    .error_reports
                                    .failed(&request, &method, error.to_rpc()),
                            ),
                        };
                        send(&tx, &response);
                    }
                    .instrument(span),
                );
            }
        }
    }
    for (_, handle) in subscriptions {
        handle.abort();
    }
    drop(tx);
    let _ = writer.await;
}

/// A `/peer` connection: its calls answered as they end, except its archive calls, run one after
/// the other in the order they came so that a driver can send a handle's blocks without waiting
/// for each answer; its archive handles are closed with it.
async fn peer_connection(daemon: Arc<Daemon>, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(text) = rx.recv().await {
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });
    let archives =
        crate::cloud_transfer_driver::archive_bridge::ArchiveBridge::new(&daemon.paths.home);
    let (archive_tx, mut archive_rx) = mpsc::unbounded_channel::<(Value, PeerCall)>();
    let ordered = {
        let archives = archives.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            while let Some((id, call)) = archive_rx.recv().await {
                let response = match archives.handle(call).await {
                    Ok(value) => Response::ok(id, value),
                    Err(error) => Response::err(id, error.to_rpc()),
                };
                send(&tx, &response);
            }
        })
    };
    let shutdown = daemon.shutdown.clone();
    loop {
        let message = tokio::select! {
            next = stream.next() => match next {
                Some(Ok(message)) => message,
                Some(Err(error)) => {
                    tracing::warn!(%error, "peer connection closed on an error");
                    break;
                }
                None => break,
            },
            _ = shutdown.cancelled() => break,
        };
        let text = match message {
            Message::Text(text) => text.to_string(),
            Message::Binary(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Message::Close(_) => break,
            _ => continue,
        };
        let incoming: Incoming = match serde_json::from_str(&text) {
            Ok(incoming) => incoming,
            Err(error) => {
                send(
                    &tx,
                    &Response::err(Value::Null, rpc_error(codes::PARSE_ERROR, error)),
                );
                continue;
            }
        };
        let id = incoming.id.clone().unwrap_or(Value::Null);
        let call: PeerCall = match serde_json::from_value(incoming.call_value()) {
            Ok(call) => call,
            Err(error) => {
                let method = incoming.method.as_deref().unwrap_or_default();
                let code = match PeerMethod::from_name(method) {
                    Some(_) => codes::INVALID_PARAMS,
                    None => codes::METHOD_NOT_FOUND,
                };
                send(&tx, &Response::err(id, rpc_error(code, error)));
                continue;
            }
        };
        if crate::cloud_transfer_driver::archive_bridge::is_archive_call(&call) {
            let _ = archive_tx.send((id, call));
            continue;
        }
        let daemon = daemon.clone();
        let tx = tx.clone();
        // A transfer's steps run long: they must not hold the connection's other calls.
        tokio::spawn(async move {
            let response = match crate::peer::handle(&daemon, call).await {
                Ok(value) => Response::ok(id, value),
                Err(error) => Response::err(id, error.to_rpc()),
            };
            send(&tx, &response);
        });
    }
    // Closed first, the bridge refuses the archive calls still queued rather than run them.
    archives.close();
    drop(archive_tx);
    let _ = ordered.await;
    drop(tx);
    let _ = writer.await;
}

fn is_peer_method(method: &str) -> bool {
    PeerMethod::from_name(method).is_some_and(|m| m != PeerMethod::Hello)
}

fn is_known_method(method: &str) -> bool {
    serde_json::from_value::<ClientCall>(json!({ "method": method, "params": {} })).is_ok()
        || method.contains('.')
}

fn rpc_error(code: i64, error: impl std::fmt::Display) -> RpcError {
    RpcError {
        code,
        message: error.to_string(),
        failure: None,
        data: None,
    }
}

fn send<T: serde::Serialize>(tx: &mpsc::UnboundedSender<String>, message: &T) {
    if let Ok(text) = serde_json::to_string(message) {
        let _ = tx.send(text);
    }
}

fn resolve_filter(daemon: &Arc<Daemon>, params: &SubscribeParams) -> crate::Result<EventFilter> {
    let mut filter = EventFilter {
        all: params.all,
        registry: params.registry,
        office: params.office,
        ..Default::default()
    };
    for workspace in &params.workspaces {
        filter.workspaces.push(daemon.workspaces.row(workspace)?.id);
    }
    for chat in &params.chats {
        filter.chats.push(daemon.chats.resolve(chat)?.id);
    }
    Ok(filter)
}

/// Replays persisted events after `after_seq`, then forwards live ones, without gaps or
/// duplicates: `live` was opened before `last_seq` was read, and is deduplicated by sequence.
fn spawn_subscription(
    daemon: Arc<Daemon>,
    mut live: tokio::sync::broadcast::Receiver<Arc<EventEnvelope>>,
    last_seq: i64,
    subscription: String,
    params: SubscribeParams,
    filter: EventFilter,
    tx: mpsc::UnboundedSender<String>,
) -> JoinHandle<()> {
    // An Office view: the daemon stays connected to the office server while it is subscribed.
    let viewer = params
        .office
        .then(|| {
            daemon
                .extensions
                .viewer(chauffeur_protocol::extensions::ExtensionId::Office, &daemon)
        })
        .flatten();
    tokio::spawn(async move {
        let _viewer = viewer;
        let notify = |envelope| {
            send(
                &tx,
                &Notification::new(DaemonNotification::Event(EventNotification {
                    subscription: subscription.clone(),
                    envelope,
                })),
            )
        };
        let mut last_seen = match (params.after_seq, params.replay) {
            (Some(seq), _) => seq,
            (None, Some(_)) => 0,
            (None, None) => last_seq,
        };
        if params.after_seq.is_some() || params.replay.is_some() {
            for envelope in stored(&daemon, last_seen, &filter, params.replay).await {
                last_seen = last_seen.max(envelope.seq.unwrap_or(0));
                notify(envelope);
            }
            // The history is out: a client that waits for it can stop.
            if params.ephemeral {
                for workspace in &filter.workspaces {
                    notify(EventEnvelope {
                        seq: None,
                        ts: crate::store::now(),
                        event: Event::Workspace {
                            workspace_id: workspace.clone(),
                            event: WorkspaceEvent::Replayed,
                        },
                    });
                }
            }
            // The older part is a one-off: the subscription that has the tail follows the log.
            if matches!(params.replay, Some(Replay::BeforeTail { .. })) {
                return;
            }
        }
        loop {
            match live.recv().await {
                Ok(envelope) => {
                    if let Some(seq) = envelope.seq {
                        if seq <= last_seen {
                            continue;
                        }
                        last_seen = seq;
                    } else if !params.ephemeral
                        && !matches!(
                            envelope.event,
                            Event::Office { .. } | Event::Extension { .. }
                        )
                        && !(params.registry && crate::events::for_registry(&envelope.event))
                    {
                        // Office events are ephemeral too, and what an `office` subscription is
                        // for; a sidebar's shares and suggestions are a registry subscription's.
                        continue;
                    }
                    if crate::events::matches(&filter, &envelope) {
                        notify(Arc::unwrap_or_clone(envelope));
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // Too slow: catch up from the store (ephemeral events are lost).
                    for envelope in stored(&daemon, last_seen, &filter, None).await {
                        last_seen = last_seen.max(envelope.seq.unwrap_or(0));
                        notify(envelope);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
            if tx.is_closed() {
                break;
            }
        }
    })
}

/// An `extensions.watch` subscription: the extension's own events, live (they are never stored),
/// while `lease` keeps its view counted.
fn spawn_watch(
    lease: Box<dyn Send>,
    mut live: tokio::sync::broadcast::Receiver<Arc<EventEnvelope>>,
    extension: chauffeur_protocol::extensions::ExtensionId,
    subscription: String,
    tx: mpsc::UnboundedSender<String>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let _lease = lease;
        loop {
            match live.recv().await {
                Ok(envelope) => {
                    if matches!(&envelope.event, Event::Extension { id, .. } if *id == extension) {
                        send(
                            &tx,
                            &Notification::new(DaemonNotification::Event(EventNotification {
                                subscription: subscription.clone(),
                                envelope: Arc::unwrap_or_clone(envelope),
                            })),
                        );
                    }
                }
                // Too slow: the events missed are ephemeral, gone.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
            if tx.is_closed() {
                break;
            }
        }
    })
}

/// Stored events after `after_seq`, read on a blocking thread: a workspace's history can take a
/// while to read and decode, and the async workers serve every other client meanwhile.
async fn stored(
    daemon: &Arc<Daemon>,
    after_seq: i64,
    filter: &EventFilter,
    part: Option<Replay>,
) -> Vec<EventEnvelope> {
    let store = daemon.store.clone();
    let filter = filter.clone();
    tokio::task::spawn_blocking(move || store.events_part(after_seq, &filter, part))
        .await
        .ok()
        .and_then(|read| read.ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn deliverable_downloads_require_auth_and_serve_only_scoped_documents_as_attachments() {
        let temp = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::new(
            temp.path().join("home"),
            temp.path().join("workspaces"),
            temp.path().join("repos"),
        );
        paths.ensure().unwrap();
        std::fs::write(
            &paths.settings,
            "linear_enabled = false\ngithub_sources = false\n",
        )
        .unwrap();
        let daemon = Daemon::open(paths.clone()).unwrap();
        let workspace = paths.workspaces_dir.join("documents");
        let artifacts = workspace.join(".chauffeur/artifacts");
        std::fs::create_dir_all(&artifacts).unwrap();
        daemon
            .store
            .insert_workspace(&crate::store::WorkspaceRow {
                id: "docs".into(),
                slug: "documents".into(),
                path: workspace.to_string_lossy().into_owned(),
                branch: "docs".into(),
                parent_id: None,
                state: chauffeur_protocol::model::WorkspaceState::Ready,
                tickets: vec![],
                created_at: String::new(),
                archived_at: None,
                last_error: None,
                attention_at: None,
                seen_at: None,
                explore: false,
                title: None,
                auto_title: false,
                title_source: None,
                pinned: false,
                position: 0.0,
                section_id: None,
                house_position: None,
            })
            .unwrap();
        std::fs::write(artifacts.join("page.html"), "<h1>One page</h1>").unwrap();
        std::fs::write(artifacts.join("page.pdf"), "%PDF-1.4\nfixture").unwrap();
        std::fs::write(artifacts.join("fake.pdf"), "<script>bad()</script>").unwrap();
        let outside = temp.path().join("outside.html");
        std::fs::write(&outside, "outside").unwrap();
        std::os::unix::fs::symlink(outside, artifacts.join("escape.html")).unwrap();
        let state = AppState {
            daemon: daemon.clone(),
            token: Arc::new("secret".into()),
            require_token: true,
        };
        for (path, issue_ticket, expected) in [
            (
                ".chauffeur/artifacts/page.html",
                false,
                StatusCode::UNAUTHORIZED,
            ),
            (".chauffeur/artifacts/page.html", true, StatusCode::OK),
            (".chauffeur/artifacts/page.pdf", true, StatusCode::OK),
            (".chauffeur/artifacts/fake.pdf", true, StatusCode::NOT_FOUND),
            (
                ".chauffeur/artifacts/escape.html",
                true,
                StatusCode::NOT_FOUND,
            ),
            (
                ".chauffeur/artifacts/missing.html",
                true,
                StatusCode::NOT_FOUND,
            ),
            (
                ".chauffeur/artifacts/../page.html",
                true,
                StatusCode::UNAUTHORIZED,
            ),
            (
                ".chauffeur/artifacts/%2e%2e/page.html",
                true,
                StatusCode::UNAUTHORIZED,
            ),
            ("/tmp/page.html", true, StatusCode::UNAUTHORIZED),
        ] {
            let ticket = if issue_ticket {
                daemon.downloads.issue("docs", path).unwrap()
            } else {
                String::new()
            };
            let response = deliverable_handler(
                State(state.clone()),
                Path("docs".into()),
                Query(HashMap::from([
                    ("path".into(), path.into()),
                    ("ticket".into(), ticket),
                ])),
                HeaderMap::new(),
            )
            .await;
            assert_eq!(response.status(), expected, "{path}");
            if expected == StatusCode::OK {
                assert!(
                    response.headers()[header::CONTENT_DISPOSITION]
                        .to_str()
                        .unwrap()
                        .starts_with("attachment;")
                );
                assert_eq!(
                    response.headers()[header::X_CONTENT_TYPE_OPTIONS],
                    "nosniff"
                );
                assert!(
                    !axum::body::to_bytes(response.into_body(), 1024)
                        .await
                        .unwrap()
                        .is_empty()
                );
            }
        }
        let response = deliverable_handler(
            State(state.clone()),
            Path("docs".into()),
            Query(HashMap::from([
                ("token".into(), "secret".into()),
                ("path".into(), ".chauffeur/artifacts/page.html".into()),
            ])),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let ticket = daemon
            .downloads
            .issue("docs", ".chauffeur/artifacts/page.html")
            .unwrap();
        let response = deliverable_handler(
            State(state),
            Path("docs".into()),
            Query(HashMap::from([
                ("ticket".into(), ticket),
                ("path".into(), ".chauffeur/artifacts/page.pdf".into()),
            ])),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap(),
            "<h1>One page</h1>"
        );
        daemon.shutdown.cancel();
    }

    #[test]
    fn a_remote_document_keeps_its_attachment_disposition_without_upstream_cookies() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            header::CONTENT_DISPOSITION,
            "attachment; filename=report.html".parse().unwrap(),
        );
        headers.insert(header::CONTENT_TYPE, "text/html".parse().unwrap());
        headers.insert(header::SET_COOKIE, "private=value".parse().unwrap());
        let forwarded = relay_response_headers(&headers);
        assert_eq!(
            forwarded[header::CONTENT_DISPOSITION],
            "attachment; filename=report.html"
        );
        assert_eq!(forwarded[header::CONTENT_TYPE], "text/html");
        assert!(!forwarded.contains_key(header::SET_COOKIE));
    }

    fn extension_event(id: chauffeur_protocol::extensions::ExtensionId) -> Event {
        Event::Extension {
            id,
            event: json!({ "type": "something_new", "from": format!("{id:?}") }),
        }
    }

    #[tokio::test]
    async fn an_extension_watch_hears_its_extension_only_and_holds_its_lease_until_it_ends() {
        use chauffeur_protocol::extensions::ExtensionId;
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Lease(Arc<AtomicBool>);
        impl Drop for Lease {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let released = Arc::new(AtomicBool::new(false));
        let (live_tx, live) = tokio::sync::broadcast::channel(16);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let watch = spawn_watch(
            Box::new(Lease(released.clone())),
            live,
            ExtensionId::Office,
            "s".into(),
            tx,
        );
        for event in [
            extension_event(ExtensionId::Database),
            Event::Office {
                event: chauffeur_protocol::office::OfficeEvent::BrowserJoined,
            },
            extension_event(ExtensionId::Office),
        ] {
            live_tx
                .send(Arc::new(EventEnvelope {
                    seq: None,
                    ts: String::new(),
                    event,
                }))
                .unwrap();
        }
        let heard: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert_eq!(heard["params"]["subscription"], "s");
        assert_eq!(
            heard["params"]["envelope"]["event"]["id"], "office",
            "{heard}"
        );
        assert_eq!(
            heard["params"]["envelope"]["event"]["event"]["type"],
            "something_new"
        );
        assert!(rx.try_recv().is_err(), "nothing else");
        assert!(!released.load(Ordering::SeqCst));
        watch.abort();
        let _ = watch.await;
        assert!(
            released.load(Ordering::SeqCst),
            "unsubscribing ends the view"
        );
    }

    #[tokio::test]
    async fn an_office_view_hears_the_offices_generic_events_without_asking_for_ephemeral_ones() {
        use chauffeur_protocol::extensions::ExtensionId;
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::new(
            dir.path().join("home"),
            dir.path().join("workspaces"),
            dir.path().join("repos"),
        );
        paths.ensure().unwrap();
        std::fs::write(&paths.settings, "linear_enabled = false\n").unwrap();
        let daemon = Daemon::open(paths).unwrap();
        let params: SubscribeParams =
            serde_json::from_value(json!({ "office": true, "ephemeral": false })).unwrap();
        let filter = resolve_filter(&daemon, &params).unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let subscription = spawn_subscription(
            daemon.clone(),
            daemon.events.subscribe(),
            0,
            "s".into(),
            params,
            filter,
            tx,
        );
        daemon
            .events
            .publish(extension_event(ExtensionId::Database));
        daemon.events.publish(extension_event(ExtensionId::Office));
        let heard: Value = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .map(|text| serde_json::from_str(&text).unwrap())
            .unwrap();
        assert_eq!(
            heard["params"]["envelope"]["event"]["id"], "office",
            "{heard}"
        );
        subscription.abort();
    }

    /// A message the size of a 20 MB file, base64-encoded, sent as one frame as a browser does.
    #[tokio::test]
    async fn a_message_carrying_a_20_mb_file_goes_through_in_one_frame() {
        async fn echo_length(upgrade: WebSocketUpgrade) -> HttpResponse {
            with_limits(upgrade).on_upgrade(|mut socket| async move {
                if let Some(Ok(Message::Text(text))) = socket.next().await {
                    let _ = socket
                        .send(Message::Text(text.len().to_string().into()))
                        .await;
                }
            })
        }
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/ws", get(echo_length)))
                .await
                .unwrap();
        });
        let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
            .await
            .unwrap();
        let size = (attachments::MAX_BYTES / 3 + 1) * 4;
        let message = "a".repeat(size);
        client
            .send(tokio_tungstenite::tungstenite::Message::Text(
                message.into(),
            ))
            .await
            .unwrap();
        let answer = client.next().await.unwrap().unwrap();
        assert_eq!(answer.into_text().unwrap().as_str(), size.to_string());
        server.abort();
    }

    #[test]
    fn the_token_file_is_created_private_and_kept_so() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("token");
        let token = load_or_create_token(&path).unwrap();
        assert_eq!(token.len(), 64);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // Loosened afterwards (a backup, `chmod -R`): fixed, and the token kept.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(load_or_create_token(&path).unwrap(), token);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn a_link_in_place_of_the_token_file_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let elsewhere = temp.path().join("elsewhere");
        std::fs::write(&elsewhere, "stolen").unwrap();
        let path = temp.path().join("token");
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
        let error = load_or_create_token(&path).unwrap_err().to_string();
        assert!(error.contains("not a regular file"), "{error}");
        assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), "stolen");
    }

    #[test]
    fn a_player_gets_the_byte_range_it_asks_for() {
        assert_eq!(byte_range("bytes=0-1", 100), Some((0, 2)));
        assert_eq!(byte_range("bytes=10-", 100), Some((10, 100)));
        assert_eq!(byte_range("bytes=-30", 100), Some((70, 100)));
        // Past the end is cut to the file; only the first of several ranges is served.
        assert_eq!(byte_range("bytes=90-500", 100), Some((90, 100)));
        assert_eq!(byte_range("bytes=0-9, 20-29", 100), Some((0, 10)));
        for unsatisfiable in [
            "bytes=100-",
            "bytes=5-2",
            "bytes=abc",
            "items=0-1",
            "bytes=0-",
            "",
        ] {
            let size = if unsatisfiable == "bytes=0-" { 0 } else { 100 };
            assert_eq!(byte_range(unsatisfiable, size), None, "{unsatisfiable}");
        }
    }

    #[test]
    fn only_the_apps_origins_and_the_dev_server_may_connect() {
        for origin in [
            "tauri://localhost",
            "http://tauri.localhost",
            "http://localhost:1420",
            "http://127.0.0.1:1420",
        ] {
            assert!(origin_allowed(origin), "{origin}");
        }
        for origin in [
            "http://localhost:3000",
            "http://127.0.0.1:41190",
            "https://evil.example",
            "http://localhost",
        ] {
            assert!(!origin_allowed(origin), "{origin}");
        }
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn archive_handles_stay_on_their_unix_connection_and_public_websockets_refuse_every_archive_and_transfer_method()
     {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        use sha2::{Digest, Sha256};
        use std::time::Duration;
        use tokio_tungstenite::{WebSocketStream, tungstenite::Message as WsMessage};
        async fn request<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
            socket: &mut WebSocketStream<S>,
            method: &str,
            params: Value,
        ) -> Value {
            socket
                .send(WsMessage::Text(
                    json!({"jsonrpc":"2.0", "id":1, "method":method, "params":params})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            serde_json::from_str(message.to_text().unwrap()).unwrap()
        }
        let temp = tempfile::Builder::new()
            .prefix("chf-arc-")
            .tempdir_in("/tmp")
            .unwrap();
        let paths = crate::paths::Paths::new(
            temp.path().join("home"),
            temp.path().join("workspaces"),
            temp.path().join("repos"),
        );
        paths.ensure().unwrap();
        std::fs::write(
            &paths.settings,
            "linear_enabled = false\ngithub_org = \"\"\ngithub_sources = false\n",
        )
        .unwrap();
        let daemon = Daemon::open(paths.clone()).unwrap();
        let router = |require_token| {
            match require_token {
                true => Router::new().route("/ws", get(ws_handler)),
                false => Router::new()
                    .route("/ws", get(ws_unix_handler))
                    .route("/peer", get(peer_handler)),
            }
            .with_state(AppState {
                daemon: daemon.clone(),
                token: Arc::new("archive-fixture".into()),
                require_token,
            })
        };
        let unix = tokio::net::UnixListener::bind(&paths.socket).unwrap();
        let unix_router = router(false);
        let unix_server = tokio::spawn(async move {
            axum::serve(
                unix,
                unix_router.into_make_service_with_connect_info::<UnixPeer>(),
            )
            .await
            .unwrap();
        });
        let tcp = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = tcp.local_addr().unwrap().port();
        let public_router = router(true);
        let public_server = tokio::spawn(async move {
            axum::serve(tcp, public_router).await.unwrap();
        });
        let connect = || async {
            tokio_tungstenite::client_async(
                "ws://localhost/peer",
                tokio::net::UnixStream::connect(&paths.socket)
                    .await
                    .unwrap(),
            )
            .await
            .unwrap()
            .0
        };
        let mut owner = connect().await;
        let mut other = connect().await;
        let peer = |n: char| format!("12D3KooW{}", n.to_string().repeat(44));
        let bytes = [7u8; 80];
        let operation = uuid::Uuid::now_v7().to_string();
        let mut binding = json!({"context":{"workspace_id":uuid::Uuid::now_v7().to_string(), "owner_peer_id":peer('1'),"controller_peer_id":peer('2'),"operation_id":operation,"source_peer_id":peer('1'),"target_peer_id":peer('3'),"source_epoch":1,"snapshot_digest":URL_SAFE_NO_PAD.encode([2;32])}, "local_peer_id":peer('3'),"head":{"bytes":80,"digest":URL_SAFE_NO_PAD.encode(Sha256::digest(bytes))}});
        let opened = request(
            &mut owner,
            "cloud.archive.receive_begin",
            json!({"binding":binding,"allow_new":true}),
        )
        .await;
        let handle = opened["result"]["handle"].as_str().unwrap().to_string();
        let foreign = request(
            &mut other,
            "cloud.archive.write",
            json!({"handle":handle,"offset":0,"bytes":URL_SAFE_NO_PAD.encode(bytes)}),
        )
        .await;
        assert!(foreign.get("error").is_some());
        assert!(
            request(
                &mut owner,
                "cloud.archive.write",
                json!({"handle":handle,"offset":0,"bytes":URL_SAFE_NO_PAD.encode(bytes)})
            )
            .await
            .get("error")
            .is_none()
        );
        owner.close(None).await.unwrap();
        let until = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let resumed = request(
                &mut other,
                "cloud.archive.receive_begin",
                json!({"binding":binding,"allow_new":false}),
            )
            .await;
            if resumed["result"]["kind"] == "stored" {
                assert_eq!(resumed["result"]["head"], binding["head"]);
                break;
            }
            assert!(tokio::time::Instant::now() < until);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let public_operation = uuid::Uuid::now_v7().to_string();
        binding["context"]["operation_id"] = public_operation.clone().into();
        let (mut public, _) = tokio_tungstenite::connect_async(format!(
            "ws://127.0.0.1:{port}/ws?token=archive-fixture"
        ))
        .await
        .unwrap();
        for (method, params) in [
            ("cloud.archive.source_open", binding.clone()),
            (
                "cloud.archive.receive_begin",
                json!({"binding":binding,"allow_new":true}),
            ),
            (
                "cloud.archive.read",
                json!({"handle":handle,"offset":0,"length":1}),
            ),
            (
                "cloud.archive.write",
                json!({"handle":handle,"offset":0,"bytes":"AQ"}),
            ),
            ("cloud.archive.commit", json!({"handle":handle})),
            ("cloud.archive.close", json!({"handle":handle})),
            ("cloud.transfer.peer", json!({})),
            (
                "cloud.transfer.journal",
                json!({"action":"read","workspace_id":uuid::Uuid::now_v7().to_string(),"owner_peer_id":"peer"}),
            ),
            ("cloud.transfer.source_stop", json!({"state":{}})),
            (
                "cloud.transfer.source_capture",
                json!({"stopping":{},"stopped":{}}),
            ),
            ("cloud.transfer.source_seal", json!({"state":{}})),
            ("cloud.transfer.source_retire", json!({"state":{}})),
            ("cloud.transfer.source_discard", json!({"state":{}})),
            ("cloud.transfer.source_release", json!({"state":{}})),
            ("cloud.transfer.destination_abandon", json!({"state":{}})),
            // An app names machines of `[transfer]`; sockets and a controller are the CLI's.
            (
                "cloud.transfer.start",
                json!({"workspace":"moving","source":{"kind":"here"},"destination":{"kind":"socket","path":"/tmp/x.sock"}}),
            ),
            (
                "cloud.transfer.start",
                json!({"workspace":"moving","source":{"kind":"here"},"destination":{"kind":"machine","name":"vm"},"controller":"/tmp/c.sock"}),
            ),
            (
                "cloud.transfer.destination_restore",
                json!({"published":{},"key":"AQ","binding":binding}),
            ),
            ("cloud.transfer.destination_activate", json!({"state":{}})),
            (
                "cloud.transfer.destination_check",
                json!({"workspace_id":uuid::Uuid::now_v7().to_string(),"slug":"moving","source_peer_id":"peer","agents":[]}),
            ),
        ] {
            let denied = request(&mut public, method, params).await;
            assert!(
                denied["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("private Unix socket"),
                "{denied}"
            );
        }
        assert!(
            !paths
                .home
                .join("workspace-archives")
                .join(public_operation)
                .exists()
        );
        other.close(None).await.unwrap();
        public.close(None).await.unwrap();
        daemon.shutdown.cancel();
        unix_server.abort();
        public_server.abort();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_handle_s_blocks_sent_without_waiting_are_written_in_the_order_they_came() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        use sha2::{Digest, Sha256};
        use std::time::Duration;
        use tokio_tungstenite::tungstenite::Message as WsMessage;
        let temp = tempfile::Builder::new()
            .prefix("chf-ord-")
            .tempdir_in("/tmp")
            .unwrap();
        let paths = crate::paths::Paths::new(
            temp.path().join("home"),
            temp.path().join("workspaces"),
            temp.path().join("repos"),
        );
        paths.ensure().unwrap();
        std::fs::write(
            &paths.settings,
            "linear_enabled = false\ngithub_org = \"\"\ngithub_sources = false\n",
        )
        .unwrap();
        let daemon = Daemon::open(paths.clone()).unwrap();
        let router = Router::new()
            .route("/peer", get(peer_handler))
            .with_state(AppState {
                daemon: daemon.clone(),
                token: Arc::new("order-fixture".into()),
                require_token: false,
            });
        let unix = tokio::net::UnixListener::bind(&paths.socket).unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                unix,
                router.into_make_service_with_connect_info::<UnixPeer>(),
            )
            .await
            .unwrap();
        });
        let (mut socket, _) = tokio_tungstenite::client_async(
            "ws://localhost/peer",
            tokio::net::UnixStream::connect(&paths.socket)
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        // Many small blocks: run each in a task of its own, some would come out of order.
        let (block, count) = (1024, 256);
        let bytes: Vec<u8> = (0..block * count).map(|i| (i / block) as u8).collect();
        let peer = |n: char| format!("12D3KooW{}", n.to_string().repeat(44));
        let binding = json!({"context":{"workspace_id":uuid::Uuid::now_v7().to_string(), "owner_peer_id":peer('1'),"controller_peer_id":peer('2'),"operation_id":uuid::Uuid::now_v7().to_string(),"source_peer_id":peer('1'),"target_peer_id":peer('3'),"source_epoch":1,"snapshot_digest":URL_SAFE_NO_PAD.encode([2;32])}, "local_peer_id":peer('3'),"head":{"bytes":bytes.len(),"digest":URL_SAFE_NO_PAD.encode(Sha256::digest(&bytes))}});
        socket
            .send(WsMessage::Text(
                json!({"jsonrpc":"2.0", "id":0, "method":"cloud.archive.receive_begin", "params":{"binding":binding,"allow_new":true}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let next = async |socket: &mut tokio_tungstenite::WebSocketStream<_>| -> Value {
            let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            serde_json::from_str(message.to_text().unwrap()).unwrap()
        };
        let opened = next(&mut socket).await;
        let handle = opened["result"]["handle"].as_str().unwrap().to_string();
        for (i, chunk) in bytes.chunks(block).enumerate() {
            socket
                .send(WsMessage::Text(
                    json!({"jsonrpc":"2.0", "id":i + 1, "method":"cloud.archive.write", "params":{"handle":handle,"offset":i * block,"bytes":URL_SAFE_NO_PAD.encode(chunk)}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
        }
        socket
            .send(WsMessage::Text(
                json!({"jsonrpc":"2.0", "id":count + 1, "method":"cloud.archive.commit", "params":{"handle":handle}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        for id in 1..=count + 1 {
            let answer = next(&mut socket).await;
            assert_eq!(answer["id"], id, "{answer}");
            assert!(answer.get("error").is_none(), "{answer}");
        }
        socket.close(None).await.unwrap();
        daemon.shutdown.cancel();
        server.abort();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_shell_command_is_refused_on_the_unix_socket_and_taken_from_the_app_s_connection() {
        use std::time::Duration;
        use tokio_tungstenite::tungstenite::Message as WsMessage;
        let temp = tempfile::Builder::new()
            .prefix("chf-sh-")
            .tempdir_in("/tmp")
            .unwrap();
        let paths = crate::paths::Paths::new(
            temp.path().join("home"),
            temp.path().join("workspaces"),
            temp.path().join("repos"),
        );
        paths.ensure().unwrap();
        std::fs::write(
            &paths.settings,
            "linear_enabled = false\ngithub_org = \"\"\ngithub_sources = false\n",
        )
        .unwrap();
        let daemon = Daemon::open(paths.clone()).unwrap();
        let router = |require_token| {
            match require_token {
                true => Router::new().route("/ws", get(ws_handler)),
                false => Router::new()
                    .route("/ws", get(ws_unix_handler))
                    .route("/peer", get(peer_handler)),
            }
            .with_state(AppState {
                daemon: daemon.clone(),
                token: Arc::new("shell-fixture".into()),
                require_token,
            })
        };
        let unix = tokio::net::UnixListener::bind(&paths.socket).unwrap();
        let unix_router = router(false);
        let unix_server = tokio::spawn(async move {
            axum::serve(
                unix,
                unix_router.into_make_service_with_connect_info::<UnixPeer>(),
            )
            .await
            .unwrap();
        });
        let tcp = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = tcp.local_addr().unwrap().port();
        let public_router = router(true);
        let public_server = tokio::spawn(async move {
            axum::serve(tcp, public_router).await.unwrap();
        });
        let shell = json!({"jsonrpc": "2.0", "id": 1, "method": "chat.shell", "params": {"chat": "nope", "command": "id"}})
            .to_string();
        let error_of = |message: WsMessage| {
            serde_json::from_str::<Value>(message.to_text().unwrap()).unwrap()["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        };

        let (mut cli, _) = tokio_tungstenite::client_async(
            "ws://localhost/ws",
            tokio::net::UnixStream::connect(&paths.socket)
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        cli.send(WsMessage::Text(shell.clone().into()))
            .await
            .unwrap();
        let answer = tokio::time::timeout(Duration::from_secs(5), cli.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            error_of(answer),
            "Shell commands run from the app's composer only"
        );

        // The app's connection gets past the gate: the chat is then looked up.
        let (mut app, _) = tokio_tungstenite::connect_async(format!(
            "ws://127.0.0.1:{port}/ws?token=shell-fixture"
        ))
        .await
        .unwrap();
        app.send(WsMessage::Text(shell.into())).await.unwrap();
        let answer = tokio::time::timeout(Duration::from_secs(5), app.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let error = error_of(answer);
        assert!(!error.is_empty() && !error.contains("composer"), "{error}");
        daemon.shutdown.cancel();
        unix_server.abort();
        public_server.abort();
    }

    /// A process under a sidecar connects to the Unix socket as an agent: its workspace calls
    /// go through, the user's trust, terminals and settings do not. The test process adopts
    /// itself as a sidecar, so its own connection is read as an agent's through the socket's
    /// peer credentials.
    #[tokio::test]
    async fn an_agent_on_the_unix_socket_keeps_its_workspace_and_not_the_user_s_trust() {
        use std::time::Duration;
        use tokio_tungstenite::tungstenite::Message as WsMessage;
        let temp = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::new(
            temp.path().join("home"),
            temp.path().join("workspaces"),
            temp.path().join("repos"),
        );
        paths.ensure().unwrap();
        std::fs::write(
            &paths.settings,
            "linear_enabled = false\ngithub_org = \"\"\ngithub_sources = false\n",
        )
        .unwrap();
        let daemon = Daemon::open(paths.clone()).unwrap();
        let router = Router::new()
            .route("/ws", get(ws_unix_handler))
            .with_state(AppState {
                daemon: daemon.clone(),
                token: Arc::new("agent-fixture".into()),
                require_token: false,
            });
        let unix = tokio::net::UnixListener::bind(&paths.socket).unwrap();
        let unix_server = tokio::spawn(async move {
            axum::serve(
                unix,
                router.into_make_service_with_connect_info::<UnixPeer>(),
            )
            .await
            .unwrap();
        });
        let connect = || async {
            tokio_tungstenite::client_async(
                "ws://localhost/ws",
                tokio::net::UnixStream::connect(&paths.socket)
                    .await
                    .unwrap(),
            )
            .await
            .unwrap()
            .0
        };
        let ask = |method: &str, params: Value| {
            json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string()
        };
        let answer_of =
            |message: WsMessage| serde_json::from_str::<Value>(message.to_text().unwrap()).unwrap();

        // The user's own CLI: endpoint trust is reachable.
        let mut user = connect().await;
        user.send(WsMessage::Text(ask("remote.devices", json!({})).into()))
            .await
            .unwrap();
        let answer = answer_of(
            tokio::time::timeout(Duration::from_secs(5), user.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
        );
        assert!(answer.get("result").is_some(), "{answer}");

        daemon.chats.sidecar.adopt(std::process::id());
        let mut agent = connect().await;
        for (method, params, refusal) in [
            (
                "remote.devices",
                json!({}),
                "An agent may not call remote.devices",
            ),
            (
                "remote.revoke",
                json!({"device_id":"d"}),
                "An agent may not call remote.revoke",
            ),
            (
                "terminal.open",
                json!({"workspace":"w","cols":80,"rows":24,"command":"id"}),
                "An agent may not call terminal.open",
            ),
            (
                "settings.set",
                json!({}),
                "An agent may not call settings.set",
            ),
            (
                "daemon.shutdown",
                json!({}),
                "An agent may not call daemon.shutdown",
            ),
            (
                "chat.shell",
                json!({"chat":"c","command":"id"}),
                "Shell commands run from the app's composer only",
            ),
        ] {
            agent
                .send(WsMessage::Text(ask(method, params).into()))
                .await
                .unwrap();
            let answer = answer_of(
                tokio::time::timeout(Duration::from_secs(5), agent.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap(),
            );
            let message = answer["error"]["message"].as_str().unwrap_or_default();
            assert!(message.starts_with(refusal), "{method}: {answer}");
        }
        agent
            .send(WsMessage::Text(ask("workspace.list", json!({})).into()))
            .await
            .unwrap();
        let answer = answer_of(
            tokio::time::timeout(Duration::from_secs(5), agent.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
        );
        assert!(answer.get("result").is_some(), "{answer}");
        daemon.shutdown.cancel();
        unix_server.abort();
    }
}
