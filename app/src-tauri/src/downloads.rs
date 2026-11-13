//! Saving a chat's attachments to the user's Downloads folder.
//!
//! The UI gives the URLs of the daemon's `/attachments/…` route (token included) and the names to
//! save them under. The shell fetches them itself: the web view cannot save a file (WebKit ignores
//! `download` on a cross-origin link), and the URL, token included, must not reach the system
//! browser. It refuses any other route, so the command cannot fetch what the daemon serves elsewhere.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use tauri::Manager;

#[derive(Deserialize)]
pub struct Download {
    url: String,
    /// The name the file was attached under; made safe to use as a file name here.
    name: String,
}

/// Saves each file in Downloads, or in a new folder named `folder` there when given. Returns the
/// path of the file (one file) or of the folder (several), for the UI to tell the user.
#[tauri::command]
pub async fn save_downloads(
    app: tauri::AppHandle,
    files: Vec<Download>,
    folder: Option<String>,
) -> Result<String, String> {
    if files.is_empty() {
        return Err("no file to save".into());
    }
    let downloads = app
        .path()
        .download_dir()
        .map_err(|_| "this system has no Downloads folder".to_string())?;
    let target = match folder {
        Some(name) => {
            let dir = unique_path(&downloads, &safe_name(&name));
            std::fs::create_dir_all(&dir)
                .map_err(|error| format!("could not create {}: {error}", dir.display()))?;
            dir
        }
        None => downloads,
    };
    let client = reqwest::Client::new();
    let mut saved = Vec::new();
    for file in &files {
        let path = unique_path(&target, &safe_name(&file.name));
        fetch(&client, &file.url, &path)
            .await
            .map_err(|error| format!("could not save {}: {error}", file.name))?;
        saved.push(path);
    }
    let shown = if files.len() == 1 { &saved[0] } else { &target };
    Ok(shown.display().to_string())
}

async fn fetch(client: &reqwest::Client, url: &str, path: &Path) -> Result<(), String> {
    let url = url::Url::parse(url).map_err(|error| error.to_string())?;
    if !is_attachment_route(&url) {
        return Err("not an attachment".into());
    }
    let response = client
        .get(url)
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .map_err(|error| error.without_url().to_string())?;
    let bytes = response
        .bytes()
        .await
        .map_err(|error| error.without_url().to_string())?;
    tokio::fs::write(path, &bytes)
        .await
        .map_err(|error| error.to_string())
}

fn is_attachment_route(url: &url::Url) -> bool {
    matches!(url.scheme(), "http" | "https") && url.path().starts_with("/attachments/")
}

/// A name that stays inside its folder: no separator or control character, and not hidden.
fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.');
    if cleaned.is_empty() {
        "attachment".into()
    } else {
        cleaned.chars().take(200).collect()
    }
}

/// `name` in `dir`, or `name (2).ext`, `name (3).ext`… when it is taken: nothing is overwritten.
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|candidate| !candidate.exists())
        .expect("an unbounded range always yields a free name")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_cannot_leave_its_folder() {
        assert_eq!(safe_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(safe_name("a\\b:c.png"), "a_b_c.png");
        assert_eq!(safe_name(".hidden"), "hidden");
        assert_eq!(safe_name("  "), "attachment");
    }

    #[test]
    fn a_taken_name_gets_a_number_before_its_extension() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(unique_path(dir.path(), "a.png"), dir.path().join("a.png"));
        std::fs::write(dir.path().join("a.png"), b"x").unwrap();
        assert_eq!(
            unique_path(dir.path(), "a.png"),
            dir.path().join("a (2).png")
        );
        std::fs::write(dir.path().join("a (2).png"), b"x").unwrap();
        assert_eq!(
            unique_path(dir.path(), "a.png"),
            dir.path().join("a (3).png")
        );
        std::fs::write(dir.path().join("README"), b"x").unwrap();
        assert_eq!(
            unique_path(dir.path(), "README"),
            dir.path().join("README (2)")
        );
    }

    #[test]
    fn only_the_attachment_route_is_fetched() {
        let ok = |s: &str| is_attachment_route(&url::Url::parse(s).unwrap());
        assert!(ok("http://127.0.0.1:41000/attachments/w/a.png?token=t"));
        assert!(!ok("http://127.0.0.1:41000/deliverables/w?token=t"));
        assert!(!ok("file:///attachments/x"));
        assert!(!ok("http://127.0.0.1:41000/attachments/../sound"));
    }
}
