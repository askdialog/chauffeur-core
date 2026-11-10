//! Slack links only prefill the create dialog. No workspace or agent is started here.

use std::{collections::VecDeque, sync::Mutex};

use tauri::{Emitter, Manager};
use tauri_plugin_deep_link::DeepLinkExt;
use url::Url;

#[derive(Default)]
pub struct PendingLinks(Mutex<VecDeque<String>>);

pub fn focus(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// A `chauffeur://slack/<token>?scope=message|thread` link, the only kind kept: the daemon reads
/// its context from the Slack service it is configured with.
fn is_context_link(link: &str) -> bool {
    context_token(link).is_some()
}

fn context_token(link: &str) -> Option<(String, String)> {
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
    Some((token.to_string(), params[0].1.to_string()))
}

fn receive(app: &tauri::AppHandle, urls: Vec<Url>) {
    let state = app.state::<PendingLinks>();
    let mut links = state.0.lock().unwrap();
    for url in urls {
        if is_context_link(url.as_str()) && links.len() < 32 {
            links.push_back(url.into());
        }
    }
    if !links.is_empty() {
        drop(links);
        focus(app);
        let _ = app.emit_to("main", "slack-open", ());
    }
}

pub fn setup(app: &tauri::AppHandle) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    app.deep_link().register_all()?;
    let handle = app.clone();
    app.deep_link()
        .on_open_url(move |event| receive(&handle, event.urls()));
    if let Some(urls) = app.deep_link().get_current()? {
        receive(app, urls);
    }
    Ok(())
}

#[tauri::command]
pub fn take_slack_links(window: tauri::WebviewWindow) -> Result<Vec<String>, String> {
    if window.label() != "main" {
        return Err("Open Slack links in the main Chauffeur window.".into());
    }
    Ok(window
        .state::<PendingLinks>()
        .0
        .lock()
        .unwrap()
        .drain(..)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_can_only_read_context_from_the_chauffeur_service() {
        let token = "a".repeat(43);
        let link = format!("chauffeur://slack/{token}?scope=thread");
        assert_eq!(
            context_token(&link).unwrap(),
            (token.clone(), "thread".to_string())
        );
        for bad in [
            link.replace("chauffeur:", "https:"),
            link.replace("slack/", "evil.example/"),
            link.replace("slack/", "user@slack/"),
            link.replace("slack/", "slack:443/"),
            format!("{link}&scope=message"),
            format!("{link}&url=http://localhost"),
            format!("{link}#anything"),
            link.replace("thread", "run"),
            link.replace(&token, "../token"),
        ] {
            assert!(!is_context_link(&bad), "{bad}");
        }
    }
}

#[tauri::command]
pub fn focus_slack_desktop(window: tauri::WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Open Slack imports in the main Chauffeur window.".into());
    }
    focus(window.app_handle());
    Ok(())
}
