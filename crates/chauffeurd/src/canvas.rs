//! A design chat's canvas (story 47): `canvas.json` read from its folder, and the screens served
//! on an origin of their own (a second listener on 127.0.0.1), never the daemon's or the app's.
//! The same listener serves an export's page to Chrome, once (design_export.rs), and the
//! workspace's HTML files for the Files tab's preview, with the files they load.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use axum::Router;
use axum::extract::{Path as UrlPath, RawQuery, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use base64::Engine;
pub use chauffeur_design::{
    MANIFEST, content_type, encode_path, manifest_error, read_canvas, safe_relative,
};
use chauffeur_design::{encode_segment, screen_file};
use chauffeur_protocol::client::{CanvasResult, DesignResource, DesignResourceParams};
use chauffeur_protocol::model::ChatKind;
use std::os::unix::fs::MetadataExt;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::{Daemon, Error, Result};

/// Appended to a screen served with `?capture=1` (the app's canvas, for comments' pins; never
/// Export PNG nor Open › Browser), story 51. The share service appends the same one.
const CAPTURE_SCRIPT: &str = chauffeur_protocol::design_share::DESIGN_CAPTURE_SCRIPT;

/// What a screen may load: anything over https (CDNs, fonts), its own files, inline code; never
/// plain http, so never localhost (the daemon, the user's dev servers). Only the app may frame it.
const SCREEN_CSP: &str = "default-src 'self' https: data: blob: 'unsafe-inline' 'unsafe-eval'; \
connect-src 'self' https:; form-action 'none'; base-uri 'self'; frame-ancestors tauri://localhost \
http://tauri.localhost https://tauri.localhost http://localhost:* http://127.0.0.1:*";

/// Added for a screen the app pictures in an offscreen web view (`snapshot=1`, story 47): opened
/// there as a page of its own, it keeps the opaque origin and the limits of its canvas frame.
const SNAPSHOT_SANDBOX: &str = "sandbox allow-scripts allow-forms";

/// The screens' listener: its port once bound, and the secret every URL starts with, drawn at
/// each daemon start and handed out only by the authenticated `design.canvas`.
pub struct DesignServer {
    secret: String,
    port: OnceLock<u16>,
    /// The export pages waiting for Chrome (design_export.rs).
    pub exports: crate::design_export::ExportPages,
    /// A test's fake Chrome, rendering exports instead of the machine's.
    chrome: Mutex<Option<PathBuf>>,
}

impl Default for DesignServer {
    fn default() -> Self {
        DesignServer {
            secret: rand::random::<[u8; 16]>()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            port: OnceLock::new(),
            exports: Default::default(),
            chrome: Mutex::new(None),
        }
    }
}

impl DesignServer {
    /// Where a chat's screens are served, ending with `/`; none before the listener is up.
    pub fn base_url(&self, workspace_id: &str, chat_id: &str) -> Option<String> {
        let port = self.port.get()?;
        Some(format!(
            "http://127.0.0.1:{port}/{}/{}/{}/",
            self.secret,
            encode_segment(workspace_id),
            encode_segment(chat_id)
        ))
    }

    /// Where a workspace's file is served for the Files tab's preview (`path` as `files.read`
    /// names it), so that the files it loads by relative URLs come along; none before the
    /// listener is up.
    pub fn file_url(&self, workspace_id: &str, path: &str) -> Option<String> {
        let port = self.port.get()?;
        let path = path.split('/').map(encode_segment).collect::<Vec<_>>();
        Some(format!(
            "http://127.0.0.1:{port}/files/{}/{}/{}",
            self.secret,
            encode_segment(workspace_id),
            path.join("/")
        ))
    }

    /// Where an export's page is served to Chrome; none before the listener is up.
    pub fn export_url(&self, token: &str) -> Option<String> {
        let port = self.port.get()?;
        Some(format!(
            "http://127.0.0.1:{port}/export/{}",
            encode_segment(token)
        ))
    }

    /// Renders exports with this Chrome, a test's fake, rather than the machine's; they then
    /// stay off the clipboard (a test must not touch the user's).
    pub fn use_chrome(&self, program: PathBuf) {
        *self.chrome.lock().unwrap() = Some(program);
    }

    pub fn fake_chrome(&self) -> Option<PathBuf> {
        self.chrome.lock().unwrap().clone()
    }

    /// Binds the listener and serves until the daemon shuts down.
    pub async fn serve(daemon: std::sync::Arc<Daemon>) -> anyhow::Result<()> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        let _ = daemon.designs.port.set(port);
        tracing::info!(port, "design screens served");
        let router = Router::new()
            .route("/{secret}/{workspace}/{chat}/{*file}", get(screen_handler))
            .route("/export/{token}", get(export_page_handler))
            .route("/files/{secret}/{workspace}/{*path}", get(file_handler))
            .with_state(daemon.clone());
        axum::serve(listener, router)
            .with_graceful_shutdown(daemon.shutdown.clone().cancelled_owned())
            .await?;
        Ok(())
    }
}

/// A design chat's canvas, as its folder holds it now.
pub fn canvas(daemon: &Daemon, chat_key: &str) -> Result<CanvasResult> {
    let chat = daemon.chats.resolve(chat_key)?;
    if chat.kind != ChatKind::Design {
        return Err(Error::BadRequest("only a design chat has a canvas".into()));
    }
    let workspace = daemon.workspaces.row(&chat.workspace_id)?;
    if crate::relay::machine_of(daemon, &workspace.id).is_some() {
        return Err(Error::BadRequest(
            "this Canvas is on another machine; open it on the Mac".into(),
        ));
    }
    let dir = crate::design::design_dir(&workspace.path, &chat.id)
        .map_err(|error| Error::Internal(anyhow::anyhow!("the design folder: {error}")))?;
    let base = daemon.designs.base_url(&workspace.id, &chat.id);
    Ok(read_canvas(&chat.id, Path::new(&dir), base.as_deref()))
}

fn design_file(daemon: &Daemon, workspace: &str, chat_key: &str, file: &str) -> Option<PathBuf> {
    let chat = daemon.store.chat(chat_key).ok()??;
    if chat.kind != ChatKind::Design || chat.workspace_id != workspace {
        return None;
    }
    let Ok(row) = daemon.workspaces.row(workspace) else {
        return None;
    };
    resource_path(Path::new(&row.path), &chat.id, file)
}

fn resource_path(workspace: &Path, chat: &str, file: &str) -> Option<PathBuf> {
    let dir = workspace
        .join(crate::paths::NOTES_DIR)
        .join(crate::paths::DESIGNS_DIR)
        .join(chat);
    // The folder itself must not be reached through a symlink either.
    for path in [
        workspace.join(crate::paths::NOTES_DIR),
        dir.parent().unwrap_or(&dir).to_path_buf(),
        dir.clone(),
    ] {
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => {}
            _ => return None,
        }
    }
    let (path, _) = screen_file(&dir, file)?;
    Some(path)
}

/// Private mobile resource reads share the desktop server's root, symlink and MIME rules.
pub async fn resource(daemon: &Daemon, params: DesignResourceParams) -> Result<DesignResource> {
    let chat = daemon.chats.resolve(&params.chat)?;
    let workspace = daemon.workspaces.row(&chat.workspace_id)?;
    if crate::relay::machine_of(daemon, &workspace.id).is_some() {
        return Err(Error::BadRequest(
            "this Canvas is on another machine; open it on the Mac".into(),
        ));
    }
    let path = design_file(daemon, &workspace.id, &chat.id, &params.file).ok_or_else(|| {
        Error::NotFound("Canvas resource is missing or outside the design folder".into())
    })?;
    let relative = path
        .strip_prefix(&workspace.path)
        .map_err(|e| Error::Internal(e.into()))?;
    read_resource(
        Path::new(&workspace.path),
        relative
            .to_str()
            .ok_or_else(|| Error::BadRequest("invalid Canvas resource path".into()))?,
        params.offset,
        params.version.as_deref(),
    )
    .await
}

fn resource_version(meta: &std::fs::Metadata) -> String {
    format!(
        "{}:{}:{}:{}:{}",
        meta.ino(),
        meta.len(),
        meta.mtime(),
        meta.mtime_nsec(),
        meta.ctime_nsec()
    )
}

async fn read_resource(
    root: &Path,
    relative: &str,
    offset: u32,
    expected: Option<&str>,
) -> Result<DesignResource> {
    let mime = content_type(Path::new(relative))
        .ok_or_else(|| Error::BadRequest("this Canvas resource type cannot be opened".into()))?;
    let opened = crate::cloud_workspace_snapshot::open_file(root, relative)?
        .ok_or_else(|| Error::NotFound("Canvas resource is missing".into()))?;
    let mut file = tokio::fs::File::from_std(opened);
    let meta = file.metadata().await?;
    if !meta.is_file() {
        return Err(Error::BadRequest(
            "Canvas resource must be a regular file".into(),
        ));
    }
    if meta.len() > 16 * 1024 * 1024 {
        return Err(Error::BadRequest(
            "this Canvas resource exceeds the mobile limit of 16 MiB; open it on the Mac".into(),
        ));
    }
    let version = resource_version(&meta);
    if (offset > 0 && expected.is_none()) || expected.is_some_and(|v| v != version) {
        return Err(Error::BadRequest(
            "Canvas changed while opening; reopen it from the conversation".into(),
        ));
    }
    if u64::from(offset) > meta.len() {
        return Err(Error::BadRequest(
            "invalid Canvas resource offset; reopen it from the conversation".into(),
        ));
    }
    file.seek(std::io::SeekFrom::Start(u64::from(offset)))
        .await?;
    let mut bytes = Vec::new();
    let mut bounded = file.take(256 * 1024);
    bounded.read_to_end(&mut bytes).await?;
    if resource_version(&bounded.into_inner().metadata().await?) != version {
        return Err(Error::BadRequest(
            "Canvas changed while opening; reopen it from the conversation".into(),
        ));
    }
    Ok(DesignResource {
        mime: mime.into(),
        base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        total: meta.len() as u32,
        version,
    })
}

async fn screen_handler(
    State(daemon): State<std::sync::Arc<Daemon>>,
    UrlPath((secret, workspace, chat, file)): UrlPath<(String, String, String, String)>,
    RawQuery(query): RawQuery,
) -> Response {
    let not_found = || (StatusCode::NOT_FOUND, "not found").into_response();
    if secret != daemon.designs.secret {
        return not_found();
    }
    let Some(path) = design_file(&daemon, &workspace, &chat, &file) else {
        return not_found();
    };
    let Some(media_type) = content_type(&path) else {
        return not_found();
    };
    let Ok(mut bytes) = tokio::fs::read(&path).await else {
        return not_found();
    };
    if media_type.starts_with("text/html") && asks(query.as_deref(), "capture=1") {
        bytes.extend_from_slice(b"\n<script>");
        bytes.extend_from_slice(CAPTURE_SCRIPT.as_bytes());
        bytes.extend_from_slice(b"</script>\n");
    }
    let csp = if asks(query.as_deref(), "snapshot=1") {
        format!("{SCREEN_CSP}; {SNAPSHOT_SANDBOX}")
    } else {
        SCREEN_CSP.to_string()
    };
    (
        [
            (header::CONTENT_TYPE, media_type),
            (header::CONTENT_SECURITY_POLICY, csp.as_str()),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        bytes,
    )
        .into_response()
}

/// A file of the workspace's repos or notes, resolved as `files.read` resolves it (no `..`, no
/// way out of its root through a symlink), typed by extension as a screen's file and under the
/// same policy: the Files tab frames an HTML file from here, sandboxed, never on the app's or
/// the daemon's origin.
async fn file_handler(
    State(daemon): State<std::sync::Arc<Daemon>>,
    UrlPath((secret, workspace, path)): UrlPath<(String, String, String)>,
) -> Response {
    let not_found = || (StatusCode::NOT_FOUND, "not found").into_response();
    if secret != daemon.designs.secret {
        return not_found();
    }
    // Moved to another machine: the page and what it loads are read there.
    let moved = daemon
        .workspaces
        .row(&workspace)
        .ok()
        .and_then(|row| crate::relay::machine_of(&daemon, &row.id).map(|m| (row.id, m)));
    let file = match moved {
        Some((id, machine)) => remote_workspace_file(&daemon, &machine, &id, &path).await,
        None => workspace_file(&daemon, &workspace, &path).await,
    };
    let Some((media_type, bytes)) = file else {
        return not_found();
    };
    (
        [
            (header::CONTENT_TYPE, media_type),
            (header::CONTENT_SECURITY_POLICY, SCREEN_CSP),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        bytes,
    )
        .into_response()
}

/// A file of the workspace as the Files tab's preview serves it: its type by extension, its bytes
/// up to a screen file's limit. None when it is not one.
pub(crate) async fn workspace_file(
    daemon: &std::sync::Arc<Daemon>,
    workspace: &str,
    path: &str,
) -> Option<(&'static str, Vec<u8>)> {
    let (_, roots) = crate::rpc::file_roots(daemon, workspace).ok()?;
    let file = crate::files::resolve(&roots, path).ok()?;
    let media_type = content_type(&file)?;
    let meta = file.metadata().ok()?;
    if meta.len() > chauffeur_design::MAX_FILE_BYTES {
        return None;
    }
    Some((media_type, tokio::fs::read(&file).await.ok()?))
}

/// [`workspace_file`] of a workspace that runs on `machine`, read from its daemon's socket
/// (`/workspace-file/{workspace}`); typed here again, by the same rule.
async fn remote_workspace_file(
    daemon: &Daemon,
    machine: &crate::relay::Machine,
    workspace: &str,
    path: &str,
) -> Option<(&'static str, Vec<u8>)> {
    let media_type = content_type(Path::new(path))?;
    let route = format!("/workspace-file/{workspace}");
    let answer = daemon
        .relay
        .http_get(machine, &route, &[("path", path)], None)
        .await
        .ok()?;
    if !answer.status().is_success() {
        return None;
    }
    let bytes = answer.bytes().await.ok()?;
    (bytes.len() as u64 <= chauffeur_design::MAX_FILE_BYTES).then(|| (media_type, bytes.to_vec()))
}

/// An export's page, once: the screens it frames come from their own URLs, so the page itself
/// carries no policy of its own.
async fn export_page_handler(
    State(daemon): State<std::sync::Arc<Daemon>>,
    UrlPath(token): UrlPath<String>,
) -> Response {
    match daemon.designs.exports.take(&token) {
        Some(html) => ([(header::CACHE_CONTROL, "no-store")], Html(html)).into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// Whether a screen's URL has `pair` (`capture=1`, `snapshot=1`) among its query parameters.
fn asks(query: Option<&str>, pair: &str) -> bool {
    query.is_some_and(|q| q.split('&').any(|p| p == pair))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mobile_resources_stream_large_files_with_a_stable_version_and_bounded_responses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("image.png");
        let bytes = vec![42; 1024 * 1024 + 123];
        std::fs::write(&path, &bytes).unwrap();
        let mut offset = 0;
        let mut version = None;
        let mut received = Vec::new();
        while offset < bytes.len() {
            let part = read_resource(dir.path(), "image.png", offset as u32, version.as_deref())
                .await
                .unwrap();
            assert!(serde_json::to_vec(&part).unwrap().len() < 360_000);
            assert_eq!(part.total as usize, bytes.len());
            version = Some(part.version);
            let data = base64::engine::general_purpose::STANDARD
                .decode(part.base64)
                .unwrap();
            assert!(data.len() <= 256 * 1024);
            offset += data.len();
            received.extend(data);
        }
        assert_eq!(received, bytes);
        std::fs::write(&path, b"new image").unwrap();
        assert!(
            read_resource(dir.path(), "image.png", 256 * 1024, version.as_deref())
                .await
                .is_err()
        );
        assert!(
            read_resource(dir.path(), "image.png", 1, None)
                .await
                .is_err()
        );
        std::fs::File::create(&path)
            .unwrap()
            .set_len(16 * 1024 * 1024 + 1)
            .unwrap();
        assert!(
            read_resource(dir.path(), "image.png", 0, None)
                .await
                .unwrap_err()
                .to_string()
                .contains("16 MiB")
        );
    }

    #[test]
    fn mobile_resource_paths_cannot_leave_the_design_or_follow_symlinked_roots() {
        let workspace = tempfile::tempdir().unwrap();
        let dir = workspace
            .path()
            .join(crate::paths::NOTES_DIR)
            .join(crate::paths::DESIGNS_DIR)
            .join("chat");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.html"), b"canvas").unwrap();
        assert!(resource_path(workspace.path(), "chat", "a.html").is_some());
        for path in ["../a.html", "/etc/hosts", ".secret", "a\\b.html"] {
            assert!(resource_path(workspace.path(), "chat", path).is_none());
        }
        std::os::unix::fs::symlink(dir.join("a.html"), dir.join("linked.html")).unwrap();
        assert!(resource_path(workspace.path(), "chat", "linked.html").is_none());
        std::fs::rename(&dir, dir.with_extension("real")).unwrap();
        std::os::unix::fs::symlink(dir.with_extension("real"), &dir).unwrap();
        assert!(resource_path(workspace.path(), "chat", "a.html").is_none());
    }
}
