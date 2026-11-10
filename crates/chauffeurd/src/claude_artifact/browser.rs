use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use reqwest::Url;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::{Error, Result};

const TIMEOUT: Duration = Duration::from_secs(40);

pub fn artifact_url(text: &str) -> Result<Url> {
    let mut url = Url::parse(text.trim()).map_err(|_| invalid_link())?;
    let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
    if url.scheme() != "https"
        || url.host_str() != Some("claude.ai")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || parts.len() != 2
        || parts[0] != "artifact"
        || parts[1].is_empty()
        || !parts[1]
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
    {
        return Err(invalid_link());
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn invalid_link() -> Error {
    Error::BadRequest("Paste a public Claude artifact link (https://claude.ai/artifact/…).".into())
}

pub fn project_url(text: &str) -> Option<Url> {
    let url = Url::parse(text).ok()?;
    let uuid = url
        .host_str()?
        .strip_suffix(".frame.claudeusercontent.com")?;
    uuid::Uuid::parse_str(uuid).ok()?;
    let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
    if url.scheme() != "https"
        || url.port().is_some()
        || parts.len() != 2
        || parts[0] != "_f"
        || !parts[1]
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        || parts[1].is_empty()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    url.join(&format!("/_f/{}/project/", parts[1])).ok()
}

/// An isolated browser loads Claude's public shell; no existing profile or login is read.
pub async fn resolve(chrome: &Path, url: &Url) -> Result<Url> {
    let profile = tempfile::tempdir().map_err(anyhow::Error::from)?;
    let child = tokio::process::Command::new(chrome)
        .args([
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            "--remote-debugging-port=0",
            "--remote-debugging-address=127.0.0.1",
        ])
        .arg(format!("--user-data-dir={}", profile.path().display()))
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            Error::Unavailable(format!("Could not start Chrome for the Claude import: {e}"))
        })?;
    let mut browser = Browser(child);
    let outcome = tokio::time::timeout(TIMEOUT, read_frame(profile.path(), url)).await
        .map_err(|_| Error::Unavailable("Claude did not expose a public design within 40 seconds: check that the link is public, or export its HTML instead.".into()))?;
    browser.stop();
    let _ = browser.0.wait().await;
    outcome
}

struct Browser(tokio::process::Child);
impl Browser {
    fn stop(&self) {
        if let Some(pid) = self.0.id() {
            // SAFETY: this child leads the process group created at spawn.
            unsafe {
                libc::killpg(pid as i32, libc::SIGKILL);
            }
        }
    }
}
impl Drop for Browser {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn read_frame(profile: &Path, url: &Url) -> Result<Url> {
    let port = loop {
        if let Ok(text) = tokio::fs::read_to_string(profile.join("DevToolsActivePort")).await
            && let Some(port) = text.lines().next().and_then(|s| s.parse::<u16>().ok())
        {
            break port;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let pages: Value = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/json/list"))
        .send()
        .await
        .map_err(anyhow::Error::from)?
        .json()
        .await
        .map_err(anyhow::Error::from)?;
    let socket = pages
        .as_array()
        .and_then(|pages| pages.iter().find(|p| p["type"] == "page"))
        .and_then(|p| p["webSocketDebuggerUrl"].as_str())
        .ok_or_else(|| Error::Unavailable("Chrome could not open the Claude import tab.".into()))?;
    let (mut ws, _) = tokio_tungstenite::connect_async(socket)
        .await
        .map_err(anyhow::Error::from)?;
    ws.send(Message::Text(
        json!({"id":1,"method":"Page.navigate","params":{"url":url.as_str()}})
            .to_string()
            .into(),
    ))
    .await
    .map_err(anyhow::Error::from)?;
    let mut id = 1u32;
    loop {
        id += 1;
        ws.send(Message::Text(json!({"id":id,"method":"Runtime.evaluate","params":{
            "expression":"Array.from(document.querySelectorAll('iframe')).map(f => f.src)","returnByValue":true
        }}).to_string().into())).await.map_err(anyhow::Error::from)?;
        while let Some(message) = ws.next().await {
            let message = message.map_err(anyhow::Error::from)?;
            let Message::Text(text) = message else {
                continue;
            };
            let response: Value = serde_json::from_str(&text).map_err(anyhow::Error::from)?;
            if response["id"].as_u64() != Some(u64::from(id)) {
                continue;
            }
            if let Some(frames) = response["result"]["result"]["value"].as_array() {
                for frame in frames {
                    if let Some(project) = frame.as_str().and_then(project_url) {
                        return Ok(project);
                    }
                }
            }
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_only_public_claude_links_and_pinned_artifact_origins() {
        assert!(artifact_url("https://claude.ai/artifact/abc123#page-1").is_ok());
        for url in [
            "http://claude.ai/artifact/abc",
            "https://example.com/artifact/abc",
            "https://claude.ai/chat/abc",
            "https://user@claude.ai/artifact/abc",
            "https://claude.ai/artifact/a/other",
        ] {
            assert!(artifact_url(url).is_err(), "{url}");
        }
        let base = "https://121a2245-82c5-4c88-800d-4788e867fecc.frame.claudeusercontent.com/_f/123-abc/?__frame_v=manifest.json";
        assert!(
            project_url(base)
                .unwrap()
                .as_str()
                .ends_with("/_f/123-abc/project/")
        );
        assert!(project_url("https://localhost/_f/123/").is_none());
        assert!(project_url("https://x.frame.claudeusercontent.com/_f/123/").is_none());
        assert!(
            project_url(
                "https://121a2245-82c5-4c88-800d-4788e867fecc.frame.claudeusercontent.com/other/"
            )
            .is_none()
        );
    }
}
