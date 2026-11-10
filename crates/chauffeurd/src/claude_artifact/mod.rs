//! Public Claude Design canvases copied into editable Chauffeur screens.
mod blobs;
mod browser;
mod manifest;
mod resources;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use chauffeur_protocol::client::ClaudeArtifactPreview;
use reqwest::{Client, Url};

use crate::{Daemon, Error, Result};

const MAX_BYTES: usize = 25 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 50 * 1024 * 1024;
const MAX_FILES: usize = 300;

pub struct Imported {
    pub preview: ClaudeArtifactPreview,
    files: BTreeMap<String, Vec<u8>>,
}

impl Imported {
    pub fn write(&self, dir: &Path) -> Result<()> {
        for (path, bytes) in &self.files {
            let relative = chauffeur_design::safe_relative(path).ok_or_else(|| {
                Error::BadRequest("The Claude artifact contains an unsafe file path.".into())
            })?;
            let target = dir.join(relative);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(anyhow::Error::from)?;
            }
            std::fs::write(target, bytes).map_err(anyhow::Error::from)?;
        }
        Ok(())
    }
}

pub async fn read(daemon: &Daemon, link: &str) -> Result<Imported> {
    let url = browser::artifact_url(link)?;
    let chrome = crate::design_export::find_windowed_chrome(daemon.shell_env.vars().get("PATH").map(String::as_str))
        .ok_or_else(|| Error::Unavailable("Install Google Chrome or Chromium to import a public Claude design, or export its HTML instead.".into()))?;
    let base = browser::resolve(&chrome, &url).await?;
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(25))
        .build()
        .map_err(anyhow::Error::from)?;
    tokio::time::timeout(Duration::from_secs(90), read_project(&client, &base))
        .await
        .map_err(|_| Error::Unavailable("Downloading the Claude design took too long: retry the import or export its HTML instead.".into()))?
}

async fn read_project(client: &Client, base: &Url) -> Result<Imported> {
    let bytes = download(
        client,
        &base.join("canvas.json").map_err(anyhow::Error::from)?,
    )
    .await?;
    let source: manifest::Manifest =
        serde_json::from_slice(&bytes).map_err(|_| {
            Error::BadRequest(
        "This link is not a supported public Claude Design canvas: export its HTML instead.".into())
        })?;
    let canvas = source.canvas()?;
    let preview = ClaudeArtifactPreview {
        title: canvas.title,
        pages: canvas.pages,
        screens: canvas.files.len() as u32,
        warnings: Vec::new(),
    };
    let mut imported = Imported {
        preview,
        files: BTreeMap::new(),
    };
    resources::copy(client, base, &canvas.files, &mut imported).await?;
    let manifest = serde_json::to_vec_pretty(&canvas.manifest).map_err(anyhow::Error::from)?;
    if imported.files.values().map(Vec::len).sum::<usize>() + manifest.len() > MAX_TOTAL_BYTES {
        return Err(Error::BadRequest(
            "This Claude design exceeds the 50 MB import limit: export a smaller design instead."
                .into(),
        ));
    }
    imported.files.insert("canvas.json".into(), manifest);
    Ok(imported)
}

async fn download(client: &Client, url: &Url) -> Result<Vec<u8>> {
    Ok(download_typed(client, url).await?.0)
}

async fn download_typed(client: &Client, url: &Url) -> Result<(Vec<u8>, String)> {
    let failure = || {
        Error::Unavailable("A file of the Claude artifact is unavailable: check that the link is public, or export its HTML instead.".into())
    };
    let mut response = client
        .get(url.clone())
        .send()
        .await
        .map_err(|_| failure())?
        .error_for_status()
        .map_err(|_| failure())?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return Err(failure());
    }
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .to_string();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| failure())? {
        if bytes.len() + chunk.len() > MAX_BYTES {
            return Err(failure());
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes
        .windows(b"__frame_denied".len())
        .any(|w| w == b"__frame_denied")
    {
        return Err(failure());
    }
    Ok((bytes, mime))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::get};
    use serde_json::json;

    #[tokio::test]
    async fn a_public_canvas_becomes_local_editable_screens_with_linked_assets() {
        let manifest = json!({"title":"Fixture","pages":[{"id":"one","name":"Page one"}],
            "boards":{"screen.dc.html":{"title":"Screen","page":"one","w":390,"h":844}}});
        let app = Router::new()
            .route("/project/canvas.json", get(move || async move { axum::Json(manifest) }))
            .route("/project/screen.dc.html", get(|| async { r#"<!doctype html><html><head><script src="./support.js"></script></head><body><x-dc><helmet><style>body{margin:0}</style></helmet><img src="assets/logo.svg"><img src="/_blob/724d639bdc0a552e34b8409d3793ed4d"></x-dc></body></html>"# }))
            .route("/project/assets/logo.svg", get(|| async { "<svg xmlns=\"http://www.w3.org/2000/svg\"><rect width=\"10\" height=\"10\"/></svg>" }))
            .route("/project/support.js", get(|| async { "<script>parent.postMessage({__frame_denied:true},'*')</script>" }));
        let app = app.route(
            "/_blob/724d639bdc0a552e34b8409d3793ed4d",
            get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "image/svg+xml")],
                    "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!(
            "http://{}/project/",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let imported = read_project(&Client::new(), &base).await.unwrap();
        assert_eq!(imported.preview.screens, 1);
        assert_eq!(imported.preview.warnings.len(), 1);
        let folder = tempfile::tempdir().unwrap();
        imported.write(folder.path()).unwrap();
        let html = std::fs::read_to_string(folder.path().join("screen.dc.html")).unwrap();
        assert!(!html.contains("support.js"));
        assert!(!html.contains("<x-dc>"));
        assert!(!html.contains("/_blob/"));
        assert!(
            folder
                .path()
                .join("claude-assets/724d639bdc0a552e34b8409d3793ed4d.svg")
                .exists()
        );
        assert!(folder.path().join("assets/logo.svg").exists());
        let canvas =
            chauffeur_design::read_canvas("test", folder.path(), Some("https://local.invalid/"));
        assert!(canvas.error.is_none());
        assert_eq!(canvas.pages[0].sections[0].screens[0].width, 390);
        server.abort();
    }

    #[tokio::test]
    #[ignore = "requires Chrome and a public Claude artifact URL"]
    async fn a_live_public_claude_design_can_be_imported_without_an_account() {
        let link = std::env::var("CLAUDE_ARTIFACT_URL").expect("CLAUDE_ARTIFACT_URL");
        let url = browser::artifact_url(&link).unwrap();
        let chrome =
            crate::design_export::find_windowed_chrome(std::env::var("PATH").ok().as_deref())
                .expect("Chrome");
        let base = browser::resolve(&chrome, &url).await.unwrap();
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(25))
            .build()
            .unwrap();
        let imported = read_project(&client, &base).await.unwrap();
        assert!(imported.preview.screens > 0);
        let folder = tempfile::tempdir().unwrap();
        imported.write(folder.path()).unwrap();
        let canvas =
            chauffeur_design::read_canvas("test", folder.path(), Some("https://local.invalid/"));
        assert!(canvas.error.is_none());
        assert_eq!(canvas.pages.len(), imported.preview.pages.len());
        println!(
            "Imported {}: {} pages, {} screens, warnings: {:?}",
            imported.preview.title,
            imported.preview.pages.len(),
            imported.preview.screens,
            imported.preview.warnings
        );
    }
}
