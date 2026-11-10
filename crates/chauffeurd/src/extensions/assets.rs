//! An installed extension's UI files over HTTP: its script, its stylesheet and the files under its
//! assets folder, nothing else of the package (never its engine nor its manifest).
//!
//! Core serves them at `/extension-assets/{id}/{revision}/{nonce}/{path}`, on its own port and on
//! the Office's browser gateway. The managed runtime resolves the revision for the nonce
//! (`Extensions::asset`): random per extension and per daemon run, it reads the installed
//! revisions' UI files and nothing else, and is never the daemon's token; a removed extension's
//! revisions are gone at once. A file is served only under the name the install recorded, from a
//! path without a link in it, and only while its bytes still match the install's digest.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chauffeur_protocol::extensions::ExtensionId;
use sha2::{Digest, Sha256};

use crate::extensions::packages;

/// The URL prefix of package files, on Core's port and on a browser gateway's.
pub const ROUTE: &str = "/extension-assets";

/// The largest UI file served: the Office's models are a few MB.
const MAX_FILE: u64 = 128 * 1024 * 1024;

/// What the managed runtime vouches for: an installed revision, as its install recorded it.
#[derive(Clone, Debug)]
pub struct VerifiedRevision {
    /// The package's folder, as `packages::Installed::check_files` returned it.
    pub root: PathBuf,
    /// Each file of the package and its SHA-256, as the install recorded them.
    pub files: BTreeMap<String, String>,
    pub script: String,
    pub stylesheet: Option<String>,
    /// The folder of the UI's other files, relative to the package.
    pub assets: String,
}

/// A UI file for a GET of `ROUTE/{id}/{revision}/{nonce}/{path}`, `revision` being what the
/// managed runtime resolved for that nonce (`Extensions::asset`), the request's Host and Origin
/// already checked by the caller (`origin` is the one it accepted, echoed for CORS).
pub(crate) async fn respond(
    revision: Option<VerifiedRevision>,
    path: &str,
    origin: Option<&str>,
) -> Response {
    let not_found = || (StatusCode::NOT_FOUND, "not found").into_response();
    let Some(files) = revision.filter(|files| servable(files, path)) else {
        return not_found();
    };
    let Some(digest) = files.files.get(path).cloned() else {
        return not_found();
    };
    let file = files.root.join(path);
    let Some(media_type) = media_type(&file) else {
        return not_found();
    };
    let root = files.root.clone();
    let read = tokio::task::spawn_blocking(move || read_verified(&root, &file, &digest)).await;
    let bytes = match read {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(Unservable::Missing)) => return not_found(),
        Ok(Err(Unservable::Changed)) => {
            tracing::warn!(
                root = %files.root.display(),
                path,
                "an installed extension file no longer matches its install: not served"
            );
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "This extension file changed since it was installed: reinstall the extension from Settings → Extensions",
            )
                .into_response();
        }
        Err(error) => {
            tracing::warn!(%error, "could not read an extension file");
            return (StatusCode::INTERNAL_SERVER_ERROR, "could not read the file").into_response();
        }
    };
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(media_type));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    // Opened as a page (an SVG), a file runs nothing and reaches nothing.
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; sandbox"),
    );
    headers.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("cross-origin"),
    );
    headers.insert(header::VARY, HeaderValue::from_static("Origin"));
    if let Some(origin) = origin.and_then(|origin| HeaderValue::from_str(origin).ok()) {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    }
    (headers, bytes).into_response()
}

pub(crate) fn extension(slug: &str) -> Option<ExtensionId> {
    serde_json::from_value(serde_json::Value::String(slug.into())).ok()
}

/// Whether `path` is one of the UI's files: its script, its stylesheet, or under its assets
/// folder, named exactly as the install recorded it, without a hidden part.
fn servable(files: &VerifiedRevision, path: &str) -> bool {
    if packages::relative(path).is_err()
        || path.contains('\0')
        || path
            .split('/')
            .any(|part| part.is_empty() || part.starts_with('.'))
        || !files.files.contains_key(path)
    {
        return false;
    }
    let assets = files.assets.trim_end_matches('/');
    path == files.script
        || files.stylesheet.as_deref() == Some(path)
        || path
            .strip_prefix(assets)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// What a UI may load; an HTML page never, so nothing served runs as a page of Core's origin.
fn media_type(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "map" => "application/json",
        "glb" => "model/gltf-binary",
        "gltf" => "model/gltf+json",
        "bin" => "application/octet-stream",
        "wasm" => "application/wasm",
        "ktx2" => "image/ktx2",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        _ => return None,
    })
}

enum Unservable {
    Missing,
    Changed,
}

/// The file's bytes, refused when a part of its path is a link, when it left the package's
/// folder, or when its bytes no longer match the install's digest.
fn read_verified(
    root: &Path,
    file: &Path,
    digest: &str,
) -> std::result::Result<Vec<u8>, Unservable> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let relative = file.strip_prefix(root).map_err(|_| Unservable::Missing)?;
    let mut walked = root.to_path_buf();
    for part in relative.components() {
        walked.push(part);
        let meta = std::fs::symlink_metadata(&walked).map_err(|_| Unservable::Missing)?;
        if meta.file_type().is_symlink() {
            return Err(Unservable::Changed);
        }
    }
    let canonical_root = root.canonicalize().map_err(|_| Unservable::Missing)?;
    let canonical = file.canonicalize().map_err(|_| Unservable::Missing)?;
    if !canonical.starts_with(&canonical_root) {
        return Err(Unservable::Changed);
    }
    let mut opened = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&canonical)
        .map_err(|_| Unservable::Changed)?;
    let meta = opened.metadata().map_err(|_| Unservable::Missing)?;
    if !meta.is_file() || meta.len() > MAX_FILE {
        return Err(Unservable::Changed);
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    (&mut opened)
        .take(MAX_FILE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Unservable::Missing)?;
    if hex::encode(Sha256::digest(&bytes)) != digest {
        return Err(Unservable::Changed);
    }
    Ok(bytes)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A verified Office package of one script, one stylesheet, one model, and an engine and a
    /// manifest that must never be served.
    pub(crate) fn package(dir: &Path) -> VerifiedRevision {
        let root = dir.join("package");
        std::fs::create_dir_all(root.join("ui/assets/models")).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let mut files = BTreeMap::new();
        for (name, bytes) in [
            ("ui/office.js", b"console.log('office')".as_slice()),
            ("ui/office.css", b".office{}".as_slice()),
            ("ui/assets/models/villager.glb", b"glTF-model".as_slice()),
            (
                "ui/assets/page.html",
                b"<script>alert(1)</script>".as_slice(),
            ),
            ("bin/office-engine", b"\x7fELF".as_slice()),
            ("extension.json", b"{}".as_slice()),
        ] {
            std::fs::write(root.join(name), bytes).unwrap();
            files.insert(name.to_string(), hex::encode(Sha256::digest(bytes)));
        }
        VerifiedRevision {
            root: root.canonicalize().unwrap(),
            files,
            script: "ui/office.js".into(),
            stylesheet: Some("ui/office.css".into()),
            assets: "ui/assets".into(),
        }
    }

    #[test]
    fn only_the_ui_files_the_install_recorded_are_servable() {
        let dir = tempfile::tempdir().unwrap();
        let files = package(dir.path());
        for allowed in [
            "ui/office.js",
            "ui/office.css",
            "ui/assets/models/villager.glb",
        ] {
            assert!(servable(&files, allowed), "{allowed}");
        }
        for refused in [
            "bin/office-engine",
            "extension.json",
            "ui/assets/missing.png",
            "../package/ui/office.js",
            "ui/../ui/office.js",
            "ui/./office.js",
            "ui//office.js",
            "/ui/office.js",
            "ui\\office.js",
            "ui/assets/.hidden",
            "",
        ] {
            assert!(!servable(&files, refused), "{refused}");
        }
        assert!(media_type(Path::new("ui/assets/page.html")).is_none());
    }
}
