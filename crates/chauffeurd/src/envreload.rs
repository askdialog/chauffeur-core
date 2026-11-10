//! Reading the login shell's environment again (`env.reload`, `chauffeur env reload`): a variable
//! the user exported since the daemon started, such as an MCP server's token, reaches what starts
//! afterwards, and the idle chats whose broken servers take a value from a variable it set or
//! changed start again with it (decided with Justin 2026-10-18). Variables are named, never their
//! values.

use std::sync::Arc;

use chauffeur_protocol::client::EnvReloadResult;

use crate::shellenv::shell_profile;
use crate::{Daemon, Error, Result};

pub async fn reload(daemon: &Arc<Daemon>) -> Result<EnvReloadResult> {
    let needed = daemon.chats.missing_env();
    let shell_env = daemon.shell_env.clone();
    let change = tokio::task::spawn_blocking(move || shell_env.reload())
        .await
        .map_err(anyhow::Error::from)?
        .map_err(|error| {
            Error::Unavailable(format!(
                "could not read the login shell's environment: {error}"
            ))
        })?;
    let vars = daemon.shell_env.vars();
    tracing::info!(
        added = ?change.added,
        changed = ?change.changed,
        removed = ?change.removed,
        "login shell environment read again"
    );
    let (restarted, deferred) = if change.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        // Agents started from now on, by the sidecar running now, get it too.
        if let Err(error) = daemon.chats.sidecar.set_env(vars.clone()).await {
            tracing::warn!(%error, "could not give the sidecar the new environment");
        }
        // Only a variable given a value can bring a server up: one removed or emptied cannot.
        let set = change
            .added
            .iter()
            .chain(&change.changed)
            .filter(|name| vars.get(*name).is_some_and(|value| !value.is_empty()))
            .cloned()
            .collect();
        daemon.chats.restart_for_env(&set).await
    };
    let shell = daemon.shell_env.shell();
    Ok(EnvReloadResult {
        profile: shell_profile(&shell).to_string(),
        shell,
        added: change.added,
        changed: change.changed,
        removed: change.removed,
        still_missing: needed
            .into_iter()
            .filter(|name| vars.get(name).is_none_or(|value| value.is_empty()))
            .collect(),
        restarted,
        deferred,
    })
}
